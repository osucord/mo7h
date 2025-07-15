use std::sync::Arc;

mod task;

use serenity::all::{
    GenericChannelId, GuildId, Message, MessageId, Reaction, ReactionType, UserId,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::data::{
    database::{Database, EmoteUsageType},
    structs::Error,
};

#[derive(Default)]
pub struct EmoteProcessor {
    pub sender: Sender,
}

#[derive(Clone)]

struct EmoteUsage {
    channel: GenericChannelId,
    message: MessageId,
    user: UserId,
    guild: GuildId,
    reaction_type: ReactionType,
    now: chrono::DateTime<chrono::Utc>,
    kind: EmoteUsageType,
    message_author_id: Option<UserId>,
}

impl PartialEq for EmoteUsage {
    fn eq(&self, other: &Self) -> bool {
        self.channel == other.channel
            && self.message == other.message
            && self.user == other.user
            && self.guild == other.guild
            && self.reaction_type == other.reaction_type
            && self.kind == other.kind
        // message_author_id and now intentionally excluded
    }
}

impl Eq for EmoteUsage {}

impl std::hash::Hash for EmoteUsage {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.channel.hash(state);
        self.message.hash(state);
        self.user.hash(state);
        self.guild.hash(state);
        self.reaction_type.hash(state);
        self.kind.hash(state);
        // message_author_id and now intentionally excluded
    }
}

impl EmoteProcessor {
    /// Starts the background task, will run regardless of if an existing task is running (will not be dropped)
    ///
    /// Populates the task sender.
    pub async fn start_background_task(&self, db: Arc<Database>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.sender.set(tx).await;

        tokio::spawn(async move {
            task::start(db, rx).await;
        });
    }
}

#[derive(Default, Clone)]
pub struct Sender(Arc<tokio::sync::Mutex<Option<UnboundedSender<EmoteCommand>>>>);

impl Sender {
    pub async fn shutdown(&self) {
        let lock = self.0.lock().await;

        lock.as_ref()
            .map(|s: &UnboundedSender<EmoteCommand>| s.send(EmoteCommand::Shutdown));
    }

    /// Sets the sender to the provided `UnboundedSender`.
    async fn set(&self, tx: UnboundedSender<EmoteCommand>) {
        *self.0.lock().await = Some(tx);
    }

    /// Sends a reaction add event to the task, if one exists.
    ///
    /// Note: will not send if `guild_id` or `user_id` is missing.
    pub async fn reaction_add(&self, reaction: &Reaction) {
        let Some(guild) = reaction.guild_id else {
            return;
        };

        let Some(user) = reaction.user_id else { return };

        self.send_add_(
            reaction.channel_id,
            reaction.message_id,
            user,
            guild,
            reaction.emoji.clone(),
            EmoteUsageType::Reaction,
            reaction.message_author_id,
        )
        .await;
    }

    /// Sends a message event to the task, if one exists.
    ///
    /// Note: will not send if `guild_id` ois missing.
    pub async fn message_add(&self, message: &Message, reaction_type: ReactionType) {
        let Some(guild) = message.guild_id else {
            return;
        };

        self.send_add_(
            message.channel_id,
            message.id,
            message.author.id,
            guild,
            reaction_type,
            EmoteUsageType::Message,
            Some(message.author.id),
        )
        .await;
    }

    #[expect(clippy::too_many_arguments)]
    async fn send_add_(
        &self,
        channel: GenericChannelId,
        message: MessageId,
        user: UserId,
        guild: GuildId,
        reaction_type: ReactionType,
        kind: EmoteUsageType,
        message_author_id: Option<UserId>,
    ) {
        let now = chrono::Utc::now();
        let lock = self.0.lock().await;

        lock.as_ref().map(|s| {
            s.send(EmoteCommand::ReactionAdd(EmoteUsage {
                channel,
                message,
                user,
                guild,
                reaction_type,
                now,
                kind,
                message_author_id,
            }))
        });
    }

    /// Sends a reaction remove event to the task, if one exists.
    ///
    /// Note: will not send if `guild_id` or `user_id` is missing.
    pub async fn reaction_remove(&self, reaction: &Reaction) {
        let Some(guild) = reaction.guild_id else {
            return;
        };

        let Some(user) = reaction.user_id else {
            return;
        };

        self.send_remove_(
            reaction.channel_id,
            reaction.message_id,
            user,
            guild,
            reaction.emoji.clone(),
            EmoteUsageType::Reaction,
        )
        .await;
    }

    async fn send_remove_(
        &self,
        channel: GenericChannelId,
        message: MessageId,
        user: UserId,
        guild: GuildId,
        reaction_type: ReactionType,
        kind: EmoteUsageType,
    ) {
        let now = chrono::Utc::now();
        let lock = self.0.lock().await;

        lock.as_ref().map(|s| {
            s.send(EmoteCommand::ReactionRemove(EmoteUsage {
                channel,
                message,
                user,
                guild,
                reaction_type,
                now,
                kind,
                message_author_id: None,
            }))
        });
    }

    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(tokio::sync::Mutex::new(None)))
    }
}

enum EmoteCommand {
    ReactionAdd(EmoteUsage),
    ReactionRemove(EmoteUsage),
    Shutdown,
}

impl Drop for EmoteProcessor {
    fn drop(&mut self) {
        let sender = self.sender.clone();
        tokio::spawn(async move { sender.shutdown().await });
    }
}

impl Database {
    pub async fn get_emote_id(&self, key: &ReactionType) -> Result<i32, Error> {
        if let Some(cached_id) = self.emotes.get(key) {
            return Ok(*cached_id.value());
        }

        let (name, id) = match &key {
            ReactionType::Custom {
                animated: _,
                id,
                name,
            } => {
                let Some(name) = name else {
                    return Err("Name not found".into());
                };

                (name, Some(id.get() as i64))
            }
            ReactionType::Unicode(string) => (string, None),
            _ => return Err("Unknown reaction type".into()),
        };

        let id = if let Some(id) = id {
            sqlx::query!(
                r#"
                WITH input_rows(emote_name, discord_id) AS (
                    VALUES ($1::text, $2::bigint)
                ),
                ins AS (
                    INSERT INTO emotes (emote_name, discord_id)
                    SELECT emote_name, discord_id FROM input_rows
                    ON CONFLICT (discord_id) DO NOTHING
                    RETURNING id
                )
                SELECT id AS "id!" FROM ins
                UNION ALL
                SELECT e.id AS "id!" FROM emotes e
                JOIN input_rows i USING (discord_id)
                WHERE NOT EXISTS (SELECT 1 FROM ins);
                "#,
                name.as_str(),
                id
            )
            .fetch_one(&self.db)
            .await?
            .id
        } else {
            sqlx::query!(
                r#"
                WITH input_rows(emote_name) AS (
                    VALUES ($1::text)
                ),
                ins AS (
                    INSERT INTO emotes (emote_name)
                    SELECT emote_name FROM input_rows
                    ON CONFLICT (emote_name) WHERE discord_id IS NULL DO NOTHING
                    RETURNING id
                )
                SELECT id AS "id!" FROM ins
                UNION ALL
                SELECT e.id AS "id!"
                FROM emotes e
                JOIN input_rows i ON e.emote_name = i.emote_name
                WHERE e.discord_id IS NULL;
                "#,
                &name.as_str(),
            )
            .fetch_one(&self.db)
            .await?
            .id
        };

        Ok(id)
    }
}
