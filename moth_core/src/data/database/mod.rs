use ::serenity::{
    all::{ChannelId, GuildId, MessageId, RoleId},
    small_fixed_array::FixedString,
};
use dashmap::{DashMap, DashSet};
use parking_lot::Mutex;
use serenity::all::UserId;
use sqlx::{Executor, PgPool, postgres::PgPoolOptions, query};
use std::{collections::HashSet, env, sync::Arc, time::Duration};

use crate::data::{
    database::private_vcs::task::GUILD,
    structs::{DmActivity, Error},
};

use lumi::serenity_prelude as serenity;

pub mod auth;
pub mod starboard;
pub mod wrappers;
pub use starboard::*;
pub use wrappers::*;
pub mod private_vcs;
pub mod reactions;

pub async fn init_data() -> Database {
    let database_url =
        env::var("DATABASE_URL").expect("No database url found in environment variables!");

    let database = PgPoolOptions::new()
        .connect(&database_url)
        .await
        .expect("Failed to connect to database!");

    database
        .execute("SET client_encoding TO 'UTF8'")
        .await
        .unwrap();

    sqlx::migrate!("../migrations")
        .run(&database)
        .await
        .expect("Unable to apply migrations!");

    let cache = mini_moka::sync::CacheBuilder::new(500)
        .time_to_idle(Duration::from_secs(3600))
        .build();

    let private_vc_cache = mini_moka::sync::CacheBuilder::new(500)
        .time_to_idle(Duration::from_secs(1800))
        .build();

    Database {
        starboard: Mutex::new(
            StarboardHandler::new(&database)
                .await
                .expect("should be setup correctly."),
        ),
        db: database,
        users: cache,
        dm_activity: DashMap::new(),
        channels: DashMap::new(),
        guilds: DashMap::new(),
        messages: DashMap::new(),
        emotes: DashMap::new(),
        private_vc: private_vc_cache,
    }
}

/// Custom type.
#[derive(Debug, Clone, Copy, sqlx::Type, PartialEq, Eq, Hash)]
#[sqlx(type_name = "emoteusagetype", rename_all = "lowercase")]
pub enum EmoteUsageType {
    Message,
    Reaction,
}

pub struct Database {
    pub db: PgPool,
    users: mini_moka::sync::Cache<UserId, Arc<ApplicationUser>>,
    // TODO: simplify this store.
    guilds: DashMap<serenity::GuildId, i32>,
    channels: DashMap<serenity::GenericChannelId, (i32, Option<i32>)>,
    messages: DashMap<serenity::MessageId, MessageData>,
    emotes: DashMap<serenity::ReactionType, i32>,
    // TODO: return privacy after i decide to make proper getters and setters
    pub private_vc: mini_moka::sync::Cache<ChannelId, Option<Arc<PrivateVc>>>,
    pub starboard: Mutex<starboard::StarboardHandler>,
    // TODO: try and keep private and rewrite them when i eventually redo my users and starboard part.
    /// Runtime caches for dm activity.
    pub(crate) dm_activity: DashMap<UserId, DmActivity>,
}

pub struct Transaction<'a> {
    #[expect(dead_code)]
    database: &'a Database,
    tx: sqlx::Transaction<'a, sqlx::Postgres>,
}

impl<'a> Database {
    pub async fn begin_transaction(&'a self) -> Result<Transaction<'a>, sqlx::Error> {
        let tx = self.db.begin().await?;

        Ok(Transaction { database: self, tx })
    }
}

impl Transaction<'_> {
    pub async fn commit(self) -> Result<(), sqlx::Error> {
        self.tx.commit().await
    }

    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        self.tx.rollback().await
    }
}

#[derive(Clone, Debug)]
pub struct PrivateVc {
    pub owner_id: UserId,
    pub message_id: Option<MessageId>,
    pub trusted_users: Vec<UserId>,
    pub allowlist_roles: Vec<RoleId>,
    pub allowlist_users: Vec<UserId>,
    pub denylist_users: Vec<UserId>,
}

