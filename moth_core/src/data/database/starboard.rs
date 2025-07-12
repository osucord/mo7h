use std::collections::{HashMap, HashSet};

use super::wrappers::{
    ChannelIdWrapper, MaybeMessageIdWrapper, MaybeUserIdWrapper, MessageIdWrapper, UserIdWrapper,
};
use crate::data::structs::Error;
use parking_lot::Mutex;
use serenity::all::{GenericChannelId, GuildId, MessageId, UserId};
use sqlx::query;

#[derive(Clone, Debug)]
pub struct StarboardMessage {
    pub id: i32,
    pub user_id: UserIdWrapper,
    pub username: String,
    pub avatar_url: Option<String>,
    pub content: String,
    pub channel_id: ChannelIdWrapper,
    pub message_id: MessageIdWrapper,
    pub attachment_urls: Vec<String>,
    pub star_count: i16,
    pub starboard_status: StarboardStatus,
    pub starboard_message_id: MessageIdWrapper,
    pub starboard_message_channel: ChannelIdWrapper,
    pub reply_message_id: MaybeMessageIdWrapper,
    pub reply_username: Option<String>,
    pub reply_user_id: MaybeUserIdWrapper,
    pub forwarded: bool,
}

#[derive(Debug, Clone, sqlx::Type, PartialEq)]
#[sqlx(type_name = "starboard_status")]
pub enum StarboardStatus {
    InReview,
    Accepted,
    Denied,
}

#[derive(Debug)]
pub struct StarboardHandler {
    messages: Vec<StarboardMessage>,
    being_handled: HashSet<MessageId>,
    // message id is the appropriate in messages, the first userid is the author
    // the collection is the reaction users.
    pub reactions_cache: HashMap<MessageId, (UserId, Vec<UserId>)>,
    pub overrides: HashMap<GenericChannelId, u8>,
}

impl StarboardHandler {
    pub(super) async fn new(db: &sqlx::PgPool) -> Result<Self, Error> {
        let results = sqlx::query!(
            r#"
            SELECT
                starboard_overrides.star_count,
                channels.channel_id
            FROM
                starboard_overrides
            JOIN
                channels
            ON
                starboard_overrides.channel_id = channels.id
            "#
        )
        .fetch_all(db)
        .await?;

        let mut overrides = HashMap::with_capacity(results.len());
        for result in results {
            overrides.insert(
                GenericChannelId::new(result.channel_id as u64),
                result.star_count as u8,
            );
        }

        Ok(Self {
            overrides,
            messages: Vec::new(),
            being_handled: HashSet::new(),
            reactions_cache: HashMap::new(),
        })
    }
}

impl super::Database {
    pub async fn get_starboard_msg(&self, msg_id: MessageId) -> Result<StarboardMessage, Error> {
        if let Some(starboard) = self
            .starboard
            .lock()
            .messages
            .iter()
            .find(|s| *s.message_id == msg_id)
            .cloned()
        {
            return Ok(starboard);
        }

        let starboard = self.get_starboard_msg_(msg_id).await?;

        self.starboard.lock().messages.push(starboard.clone());

        Ok(starboard)
    }

    async fn get_starboard_msg_(&self, msg_id: MessageId) -> Result<StarboardMessage, sqlx::Error> {
        sqlx::query_as!(
            StarboardMessage,
            r#"
            SELECT
                s.id,
                u.user_id,
                s.username,
                s.avatar_url,
                s.content,
                c.channel_id,
                m.message_id,
                s.attachment_urls,
                s.star_count,
                sm.message_id AS starboard_message_id,
                sc.channel_id AS starboard_message_channel,
                s.starboard_status as "starboard_status: StarboardStatus",
                rm.message_id AS "reply_message_id?",
                ru.user_id AS "reply_user_id?",
                s.forwarded,
                s.reply_username
            FROM starboard s
            JOIN users u ON s.user_id = u.id
            JOIN messages m ON s.message_id = m.id
            JOIN channels c ON m.channel_id = c.id

            LEFT JOIN messages rm ON s.reply_message_id = rm.id
            LEFT JOIN users ru ON rm.user_id = ru.id
            LEFT JOIN messages sm ON s.starboard_message_id = sm.id
            LEFT JOIN channels sc ON sm.channel_id = sc.id

            WHERE s.message_id = $1
            "#,
            self.get_message_dataless(msg_id).await?.id
        )
        .fetch_one(&self.db)
        .await
    }

