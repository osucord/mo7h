use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::data::structs::Data;
use axum::{
    Router,
    extract::{Query, State, rejection::QueryRejection},
    http::StatusCode,
    response::Html,
    routing::get,
};
use chrono::{TimeZone, Utc};
use rosu_v2::{Osu, prelude::GameMode};
use serenity::{
    all::{CreateMessage, EditMember, GenericChannelId, GuildId, RoleId, UserId},
    futures::StreamExt,
};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio_util::time::DelayQueue;
use tower_http::cors::CorsLayer;

#[derive(serde::Deserialize)]
struct Params {
    state: u8,
    code: String,
}

pub async fn run(data: Arc<Data>) {
    let cors = CorsLayer::permissive();

    let app = Router::new()
        .route("/", get(auth_osu))
        .with_state(data)
        .layer(cors);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();

    axum::serve(listener, app).await.unwrap();
}

#[derive(serde::Serialize)]
struct IndexContext<'a> {
    success: bool,
    user: Option<&'a str>,
}

async fn auth_osu(
    query: Result<Query<Params>, QueryRejection>,
    State(state): State<Arc<Data>>,
) -> Result<(StatusCode, Html<String>), StatusCode> {
    if let Ok(page) = auth(query, &state).await {
        return Ok((StatusCode::OK, Html(page)));
    }

    let context = IndexContext {
        success: false,
        user: None,
    };

    let page = state
        .web
        .handlebars
        .render("index", &context)
        .expect("Failed to render template");

    Ok((StatusCode::OK, Html(page)))
}

async fn auth(query: Result<Query<Params>, QueryRejection>, data: &Data) -> Result<String, ()> {
    let Query(params) = query.map_err(|_| ())?;

    let osu = Osu::builder()
        .client_id(data.web.osu_client_id)
        .client_secret(data.web.osu_client_secret.expose_secret())
        .with_authorization(
            params.code,
            "https://verify.osucord.moe",
            rosu_v2::prelude::Scopes::Identify,
        )
        .build()
        .await
        .map_err(|_| ())?;

    let user = osu.own_data().await.map_err(|_| ())?;

    let context = IndexContext {
        success: true,
        user: Some(&user.username),
    };

    let page = data
        .web
        .handlebars
        .render("index", &context)
        .map_err(|_| ())?;

    data.web.auth_standby.process_osu(user, params.state);

    Ok(page)
}

#[derive(Default, Clone)]
pub struct VerificationSender {
    sender: Arc<tokio::sync::Mutex<Option<UnboundedSender<VerificationCommand>>>>,
}

impl VerificationSender {
    pub async fn shutdown(&self) {
        let lock = self.sender.lock().await;

        lock.as_ref().map(|s| s.send(VerificationCommand::Shutdown));
    }

    /// Sets the sender to the provided `UnboundedSender`.
    pub async fn set(&self, tx: UnboundedSender<VerificationCommand>) {
        *self.sender.lock().await = Some(tx);
    }

    #[must_use]
    pub fn new(tx: Option<UnboundedSender<VerificationCommand>>) -> Self {
        Self {
            sender: Arc::new(tokio::sync::Mutex::new(tx)),
        }
    }
}

pub enum VerificationCommand {
    Link((serenity::all::UserId, u32)),
    Unlink((serenity::all::UserId, u32)),
    Shutdown,
}