#[bool_to_bitflags::bool_to_bitflags]
pub struct ApplicationUser {
    pub id: i32,
    pub is_banned: bool,
    pub is_admin: bool,
    pub allowed_admin_commands: Option<Vec<FixedString<u8>>>,
}

impl ApplicationUser {
    #[must_use]
    pub fn new(
        id: i32,
        is_banned: bool,
        is_admin: bool,
        allowed_admin_commands: Option<Vec<FixedString<u8>>>,
    ) -> Self {
        let mut app = Self {
            id,
            allowed_admin_commands,
            __generated_flags: ApplicationUserGeneratedFlags::empty(),
        };

        app.set_is_admin(is_admin);
        app.set_is_banned(is_banned);

        app
    }
}

#[derive(Clone, Debug, Default)]
pub struct Checks {
    // Users under this will have access to all owner commands.
    pub owners_all: DashSet<UserId>,
    pub owners_single: DashMap<String, HashSet<UserId>>,
}

impl Database {
    pub async fn get_user(
        &self,
        user_id: serenity::UserId,
    ) -> Result<Arc<ApplicationUser>, sqlx::Error> {
        if let Some(user) = self.users.get(&user_id) {
            return Ok(user);
        }

        self.insert_user_(user_id).await.map(Arc::new)
    }

    pub async fn insert_user_(
        &self,
        user_id: serenity::UserId,
    ) -> Result<ApplicationUser, sqlx::Error> {
        let row = query!(
            r#"
            WITH input_rows(user_id) AS (
                VALUES ($1::bigint)
            ),
            ins AS (
                INSERT INTO users (user_id)
                SELECT user_id FROM input_rows
                ON CONFLICT (user_id) DO NOTHING
                RETURNING
                    id,
                    is_bot_banned,
                    is_bot_admin,
                    allowed_admin_commands
            )
            SELECT
                id AS "id!",
                is_bot_banned AS "is_bot_banned!",
                is_bot_admin AS "is_bot_admin!",
                allowed_admin_commands
            FROM ins
            UNION ALL
            SELECT
                u.id AS "id!",
                u.is_bot_banned AS "is_bot_banned!",
                u.is_bot_admin AS "is_bot_admin!",
                u.allowed_admin_commands
            FROM users u
            JOIN input_rows i USING (user_id);
            "#,
            user_id.get() as i64
        )
        .fetch_one(&self.db)
        .await?;

        let application_user = ApplicationUser::new(
            row.id,
            row.is_bot_banned,
            row.is_bot_admin,
            row.allowed_admin_commands.map(|i| {
                i.iter()
                    .map(|s| FixedString::from_str_trunc(s))
                    .collect::<Vec<_>>()
            }),
        );

        Ok(application_user)
    }

    /// Gets the channel from the database, or inserts it if it doesn't exist, returning the inner `channel_id`, and `guild_id` values.
    pub async fn get_channel(
        &self,
        channel_id: serenity::GenericChannelId,
        guild_id: Option<serenity::GuildId>,
    ) -> Result<(i32, Option<i32>), sqlx::Error> {
        if let Some(id) = self.channels.get(&channel_id) {
            let value = id.value();
            return Ok((value.0, value.1));
        }

        self.insert_channel_(channel_id, guild_id).await
    }

    // inserts the channel and guild_id, returning both the inner ids if available.
    async fn insert_channel_(
        &self,
        channel_id: serenity::GenericChannelId,
        guild_id: Option<serenity::GuildId>,
    ) -> Result<(i32, Option<i32>), sqlx::Error> {
        let inner_guild_id = if let Some(guild_id) = guild_id {
            Some(self.get_guild(guild_id).await?)
        } else {
            None
        };

        let row = query!(
            r#"
            WITH input_rows(channel_id, guild_id) AS (
                VALUES ($1::bigint, $2::int)
            ),
            ins AS (
                INSERT INTO channels (channel_id, guild_id)
                SELECT channel_id, guild_id FROM input_rows
                ON CONFLICT (channel_id) DO NOTHING
                RETURNING id
            )
            SELECT id AS "id!" FROM ins
            UNION ALL
            SELECT c.id AS "id!" FROM channels c
            JOIN input_rows i USING (channel_id);
            "#,
            channel_id.get() as i64,
            inner_guild_id,
        )
        .fetch_one(&self.db)
        .await?;

        Ok((row.id, inner_guild_id))
    }

