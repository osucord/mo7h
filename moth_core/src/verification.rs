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
use rosu_v2::{
    Osu,
    prelude::{GameMode, RankStatus, UserExtended},
    request::MapType,
};
use serenity::{
    all::{
        CreateEmbed, CreateEmbedAuthor, CreateMessage, EditMember, GenericChannelId, GuildId,
        RoleId, UserId,
    },
    futures::StreamExt,
};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio_util::time::{DelayQueue, delay_queue::Key};
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

    pub async fn verify(&self, user_id: UserId, osu_id: u32, gamemode: GameMode) {
        let lock = self.sender.lock().await;

        lock.as_ref()
            .map(|s| s.send(VerificationCommand::Link((user_id, osu_id, gamemode))));
    }

    pub async fn unverify(&self, user_id: UserId, osu_id: u32) {
        let lock = self.sender.lock().await;

        lock.as_ref()
            .map(|s| s.send(VerificationCommand::Unlink((user_id, osu_id))));
    }

    pub async fn gamemode_change(&self, user_id: UserId, gamemode: GameMode) {
        let lock = self.sender.lock().await;

        lock.as_ref()
            .map(|s| s.send(VerificationCommand::GameModeChange(user_id, gamemode)));
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
    Link((serenity::all::UserId, u32, GameMode)),
    Unlink((serenity::all::UserId, u32)),
    // changes gamemode, assigns new metadata and recalcs.
    GameModeChange(serenity::all::UserId, GameMode),
    Shutdown,
}

pub struct Metadata {
    key: Key,
    osu_id: u32,
    gamemode: GameMode,
    rank: Option<u32>,
    initial_verification: bool,
}