pub async fn task(
    ctx: serenity::all::Context,
    mut rx: mpsc::UnboundedReceiver<VerificationCommand>,
) {
    let data = ctx.data::<Data>();
    let mut delay_queue = DelayQueue::new();
    let mut keys = HashMap::new();
    let mut empty_fill_instant = std::time::Instant::now();

    loop {
        tokio::select! {
            Some(cmd) = rx.recv() => {
                match cmd {
                    VerificationCommand::Link((u, o)) => {
                        let key = delay_queue.insert(u, Duration::from_secs(86400));
                        keys.insert(u, (key, o));
                    },
                    VerificationCommand::Unlink((u, _)) => {
                        if let Some((key, _)) = keys.remove(&u) {
                            delay_queue.remove(&key);
                        }
                    },
                    VerificationCommand::Shutdown => break,
                }
            },
            Some(expired) = delay_queue.next() => {
                let u = expired.into_inner();
                if let Some((_, o)) = keys.remove(&u) {
                    let osu = &data.web.osu;
                    let Ok(gamemode) = data.database.get_gamemode(u, o).await else {
                        println!("checking user that is not in the database, delayqueue is outdated.");
                        continue;
                    };


                    let (valid, user_name) = match osu.user(o).mode(GameMode::Osu).await {
                        Ok(osu_user) => {
                            let rank = osu_user.statistics.expect("always sent").global_rank.expect("always sent");
                            let success = maybe_update(&ctx, u, gamemode, rank).await;
                            (success, Some(osu_user.username))

                        }
                        Err(_) => {
                            (false, None)
                        }
                    };

                    let mentions = serenity::all::CreateAllowedMentions::new()
                    .all_users(false)
                    .everyone(false)
                    .all_roles(false);

                    if valid {
                        let _ = LOG_CHANNEL.send_message(&ctx.http, CreateMessage::new().content(format!("✅ <@{u}> verify loop completed for {} (osu ID: {o})",
                        user_name.unwrap())).allowed_mentions(mentions)).await;
                    } else {
                        let _ = LOG_CHANNEL.send_message(&ctx.http, CreateMessage::new().content(format!("❌ Could not update <@{u}>'s roles due to error: (https://osu.ppy.sh/users/{o})")).allowed_mentions(mentions)).await;
                    }

                }
            }
            else => {
                if delay_queue.is_empty()
                    && empty_fill_instant.elapsed() > Duration::from_secs(60) {
                        let Ok(users) = sqlx::query!(
                            r#"
                            SELECT user_id, osu_id, last_updated
                            FROM verified_users
                            WHERE is_active = TRUE
                            ORDER BY last_updated ASC
                            LIMIT 100
                            "#
                        )
                        .fetch_all(&data.database.db)
                        .await else {
                            continue;
                        };

                        for user in users {
                            let last_updated_time = Utc.timestamp_opt(user.last_updated, 0);
                            let target_time = last_updated_time.latest().unwrap() + chrono::Duration::days(1);

                            let now = Utc::now();
                            let duration = target_time.signed_duration_since(now);
                            let seconds = duration.num_seconds();

                            let key = delay_queue.insert((user.user_id as u64).into(), Duration::from_secs(seconds.try_into().unwrap_or(0)));
                            keys.insert((user.user_id as u64).into(), (key, user.osu_id as u32));
                        }

                        empty_fill_instant = Instant::now();
                    }
            }

            // need a case for when the delayqueue is empty, just like... fetch 10 users?
        }
    }
}

const GUILD_ID: GuildId = GuildId::new(98226572468690944);
pub const LOG_CHANNEL: GenericChannelId = GenericChannelId::new(776522946872344586);

struct RoleRange {
    min_rank: u32,
    max_rank: u32,
    role_id: RoleId,
}

#[rustfmt::skip]
const OSU_RANGES: &[RoleRange] = &[
    RoleRange { min_rank: 1, max_rank: 99, role_id: RoleId::new(754085973003993119) },
    RoleRange { min_rank: 100, max_rank: 499, role_id: RoleId::new(754086188025118770) },
    RoleRange { min_rank: 500, max_rank: 999, role_id: RoleId::new(754086290785304627) },
    RoleRange { min_rank: 1000, max_rank: 4999, role_id: RoleId::new(754086299681685696) },
    RoleRange { min_rank: 5000, max_rank: 9999, role_id: RoleId::new(869294796404035675) },
    RoleRange { min_rank: 10000, max_rank: 24999, role_id: RoleId::new(869295190601531462) },
    RoleRange { min_rank: 25000, max_rank: 49999, role_id: RoleId::new(869295555489202217) },
    RoleRange { min_rank: 50000, max_rank: 99999, role_id: RoleId::new(754086107456471062) },
    RoleRange { min_rank: 100000, max_rank: 499999, role_id: RoleId::new(754089529287245855) },
    RoleRange { min_rank: 500000, max_rank: u32::MAX, role_id: RoleId::new(869295874306605066) },
];

#[rustfmt::skip]
const MANIA_RANGES: &[RoleRange] = &[
    RoleRange { min_rank: 1, max_rank: 99, role_id: RoleId::new(754086656889585714) },
    RoleRange { min_rank: 100, max_rank: 499, role_id: RoleId::new(754086784484376596) },
    RoleRange { min_rank: 500, max_rank: 999, role_id: RoleId::new(754086852524507246) },
    RoleRange { min_rank: 1000, max_rank: 4999, role_id: RoleId::new(754086905825460265) },
    RoleRange { min_rank: 5000, max_rank: 9999, role_id: RoleId::new(754086720638681109) },
    RoleRange { min_rank: 10000, max_rank: 24999, role_id: RoleId::new(754089662242357289) },
    RoleRange { min_rank: 25000, max_rank: 49999, role_id: RoleId::new(869296510909689896) },
    RoleRange { min_rank: 50000, max_rank: 99999, role_id: RoleId::new(869296562881302528) },
    RoleRange { min_rank: 100000, max_rank: 499999, role_id: RoleId::new(869296602869801070) },
    RoleRange { min_rank: 500000, max_rank: u32::MAX, role_id: RoleId::new(869296657882300446) },
];