    pub async fn get_message(
        &self,
        message_id: MessageId,
        channel_id: serenity::GenericChannelId,
        guild_id: Option<serenity::GuildId>,
        user_id: UserId,
    ) -> Result<MessageData, sqlx::Error> {
        if let Some(message) = self.messages.get(&message_id) {
            return Ok(*message);
        }

        let (channel_id, guild_id) = self.get_channel(channel_id, guild_id).await?;
        let user_id = self.get_user(user_id).await?.id;

        let row = query!(
            r#"
            WITH input_rows(message_id, channel_id, user_id, guild_id) AS (
                VALUES ($1::bigint, $2::int, $3::int, $4::int)
            ),
            ins AS (
                INSERT INTO messages (message_id, channel_id, user_id, guild_id)
                SELECT message_id, channel_id, user_id, guild_id FROM input_rows
                ON CONFLICT (message_id) DO NOTHING
                RETURNING id
            )
            SELECT id AS "id!" FROM ins
            UNION ALL
            SELECT m.id AS "id!" FROM messages m
            JOIN input_rows i USING (message_id);
            "#,
            message_id.get() as i64,
            channel_id,
            user_id,
            guild_id
        )
        .fetch_one(&self.db)
        .await?;

        let message_data = MessageData {
            id: row.id,
            channel_id,
            guild_id,
            user_id,
        };
        self.messages.insert(message_id, message_data);

        Ok(message_data)
    }

    /// a version of `Self::get_messages` that will not insert if not present.
    pub async fn get_message_dataless(
        &self,
        message_id: MessageId,
    ) -> Result<MessageData, sqlx::Error> {
        if let Some(message) = self.messages.get(&message_id) {
            return Ok(*message);
        }

        let message_data = sqlx::query_as!(
            MessageData,
            "SELECT id, channel_id, user_id, guild_id FROM messages WHERE message_id = $1",
            message_id.get() as i64
        )
        .fetch_one(&self.db)
        .await?;

        self.messages.insert(message_id, message_data);

        Ok(message_data)
    }

    pub fn get_cached_message(&self, message_id: &MessageId) -> Option<MessageData> {
        self.messages.get(message_id).map(|v| *v)
    }

    /// Gets the guild from the database, or inserts it if it doesn't exist, returning the inner id value.
    pub async fn get_guild(&self, guild_id: serenity::GuildId) -> Result<i32, sqlx::Error> {
        if let Some(id) = self.guilds.get(&guild_id) {
            return Ok(*id.value());
        }

        self.insert_guild_(guild_id).await
    }

    async fn insert_guild_(&self, guild_id: serenity::GuildId) -> Result<i32, sqlx::Error> {
        let row = query!(
            r#"
            WITH input_rows(guild_id) AS (
                VALUES ($1::bigint)
            ),
            ins AS (
                INSERT INTO guilds (guild_id)
                SELECT guild_id FROM input_rows
                ON CONFLICT (guild_id) DO NOTHING
                RETURNING id
            )
            SELECT id AS "id!" FROM ins
            UNION ALL
            SELECT g.id AS "id!" FROM guilds g
            JOIN input_rows i USING (guild_id);
        "#,
            guild_id.get() as i64
        )
        .fetch_one(&self.db)
        .await?;

        Ok(row.id)
    }