    pub async fn update_star_count(&self, id: i32, count: i16) -> Result<(), sqlx::Error> {
        {
            let mut starboard = self.starboard.lock();
            let entry = starboard.messages.iter_mut().find(|s| s.id == id);
            if let Some(entry) = entry {
                entry.star_count = count;
            }
        };

        query!(
            "UPDATE starboard SET star_count = $1 WHERE id = $2",
            count,
            id,
        )
        .execute(&self.db)
        .await?;

        Ok(())
    }

    /// Check if a starboard is being handled, and if its not, handle it.
    ///
    /// returns if its already being handled.
    pub fn handle_starboard(&self, message_id: MessageId) -> bool {
        !self.starboard.lock().being_handled.insert(message_id)
    }

    /// Remove the safety check for a starboard being handled.
    pub fn stop_handle_starboard(&self, message_id: &MessageId) {
        self.starboard.lock().being_handled.remove(message_id);
    }

    pub async fn update_starboard_fields(&self, m: &StarboardMessage) -> Result<(), Error> {
        sqlx::query!(
            r#"
            UPDATE starboard
            SET
                content = $1,
                attachment_urls = $2,
                starboard_status = $3
            WHERE message_id = $4
            "#,
            m.content,
            &m.attachment_urls,
            m.starboard_status as _,
            self.get_message_dataless(*m.message_id).await?.id,
        )
        .execute(&self.db)
        .await?;

        let mut lock = self.starboard.lock();
        let index = lock
            .messages
            .iter()
            .position(|cache| cache.message_id.0 == m.message_id.0);

        if let Some(index) = index {
            lock.messages.remove(index);
        }

        Ok(())
    }

    pub async fn insert_starboard_msg(
        &self,
        m: StarboardMessage,
        guild_id: Option<GuildId>,
        bot_id: UserId,
    ) -> Result<(), sqlx::Error> {
        let m_id = *m.message_id;
        let _ = self.insert_starboard_msg_(m, guild_id, bot_id).await;
        self.stop_handle_starboard(&m_id);

        Ok(())
    }

    async fn insert_starboard_msg_(
        &self,
        mut m: StarboardMessage,
        guild_id: Option<GuildId>,
        bot_id: UserId,
    ) -> Result<(), Error> {
        let origin_message = self
            .get_message(*m.message_id, *m.channel_id, guild_id, *m.user_id)
            .await?;

        // bot always sends the message
        let starboard_message = self
            .get_message(
                *m.starboard_message_id,
                *m.starboard_message_channel,
                guild_id,
                bot_id,
            )
            .await?;

        let reply_message_id = if let Some(reply_message_id) = m.reply_message_id.0 {
            let reply_user_id = m.reply_user_id.0.ok_or::<Error>(
                "value pair should never be null at this stage, if you see this, Moxy messed up."
                    .into(),
            )?;

            let reply_msg = self
                .get_message(*reply_message_id, *m.channel_id, guild_id, *reply_user_id)
                .await?;

            Some(reply_msg.id)
        } else {
            None
        };

        let val = match sqlx::query!(
            r#"
            INSERT INTO starboard (
                user_id, username, avatar_url, content, message_id,
                attachment_urls, star_count, starboard_status,
                starboard_message_id, forwarded, reply_message_id, reply_username
            )
            VALUES (
                $1, $2, $3, $4, $5, $6,
                $7, $8, $9, $10, $11,
                $12
            ) RETURNING id
            "#,
            origin_message.user_id,
            m.username,
            m.avatar_url,
            m.content,
            origin_message.id,
            &m.attachment_urls,
            m.star_count,
            m.starboard_status as _,
            starboard_message.id,
            m.forwarded,
            reply_message_id,
            m.reply_username
        )
        .fetch_one(&self.db)
        .await
        {
            Ok(result) => result,
            Err(e) => {
                println!("SQL query failed: {e:?}");
                return Err(e.into());
            }
        };

        m.id = val.id;

        let mut lock = self.starboard.lock();
        let m_id = *m.message_id;

        lock.messages.push(m);
        lock.being_handled.remove(&m_id);

        Ok(())
    }