#[expect(clippy::too_many_lines)]
pub async fn task(
    ctx: serenity::all::Context,
    mut rx: mpsc::UnboundedReceiver<VerificationCommand>,
) {
    let data = ctx.data::<Data>();
    let mut delay_queue = DelayQueue::new();
    let mut keys = HashMap::new();
    let mut empty_fill_instant = std::time::Instant::now();
    let mut interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            Some(cmd) = rx.recv() => {
                match cmd {
                    VerificationCommand::Link((u, o, mode)) => {
                        let key = delay_queue.insert(u, Duration::from_secs(86400));
                        keys.insert(u, Metadata {
                            key,
                            osu_id: o,
                            gamemode: mode,
                            rank: None,
                            initial_verification: true,
                        });
                    },
                    VerificationCommand::Unlink((u, _)) => {
                        if let Some(metadata) = keys.remove(&u) {
                            delay_queue.remove(&metadata.key);
                        }
                    },
                    VerificationCommand::GameModeChange(u, mode) => {
                        if let Some(metadata) = keys.get_mut(&u) {
                            metadata.initial_verification = true;
                            metadata.gamemode = mode;
                            metadata.rank = None;
                            delay_queue.reset_at(&metadata.key, tokio::time::Instant::now());
                        } else {
                            let Ok(user) = sqlx::query!(
                                r#"SELECT user_id, osu_id, last_updated, rank
                                 FROM verified_users
                                 WHERE is_active = TRUE AND user_id = $1
                                 "#,
                                u.get() as i64
                            )
                            .fetch_one(&data.database.db)
                            .await else {
                                continue;
                            };


                            let key = delay_queue.insert(u, Duration::from_secs(2));
                            keys.insert(u, Metadata {
                                key,
                                osu_id: user.osu_id as u32,
                                gamemode: mode,
                                rank: user.rank.map(|r| r as u32),
                                initial_verification: true
                            });
                        }

                    }
                    VerificationCommand::Shutdown => break,
                }
            },
            Some(expired) = delay_queue.next() => {
                let u = expired.into_inner();
                if let Some(metadata) = keys.remove(&u) {
                    let osu = &data.web.osu;
                    let (valid, rank) = match osu.user(metadata.osu_id).mode(metadata.gamemode).await {
                        Ok(osu_user) => {
                            let rank = osu_user.statistics.as_ref().expect("always sent").global_rank;
                            // do not update rank if its not a new role.
                            // but always if its "initial" or a gamemode switch.
                            if metadata.initial_verification
                                || get_role_id_for_rank_opt(metadata.gamemode, metadata.rank) != get_role_id_for_rank_opt(metadata.gamemode, rank)
                            {
                                (maybe_update(&ctx, u, Some(&osu_user), Some(metadata.gamemode)).await, Some(rank))
                            } else {
                                (true, Some(rank))
                            }
                        }
                        Err(e) => {
                            dbg!(e);
                            (false, None)
                        }
                    };

                    let mentions = serenity::all::CreateAllowedMentions::new()
                    .all_users(false)
                    .everyone(false)
                    .all_roles(false);

                    if valid {
                        // 1 day
                        let time = Utc::now().timestamp();
                        let _ = data.database.update_last_updated(u, time, rank).await;
                    } else {
                        let _ = LOG_CHANNEL.send_message(&ctx.http, CreateMessage::new().content(format!("❌ Could not update <@{u}>'s roles due to error: (https://osu.ppy.sh/users/{})", metadata.osu_id)).allowed_mentions(mentions)).await;
                        let _ = data.database.inactive_user(u).await;
                        // i should figure out if its a member failure or a restricted failure.
                    }

                }
            },
            _ = interval.tick() => {
                if delay_queue.is_empty() && empty_fill_instant.elapsed() > Duration::from_secs(30) {
                    let Ok(users) = sqlx::query!(
                        r#"
                            SELECT user_id, osu_id, last_updated, rank, gamemode
                            FROM verified_users
                            WHERE is_active = TRUE
                            ORDER BY last_updated ASC
                            LIMIT 100
                        "#
                    )
                    .fetch_all(&data.database.db)
                    .await else {
                        return;
                    };

                    for user in users {
                        let last_updated_time = Utc.timestamp_opt(user.last_updated, 0);
                        let target_time = last_updated_time.latest().unwrap() + chrono::Duration::days(1);

                        let now = Utc::now();
                        let duration = target_time.signed_duration_since(now);
                        let seconds = duration.num_seconds();

                        let key = delay_queue.insert(
                            (user.user_id as u64).into(),
                            Duration::from_secs(seconds.try_into().unwrap_or(0)),
                        );
                        keys.insert((user.user_id as u64).into(), Metadata {
                            key,
                            osu_id: user.osu_id as u32,
                            gamemode: (user.gamemode as u8).into(),
                            rank: user.rank.map(|r| r as u32),
                            initial_verification: false,
                        });
                    }

                    empty_fill_instant = Instant::now();
                }
            }
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

const ALL_RANGES: [&[RoleRange]; 4] = [OSU_RANGES, MANIA_RANGES, TAIKO_RANGES, CTB_RANGES];

fn get_role_id_for_rank(game_mode: GameMode, rank: u32) -> RoleId {
    match game_mode {
        GameMode::Osu => find_role_for_rank(OSU_RANGES, rank),
        GameMode::Mania => find_role_for_rank(MANIA_RANGES, rank),
        GameMode::Taiko => find_role_for_rank(TAIKO_RANGES, rank),
        GameMode::Catch => find_role_for_rank(CTB_RANGES, rank),
    }
}

fn get_role_id_for_rank_opt(game_mode: GameMode, rank: Option<u32>) -> Option<RoleId> {
    Some(get_role_id_for_rank(game_mode, rank?))
}

fn find_role_for_rank(ranges: &[RoleRange], rank: u32) -> RoleId {
    // Simple linear search, which should be fine since the data is small
    ranges
        .iter()
        .find(|range| rank >= range.min_rank && rank <= range.max_rank)
        .map(|range| range.role_id)
        .expect("All ranges have a u32::MAX")
}

const SPECIAL_MAPPING: &[(u32, RoleId)] = &[
    // GMT
    (4, RoleId::new(974674488803340338)),
    // PROJECT LOVED
    (31, RoleId::new(969880026084429824)),
    // FEATURED ARTIST
    (35, RoleId::new(901768871038570546)),
    // NOMINATION ASSESSMENT TEAM
    (7, RoleId::new(1069665975630315611)),
    // BEATMAP SPOTLIGHT CURATOR
    (48, RoleId::new(1089591328985329716)),
    // BEATMAP NOMINATOR
    (28, RoleId::new(901772287445987348)),
    // ALM is 16, but not yet supported as not needed.
];

#[expect(clippy::type_complexity)]
const ALL_MAPPER_ROLES: &[(fn(&UserMapHolder) -> bool, RoleId)] = &[
    // Ranked roles
    (|u| u.ranked_std(), RoleId::new(1041039012179222660)),
    (|u| u.ranked_mania(), RoleId::new(1041036116482080811)),
    (|u| u.ranked_taiko(), RoleId::new(1041036580770562149)),
    (|u| u.ranked_catch(), RoleId::new(1041036816909881404)),
    // Loved roles
    (|u| u.loved_std(), RoleId::new(1056525314303475752)),
    (|u| u.loved_mania(), RoleId::new(1120351610107858985)),
    (|u| u.loved_taiko(), RoleId::new(1120351662075289641)),
    (|u| u.loved_catch(), RoleId::new(1120351771634712646)),
];

#[bool_to_bitflags::bool_to_bitflags]
#[derive(Default)]
struct UserMapHolder {
    // DO NOT change the order of these.
    // well, its not a big deal but the bot will recalc everyone due to different bits.
    ranked_std: bool,
    ranked_mania: bool,
    ranked_taiko: bool,
    ranked_catch: bool,
    loved_std: bool,
    loved_mania: bool,
    loved_taiko: bool,
    loved_catch: bool,
}

impl UserMapHolder {
    /// Returns the set of **all possible** mapper roles this struct knows about
    pub fn all_roles() -> impl Iterator<Item = RoleId> {
        ALL_MAPPER_ROLES.iter().map(|(_, role)| *role)
    }

    /// Returns the set of roles this user **should** have based on the flags
    pub fn active_roles(&self) -> impl Iterator<Item = RoleId> + '_ {
        ALL_MAPPER_ROLES
            .iter()
            .filter_map(move |(check, role)| if check(self) { Some(*role) } else { None })
    }
}