    /// Sets the user banned/unbanned from the bot, returning the old status.
    pub async fn set_banned(&self, user_id: UserId, set_banned: bool) -> Result<(), Error> {
        let is_cached = if let Some(cached_user) = self.users.get(&user_id) {
            if cached_user.is_banned() == set_banned {
                return Ok(());
            }

            true
        } else {
            false
        };

        if is_cached {
            query!(
                "UPDATE users SET is_bot_banned  = $1 WHERE user_id = $2",
                set_banned,
                user_id.get() as i64
            )
            .execute(&self.db)
            .await?;
        } else {
            let row = query!(
                r#"
                INSERT INTO users (user_id, is_bot_banned)
                VALUES ($1, $2)
                ON CONFLICT (user_id)
                DO UPDATE SET is_bot_banned = EXCLUDED.is_bot_banned
                RETURNING id, is_bot_admin, allowed_admin_commands
                "#,
                user_id.get() as i64,
                set_banned
            )
            .fetch_one(&self.db)
            .await?;

            let application_user = ApplicationUser::new(
                row.id,
                set_banned,
                row.is_bot_admin,
                row.allowed_admin_commands.map(|i| {
                    i.iter()
                        .map(|s| FixedString::from_str_trunc(s))
                        .collect::<Vec<_>>()
                }),
            );

            self.users.insert(user_id, Arc::new(application_user));
        }

        Ok(())
    }

    /// To be called in a function that uses the admin check.
    pub async fn check_admin(&self, user_id: UserId, command: &str) -> Result<bool, Error> {
        let user = self.get_user(user_id).await?;

        if user.is_admin() {
            return Ok(true);
        }

        if let Some(commands) = &user.allowed_admin_commands {
            return Ok(commands.iter().any(|c| c.as_str() == command));
        }

        Ok(false)
    }

    /// Sets or unsets a user's admin access to this bot.
    pub async fn set_admin(
        &self,
        user_id: UserId,
        command: Option<&str>,
        enable: bool,
    ) -> Result<bool, Error> {
        let Some(command) = command else {
            if let Some(cached) = self.users.get(&user_id)
                && cached.is_admin() == enable
            {
                return Ok(true);
            }

            sqlx::query!(
                r#"
                INSERT INTO users (user_id, is_bot_admin)
                VALUES ($1, $2)
                ON CONFLICT (user_id)
                DO UPDATE SET is_bot_admin = EXCLUDED.is_bot_admin
                "#,
                user_id.get() as i64,
                enable
            )
            .execute(&self.db)
            .await?;

            return Ok(true);
        };

        // If command is present, handle add/remove
        if let Some(cached) = self.users.get(&user_id) {
            let already_has = cached
                .allowed_admin_commands
                .as_ref()
                .is_some_and(|array| array.iter().any(|c| c == command));

            if already_has == enable {
                return Ok(true);
            }
        }

        if enable {
            // Add command if not present
            sqlx::query!(
                r#"
                INSERT INTO users (user_id, allowed_admin_commands)
                VALUES ($1, ARRAY[$2])
                ON CONFLICT (user_id)
                DO UPDATE SET allowed_admin_commands =
                    CASE
                        WHEN NOT $2 = ANY(users.allowed_admin_commands) THEN
                            array_append(users.allowed_admin_commands, $2)
                        ELSE users.allowed_admin_commands
                    END
                "#,
                user_id.get() as i64,
                command
            )
            .execute(&self.db)
            .await?;
        } else {
            // Remove command if present
            sqlx::query!(
                r#"
                INSERT INTO users (user_id, allowed_admin_commands)
                VALUES ($1, ARRAY[]::TEXT[])
                ON CONFLICT (user_id)
                DO UPDATE SET allowed_admin_commands = array_remove(users.allowed_admin_commands, $2)
                "#,
                user_id.get() as i64,
                command
            )
            .execute(&self.db)
            .await?;
        }

        Ok(false)
    }

    pub async fn get_private_vc(
        &self,
        channel_id: serenity::ChannelId,
        guild_id: Option<serenity::GuildId>,
    ) -> Option<Arc<PrivateVc>> {
        if let Some(private_vc) = self.private_vc.get(&channel_id) {
            return private_vc;
        }

        if let Some(vc) = self.get_private_vc_(channel_id, guild_id).await {
            let vc = Some(Arc::new(vc));

            self.private_vc.insert(channel_id, vc.clone());

            return vc.clone();
        }

        None
    }