#[rustfmt::skip]
const TAIKO_RANGES: &[RoleRange] = &[
    RoleRange { min_rank: 1, max_rank: 99, role_id: RoleId::new(754087013904547930) },
    RoleRange { min_rank: 100, max_rank: 499, role_id: RoleId::new(754087748209475595) },
    RoleRange { min_rank: 500, max_rank: 999, role_id: RoleId::new(754087814106448012) },
    RoleRange { min_rank: 1000, max_rank: 4999, role_id: RoleId::new(754087911066173460) },
    RoleRange { min_rank: 5000, max_rank: 9999, role_id: RoleId::new(754087679003721790) },
    RoleRange { min_rank: 10000, max_rank: 24999, role_id: RoleId::new(754089750717136906) },
    RoleRange { min_rank: 25000, max_rank: 49999, role_id: RoleId::new(869297047050784870) },
    RoleRange { min_rank: 50000, max_rank: 99999, role_id: RoleId::new(869297101086011483 )},
    RoleRange { min_rank: 100000, max_rank: 499999, role_id: RoleId::new(869297132958531584) },
    RoleRange { min_rank: 500000, max_rank: u32::MAX, role_id: RoleId::new(869297154253017108) },
];

#[rustfmt::skip]
const CTB_RANGES: &[RoleRange] = &[
    RoleRange { min_rank: 1, max_rank: 99, role_id: RoleId::new(754087989717762080) },
    RoleRange { min_rank: 100, max_rank: 499, role_id: RoleId::new(754088203534729276) },
    RoleRange { min_rank: 500, max_rank: 999, role_id: RoleId::new(754088281674743858) },
    RoleRange { min_rank: 1000, max_rank: 4999, role_id: RoleId::new(754088358916915241) },
    RoleRange { min_rank: 5000, max_rank: 9999, role_id: RoleId::new(754088053101953034) },
    RoleRange { min_rank: 10000, max_rank: 24999, role_id: RoleId::new(754089875157942435) },
    RoleRange { min_rank: 25000, max_rank: 49999, role_id: RoleId::new(869299174556987403) },
    RoleRange { min_rank: 50000, max_rank: 99999, role_id: RoleId::new(869299210883850280) },
    RoleRange { min_rank: 100000, max_rank: 499999, role_id: RoleId::new(869299235592478770) },
    RoleRange { min_rank: 500000, max_rank: u32::MAX, role_id: RoleId::new(869299254076792892) },
];

fn get_role_id_for_rank(game_mode: GameMode, rank: u32) -> RoleId {
    match game_mode {
        GameMode::Osu => find_role_for_rank(OSU_RANGES, rank),
        GameMode::Mania => find_role_for_rank(MANIA_RANGES, rank),
        GameMode::Taiko => find_role_for_rank(TAIKO_RANGES, rank),
        GameMode::Catch => find_role_for_rank(CTB_RANGES, rank),
    }
}

fn find_role_for_rank(ranges: &[RoleRange], rank: u32) -> RoleId {
    // Simple linear search, which should be fine since the data is small
    ranges
        .iter()
        .find(|range| rank >= range.min_rank && rank <= range.max_rank)
        .map(|range| range.role_id)
        .expect("All ranges have a u32::MAX")
}

pub async fn update_roles(
    ctx: &serenity::all::Context,
    user_id: UserId,
    gamemode: Option<GameMode>,
    rank: Option<u32>,
    reason: &str,
) -> bool {
    let Ok(member) = ctx.http.get_member(GUILD_ID, user_id).await else {
        return false;
    };

    let mut roles = member.roles.to_vec();

    let all_ranges = OSU_RANGES
        .iter()
        .chain(MANIA_RANGES)
        .chain(TAIKO_RANGES)
        .chain(CTB_RANGES);

    // remove existing rank roles from the users roles.
    roles.retain(|role_id| !all_ranges.clone().any(|range| *role_id == range.role_id));

    // Conditionally add the new role (only for update, not remove)
    if let (Some(gamemode), Some(rank)) = (gamemode, rank) {
        roles.push(get_role_id_for_rank(gamemode, rank));
    }

    if GUILD_ID
        .edit_member(
            &ctx.http,
            user_id,
            EditMember::new().roles(roles).audit_log_reason(reason),
        )
        .await
        .is_err()
    {
        return false;
    }

    true
}

async fn maybe_update(
    ctx: &serenity::all::Context,
    user_id: UserId,
    gamemode: GameMode,
    rank: u32,
) -> bool {
    update_roles(
        ctx,
        user_id,
        Some(gamemode),
        Some(rank),
        "Roles adjusted due to osu! rank update.",
    )
    .await
}

pub async fn remove(ctx: &serenity::all::Context, user_id: UserId) -> bool {
    update_roles(ctx, user_id, None, None, "User has unverified.").await
}