pub async fn update_roles(
    ctx: &serenity::all::Context,
    user_id: UserId,
    user: Option<&UserExtended>,
    game_mode: Option<GameMode>,
    reason: &str,
) -> bool {
    // unlink -> remove everything.
    let Some(user) = user else {
        kill_roles(ctx, user_id).await;
        return true;
    };

    let groups = user.groups.as_deref().expect("always sent");

    // we do the osu map checking up here instead of below to minimise the amount of time we are using "outdated" roles.
    // if we wait too long the chances of say, chirou muting them or a mute expiring increases.
    // if we do it here we will only wait a couple micros at most.
    let mut holder = UserMapHolder::default();
    if user.ranked_mapset_count.expect("always sent") > 0 {
        handle_maps(ctx, user.user_id, MapTypeChoice::Ranked, &mut holder).await;
    }
    if user.loved_mapset_count.expect("always sent") > 0 {
        handle_maps(ctx, user.user_id, MapTypeChoice::Loved, &mut holder).await;
    }
    if user.guest_mapset_count.expect("always sent") > 0 {
        handle_maps(ctx, user.user_id, MapTypeChoice::GuestEither, &mut holder).await;
    }

    let Ok(member) = ctx.http.get_member(GUILD_ID, user_id).await else {
        println!("could not fetch member, failing...");
        return false;
    };

    let mut roles = member.roles.to_vec();

    // remove existing rank roles from the users roles.
    roles.retain(|role_id| {
        !ALL_RANGES
            .iter()
            .flat_map(|slice| slice.iter())
            .any(|range| *role_id == range.role_id)
    });

    roles.retain(|role_id| {
        // Remove role if it's in ALL_MAPPER_ROLES
        !UserMapHolder::all_roles().any(|r| r == *role_id)
    });

    // assign the special roles they should have.
    let mut new_special = Vec::new();
    for group in groups {
        if let Some((_, role_id)) = SPECIAL_MAPPING.iter().find(|g| g.0 == group.id).copied() {
            // we use this to notify if we assigned a new special role.
            if !member.roles.contains(&role_id) {
                new_special.push(role_id);
            }
            roles.push(role_id);
        }
    }

    for active_role in holder.active_roles() {
        if !member.roles.contains(&active_role) {
            new_special.push(active_role);
        }
        roles.push(active_role);
    }

    // Conditionally add the new role (only for update, not remove)
    if let Some(rank) = user.statistics.as_ref().expect("always sent").global_rank {
        roles.push(dbg!(get_role_id_for_rank(
            game_mode.unwrap_or_default(),
            rank
        )));
    }

    if *roles == *member.roles {
        return true;
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
        println!("failed to edit member...");
        return false;
    }

    let embed = CreateEmbed::new()
        .author(
            CreateEmbedAuthor::new(user.username.as_str())
                .url(format!("https://osu.ppy.sh/u/{}", user.user_id)),
        )
        .description("Assigned one or more roles to this user that may be considered special.")
        .field("Discord user", format!("<@{user_id}>"), true)
        .thumbnail(&user.avatar_url);

    for role in new_special {
        let embed = embed.clone().field("Role", format!("<@&{role}>"), true);

        let _ = LOG_CHANNEL
            .send_message(
                &ctx.http,
                CreateMessage::new()
                    .content("<@101090238067113984> <@291089948709486593> <@158567567487795200>")
                    .embed(embed),
            )
            .await;
    }

    true
}