    pub async fn get_starboard_msg_by_starboard_id(
        &self,
        starboard_msg_id: MessageId,
    ) -> Result<StarboardMessage, Error> {
        if let Some(starboard) = self
            .starboard
            .lock()
            .messages
            .iter()
            .find(|s| *s.starboard_message_id == starboard_msg_id)
            .cloned()
        {
            return Ok(starboard);
        }

        let starboard = self
            .get_starboard_msg_by_starboard_id_(starboard_msg_id)
            .await?;

        self.starboard.lock().messages.push(starboard.clone());

        Ok(starboard)
    }

    // TODO: test
    async fn get_starboard_msg_by_starboard_id_(
        &self,
        starboard_msg_id: MessageId,
    ) -> Result<StarboardMessage, sqlx::Error> {
        sqlx::query_as!(
            StarboardMessage,
            r#"
            SELECT
                s.id,
                u.user_id,
                s.username,
                s.avatar_url,
                s.content,
                c.channel_id,
                m.message_id,
                s.attachment_urls,
                s.star_count,
                sm.message_id AS starboard_message_id,
                sc.channel_id AS starboard_message_channel,
                s.starboard_status as "starboard_status: StarboardStatus",
                rm.message_id AS "reply_message_id?",
                ru.user_id AS "reply_user_id?",
                s.forwarded,
                s.reply_username
            FROM starboard s
            JOIN users u ON s.user_id = u.id
            JOIN messages m ON s.message_id = m.id
            JOIN channels c ON m.channel_id = c.id

            LEFT JOIN messages rm ON s.reply_message_id = rm.id
            LEFT JOIN users ru ON rm.user_id = ru.id
            LEFT JOIN messages sm ON s.starboard_message_id = sm.id
            LEFT JOIN channels sc ON sm.channel_id = sc.id

            WHERE s.starboard_message_id = $1
            "#,
            self.get_message_dataless(starboard_msg_id).await?.id
        )
        .fetch_one(&self.db)
        .await
    }

    pub async fn approve_starboard(
        &self,
        bot_user_id: UserId,
        starboard_message_id: MessageId,
        starboard_message_channel: GenericChannelId,
        new_message_id: MessageId,
        new_channel_id: GenericChannelId,
    ) -> Result<(), Error> {
        let status = StarboardStatus::Accepted;

        // todo, use starboard config
        let guild_id = std::env::var("STARBOARD_GUILD")
            .unwrap()
            .parse::<u64>()
            .map(std::convert::Into::into)
            .ok();

        let new_message_data = self
            .get_message(
                new_message_id,
                new_channel_id,
                guild_id,
                // bot user always sends starboard
                bot_user_id,
            )
            .await?;

        let old_message_data = self
            .get_message(
                starboard_message_id,
                starboard_message_channel,
                guild_id,
                bot_user_id,
            )
            .await?;

        query!(
            "UPDATE starboard SET starboard_status = $1, starboard_message_id = $2 WHERE \
             starboard_message_id = $3",
            status as _,
            new_message_data.id,
            old_message_data.id,
        )
        .execute(&self.db)
        .await?;

        let mut lock = self.starboard.lock();
        let m = lock
            .messages
            .iter_mut()
            .find(|m| *m.starboard_message_id == starboard_message_id);

        if let Some(m) = m {
            m.starboard_message_channel = ChannelIdWrapper(new_channel_id);
            m.starboard_message_id = MessageIdWrapper(new_message_id);
            m.starboard_status = StarboardStatus::Accepted;
        }

        Ok(())
    }