    pub async fn get_private_vc_(
        &self,
        channel_id: serenity::ChannelId,
        guild_id: Option<serenity::GuildId>,
    ) -> Option<PrivateVc> {
        let results = query!(
            r#"
            SELECT
                ch.channel_id AS channel_id,
                owner.user_id AS owner_id,
                pv.allowlist_roles,
                COALESCE(alu.allowlist_users, ARRAY[]::BIGINT[]) AS allowlist_users,
                COALESCE(tru.trusted_users, ARRAY[]::BIGINT[]) AS trusted_users,
                COALESCE(dnu.denylist_users, ARRAY[]::BIGINT[]) AS denylist_users,
                msg.message_id AS "message_id?"
            FROM private_vcs pv
            JOIN users owner ON pv.owner_id = owner.id
            JOIN channels ch ON pv.channel_id = ch.id
            LEFT JOIN messages msg ON pv.message_id = msg.id

            LEFT JOIN LATERAL (
                SELECT ARRAY_AGG(DISTINCT u.user_id) AS allowlist_users
                FROM private_vc_allowlist_users al
                LEFT JOIN users u ON al.user_id = u.id
                WHERE al.channel_id = pv.channel_id
            ) alu ON TRUE

            LEFT JOIN LATERAL (
                SELECT ARRAY_AGG(DISTINCT u.user_id) AS trusted_users
                FROM private_vc_trusted_users tu
                LEFT JOIN users u ON tu.user_id = u.id
                WHERE tu.channel_id = pv.channel_id
            ) tru ON TRUE

            LEFT JOIN LATERAL (
                SELECT ARRAY_AGG(DISTINCT u.user_id) AS denylist_users
                FROM private_vc_denylist_users du
                LEFT JOIN users u ON du.user_id = u.id
                WHERE du.channel_id = pv.channel_id
            ) dnu ON TRUE
            WHERE pv.channel_id = $1
            "#,
            self.get_channel(channel_id.widen(), guild_id).await.ok()?.0
        )
        .fetch_one(&self.db)
        .await
        .ok()?;

        Some(PrivateVc {
            owner_id: UserId::new(results.owner_id as u64),
            message_id: results.message_id.map(|m| MessageId::new(m as u64)),
            trusted_users: results
                .trusted_users
                .unwrap_or_default()
                .into_iter()
                .map(|u| UserId::new(u as u64))
                .collect(),
            allowlist_roles: results
                .allowlist_roles
                .into_iter()
                .map(|r| RoleId::new(r as u64))
                .collect(),
            allowlist_users: results
                .allowlist_users
                .unwrap_or_default()
                .into_iter()
                .map(|u| UserId::new(u as u64))
                .collect(),
            denylist_users: results
                .denylist_users
                .unwrap_or_default()
                .into_iter()
                .map(|u| UserId::new(u as u64))
                .collect(),
        })
    }

    pub async fn get_all_private_vcs(&self) -> Result<Vec<(ChannelId, Arc<PrivateVc>)>, Error> {
        let records = sqlx::query!(
            r#"
            SELECT
                ch.channel_id AS channel_id,
                owner.user_id AS owner_id,
                pv.allowlist_roles,
                COALESCE(alu.allowlist_users, ARRAY[]::BIGINT[]) AS allowlist_users,
                COALESCE(tru.trusted_users, ARRAY[]::BIGINT[]) AS trusted_users,
                COALESCE(dnu.denylist_users, ARRAY[]::BIGINT[]) AS denylist_users,
                msg.message_id AS "message_id?"
            FROM private_vcs pv
            JOIN users owner ON pv.owner_id = owner.id
            JOIN channels ch ON pv.channel_id = ch.id
            LEFT JOIN messages msg ON pv.message_id = msg.id

            LEFT JOIN LATERAL (
                SELECT ARRAY_AGG(DISTINCT u.user_id) AS allowlist_users
                FROM private_vc_allowlist_users al
                LEFT JOIN users u ON al.user_id = u.id
                WHERE al.channel_id = pv.channel_id
            ) alu ON TRUE

            LEFT JOIN LATERAL (
                SELECT ARRAY_AGG(DISTINCT u.user_id) AS trusted_users
                FROM private_vc_trusted_users tu
                LEFT JOIN users u ON tu.user_id = u.id
                WHERE tu.channel_id = pv.channel_id
            ) tru ON TRUE

            LEFT JOIN LATERAL (
                SELECT ARRAY_AGG(DISTINCT u.user_id) AS denylist_users
                FROM private_vc_denylist_users du
                LEFT JOIN users u ON du.user_id = u.id
                WHERE du.channel_id = pv.channel_id
            ) dnu ON TRUE
        "#
        )
        .fetch_all(&self.db)
        .await?;

        let mut result = Vec::with_capacity(records.len());

        for row in records {
            let channel_id = ChannelId::new(row.channel_id as u64);
            let owner_id = UserId::new(row.owner_id as u64);

            let allowlist_roles = row
                .allowlist_roles
                .into_iter()
                .map(|r| RoleId::new(r as u64))
                .collect::<Vec<_>>();

            let allowlist_users = row
                .allowlist_users
                .unwrap_or_default()
                .into_iter()
                .map(|id| UserId::new(id as u64))
                .collect::<Vec<_>>();

            let trusted_users = row
                .trusted_users
                .unwrap_or_default()
                .into_iter()
                .map(|id| UserId::new(id as u64))
                .collect::<Vec<_>>();

            let denylist_users = row
                .denylist_users
                .unwrap_or_default()
                .into_iter()
                .map(|id| UserId::new(id as u64))
                .collect::<Vec<_>>();

            let private_vc = Arc::new(PrivateVc {
                owner_id,
                message_id: row.message_id.map(|m| MessageId::new(m as u64)),
                trusted_users,
                allowlist_roles,
                allowlist_users,
                denylist_users,
            });

            self.private_vc.insert(channel_id, Some(private_vc.clone()));
            result.push((channel_id, private_vc));
        }

        Ok(result)
    }