enum MapTypeChoice {
    Loved,
    Ranked,
    GuestEither,
}

impl From<MapTypeChoice> for MapType {
    fn from(val: MapTypeChoice) -> Self {
        match val {
            MapTypeChoice::Loved => MapType::Loved,
            MapTypeChoice::Ranked => MapType::Ranked,
            MapTypeChoice::GuestEither => MapType::Guest,
        }
    }
}

async fn handle_maps(
    ctx: &serenity::all::Context,
    user_id: u32,
    map_type: MapTypeChoice,
    holder: &mut UserMapHolder,
) {
    let osu = &ctx.data::<Data>().web.osu;

    let Ok(mapsets) = osu.user_beatmapsets(user_id).status(&map_type.into()).await else {
        return;
    };

    for mapset in mapsets {
        for map in mapset.maps.expect("always sent") {
            if map.creator_id == user_id {
                match map.status {
                    RankStatus::Ranked | RankStatus::Approved => match map.mode {
                        GameMode::Osu => holder.set_ranked_std(true),
                        GameMode::Taiko => holder.set_ranked_taiko(true),
                        GameMode::Catch => holder.set_ranked_catch(true),
                        GameMode::Mania => holder.set_ranked_mania(true),
                    },
                    RankStatus::Loved => match map.mode {
                        GameMode::Osu => holder.set_loved_std(true),
                        GameMode::Taiko => holder.set_loved_taiko(true),
                        GameMode::Catch => holder.set_loved_catch(true),
                        GameMode::Mania => holder.set_loved_mania(true),
                    },
                    _ => {}
                }
            }
        }
    }
}

async fn kill_roles(ctx: &serenity::all::Context, user_id: UserId) {
    let Ok(member) = ctx.http.get_member(GUILD_ID, user_id).await else {
        return;
    };

    let mut roles = member.roles.to_vec();

    // remove existing rank roles from the users roles.
    roles.retain(|role_id| {
        !ALL_RANGES
            .iter()
            .flat_map(|slice| slice.iter())
            .any(|range| *role_id == range.role_id)
    });

    roles.retain(|role_id| {
        // Check if the role is a known special role
        !SPECIAL_MAPPING.iter().any(|(_, r)| r == role_id)
    });
}

async fn maybe_update(
    ctx: &serenity::all::Context,
    user_id: UserId,
    user: Option<&UserExtended>,
    gamemode: Option<GameMode>,
) -> bool {
    update_roles(
        ctx,
        user_id,
        user,
        gamemode,
        "Roles adjusted due to osu! rank update.",
    )
    .await
}

pub async fn remove(
    ctx: &serenity::all::Context,
    user_id: UserId,
    user: Option<&UserExtended>,
) -> bool {
    update_roles(ctx, user_id, user, None, "User has unverified.").await
}