    pub async fn deny_starboard(&self, starboard_message_id: MessageId) -> Result<(), Error> {
        let status = StarboardStatus::Denied;

        query!(
            "UPDATE starboard SET starboard_status = $1 WHERE starboard_message_id = $2",
            status as _,
            self.get_message_dataless(starboard_message_id).await?.id,
        )
        .execute(&self.db)
        .await?;

        let mut lock = self.starboard.lock();
        if let Some(index) = lock
            .messages
            .iter()
            .position(|m| *m.starboard_message_id == starboard_message_id)
        {
            lock.messages.remove(index);
        }

        Ok(())
    }

    pub async fn get_all_starboard(&self) -> Result<Vec<StarboardMessage>, Error> {
        let messages = sqlx::query_as!(
            StarboardMessage,
            r#"
            SELECT
                s.id,
                u.user_id,
                s.username,
                s.avatar_url,
                s.content,
                c.channel_id,
                m.message_id,
                s.attachment_urls,
                s.star_count,
                sm.message_id AS starboard_message_id,
                sc.channel_id AS starboard_message_channel,
                s.starboard_status as "starboard_status: StarboardStatus",
                rm.message_id AS "reply_message_id?",
                ru.user_id AS "reply_user_id?",
                s.forwarded,
                s.reply_username
            FROM starboard s
            JOIN users u ON s.user_id = u.id
            JOIN messages m ON s.message_id = m.id
            JOIN channels c ON m.channel_id = c.id

            LEFT JOIN messages rm ON s.reply_message_id = rm.id
            LEFT JOIN users ru ON rm.user_id = ru.id
            LEFT JOIN messages sm ON s.starboard_message_id = sm.id
            JOIN channels sc ON sm.channel_id = sc.id
            "#,
        )
        .fetch_all(&self.db)
        .await?;

        let mut guard = self.starboard.lock();

        for message in messages {
            if !guard.messages.iter().any(|m| m.id == message.id) {
                guard.messages.push(message);
            }
        }

        Ok(guard.messages.clone())
    }

    pub async fn add_starboard_override(
        &self,
        starboard_handler: &Mutex<StarboardHandler>,
        channel_id: GenericChannelId,
        starcount: u8,
    ) -> Result<(), Error> {
        // TODO: use starboard config directly to avoid panic
        let guild_id = std::env::var("STARBOARD_GUILD")
            .unwrap()
            .parse::<u64>()
            .map(std::convert::Into::into)
            .ok();

        self.get_channel(channel_id, guild_id).await?;

        sqlx::query!(
            r#"
            INSERT INTO starboard_overrides (channel_id, star_count)
            VALUES ($1, $2)
            ON CONFLICT (channel_id) DO UPDATE
            SET star_count = EXCLUDED.star_count
            "#,
            self.get_channel(channel_id, guild_id).await?.0,
            i16::from(starcount)
        )
        .execute(&self.db)
        .await?;

        starboard_handler
            .lock()
            .overrides
            .insert(channel_id, starcount);

        Ok(())
    }

    pub async fn remove_starboard_override(
        &self,
        starboard_handler: &Mutex<StarboardHandler>,
        channel_id: GenericChannelId,
    ) -> Result<bool, Error> {
        let result = sqlx::query!(
            "DELETE FROM starboard_overrides WHERE channel_id = $1",
            channel_id.get() as i64
        )
        .execute(&self.db)
        .await?;

        if result.rows_affected() == 0 {
            return Ok(false);
        }

        starboard_handler.lock().overrides.remove(&channel_id);

        Ok(true)
    }
}