    async fn hydrate_users(&self, users: Vec<UserId>) -> Result<Vec<i32>, Error> {
        let mut hydrated = Vec::with_capacity(users.len());
        for user_id in users {
            // TODO: bulk method
            let user = self.get_user(user_id).await?;
            hydrated.push(user.id);
        }
        Ok(hydrated)
    }

    // TODO: refactor this horrid mess and split this out
    #[expect(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn create_private_vc(
        &self,
        channel_id: serenity::ChannelId,
        message_id: Option<MessageId>,
        guild_id: Option<serenity::GuildId>,
        owner_id: UserId,
        allowlist_roles: Vec<RoleId>,
        allowlist_users: Vec<UserId>,
        trusted_users: Vec<UserId>,
        denylist_users: Vec<UserId>,
        bot_id: UserId,
    ) -> Result<(), Error> {
        self.private_vc.insert(
            channel_id,
            Some(Arc::new(PrivateVc {
                owner_id,
                message_id,
                trusted_users: trusted_users.clone(),
                allowlist_roles: allowlist_roles.clone(),
                allowlist_users: allowlist_users.clone(),
                denylist_users: denylist_users.clone(),
            })),
        );

        // Hydrate channel (get fresh channel info & ID)
        let (channel, _) = self.get_channel(channel_id.widen(), guild_id).await?;

        // Hydrate owner user
        let owner = self.get_user(owner_id).await?;
        let hydrated_owner_id = owner.id;

        let role_ids = allowlist_roles
            .iter()
            .map(|r| r.get() as i64)
            .collect::<Vec<_>>();

        // Hydrate all user lists
        let allowlist_users = self.hydrate_users(allowlist_users).await?;
        let trusted_users = self.hydrate_users(trusted_users).await?;
        let denylist_users = self.hydrate_users(denylist_users).await?;

        // TODO: really need to refactor this.
        sqlx::query!(
            r#"
            DELETE FROM private_vcs WHERE channel_id = $1
            "#,
            channel
        )
        .execute(&self.db)
        .await?;

        // Insert main private_vcs record with hydrated channel_id and owner_id

        let message_id = if let Some(message_id) = message_id {
            self.get_message(message_id, channel_id.widen(), Some(GUILD), bot_id)
                .await
                .ok()
                .map(|m| m.id)
        } else {
            None
        };

        sqlx::query!(
            r#"
            INSERT INTO private_vcs (channel_id, owner_id, allowlist_roles, message_id)
            VALUES ($1, $2, $3, $4)
            "#,
            channel,
            hydrated_owner_id,
            &role_ids,
            message_id
        )
        .execute(&self.db)
        .await?;

        // cleanup

        sqlx::query!(
            r#"
            DELETE FROM private_vc_allowlist_users WHERE channel_id = $1
            "#,
            channel
        )
        .execute(&self.db)
        .await?;

        sqlx::query!(
            r#"
            DELETE FROM private_vc_trusted_users WHERE channel_id = $1
            "#,
            channel
        )
        .execute(&self.db)
        .await?;

        sqlx::query!(
            r#"
            DELETE FROM private_vc_denylist_users WHERE channel_id = $1
            "#,
            channel
        )
        .execute(&self.db)
        .await?;

        // Insert allowlist users using hydrated_channel_id and hydrated user IDs
        for user_id in allowlist_users {
            sqlx::query!(
                r#"
                INSERT INTO private_vc_allowlist_users (channel_id, user_id)
                VALUES ($1, $2)
                "#,
                channel,
                user_id
            )
            .execute(&self.db)
            .await?;
        }

        // Insert trusted users
        for user_id in trusted_users {
            sqlx::query!(
                r#"
            INSERT INTO private_vc_trusted_users (channel_id, user_id)
            VALUES ($1, $2)
            "#,
                channel,
                user_id
            )
            .execute(&self.db)
            .await?;
        }

        // Insert denylist users
        for user_id in denylist_users {
            sqlx::query!(
                r#"
                    INSERT INTO private_vc_denylist_users (channel_id, user_id)
                    VALUES ($1, $2)
                    "#,
                channel,
                user_id,
            )
            .execute(&self.db)
            .await?;
        }

        Ok(())
    }

    pub async fn set_vc_message_id(
        &self,
        channel_id: serenity::ChannelId,
        message_id: MessageId,
        guild_id: GuildId,
        bot_id: UserId,
    ) -> Result<(), Error> {
        let Some(Some(pvc)) = self.private_vc.get(&channel_id) else {
            return Ok(());
        };

        let mut pvc = (*pvc).clone();
        pvc.message_id = Some(message_id);

        self.private_vc.insert(channel_id, Some(Arc::new(pvc)));

        let msg_data = self
            .get_message(message_id, channel_id.widen(), Some(guild_id), bot_id)
            .await?;

        query!(
            "UPDATE private_vcs SET message_id = $1 WHERE channel_id = $2",
            msg_data.id,
            msg_data.channel_id,
        )
        .execute(&self.db)
        .await?;

        Ok(())
    }

    pub async fn delete_private_vc(
        &self,
        channel_id: serenity::ChannelId,
        guild_id: serenity::GuildId,
    ) -> Result<(), Error> {
        self.private_vc.invalidate(&channel_id);

        let channel = self
            .get_channel(channel_id.widen(), Some(guild_id))
            .await?
            .0;

        sqlx::query!(
            r#"
        DELETE FROM private_vcs WHERE channel_id = $1
        "#,
            channel
        )
        .execute(&self.db)
        .await?;

        // Delete from associated user tables
        sqlx::query!(
            r#"
        DELETE FROM private_vc_allowlist_users WHERE channel_id = $1
        "#,
            channel
        )
        .execute(&self.db)
        .await?;

        sqlx::query!(
            r#"
        DELETE FROM private_vc_trusted_users WHERE channel_id = $1
        "#,
            channel
        )
        .execute(&self.db)
        .await?;

        sqlx::query!(
            r#"
        DELETE FROM private_vc_denylist_users WHERE channel_id = $1
        "#,
            channel
        )
        .execute(&self.db)
        .await?;

        Ok(())
    }

    // temporary function to give access to the inner command overwrites while i figure something out.
    // TODO: stop this lamo
    #[must_use]
    pub fn inner_overwrites(&self) -> Vec<Arc<ApplicationUser>> {
        self.users
            .iter()
            .filter(|u| u.is_admin() | u.allowed_admin_commands.is_some())
            .map(|c| c.clone())
            .collect()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MessageData {
    pub id: i64,
    pub channel_id: i32,
    pub guild_id: Option<i32>,
    pub user_id: i32,
}
