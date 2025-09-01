use std::{collections::HashSet, sync::Arc};

mod member_roles;
pub(crate) mod roles;

use std::fmt::Write;

use crate::{
    Data, Error,
    helper::{get_channel_name, get_guild_name_override, get_user},
};
use lumi::serenity_prelude::{
    self as serenity, AuditLogEntry, AutoModAction, ChannelId, CreateEmbedAuthor, Guild, GuildId,
    Member, User,
};

use moth_ansi::{RESET, YELLOW};

use ::serenity::all::{CreateMessage, GenericChannelId, UserId};
use moth_core::data::structs::Fuck;
use serenity::model::guild::audit_log::Action;

pub async fn guild_create(
    ctx: &serenity::Context,
    guild: &Guild,
    is_new: &Option<bool>,
) -> Result<(), Error> {
    if let Some(true) = is_new {
        println!(
            "{YELLOW}Joined {} (ID:{})!\nNow in {} guild(s){RESET}",
            guild.name,
            guild.id,
            ctx.cache.guilds().len()
        );
    }
    Ok(())
}

pub async fn guild_member_addition(
    ctx: &serenity::Context,
    data: Arc<Data>,
    new_member: &Member,
) -> Result<(), Error> {
    let guild_id = new_member.guild_id;
    let joined_user_id = new_member.user.id;

    data.new_join_vc.insert(
        new_member.user.id,
        Fuck {
            member: new_member.clone(),
            channels: HashSet::new(),
            cleared: false,
            announce_msg: None,
        },
    );

    let guild_name = get_guild_name_override(ctx, &data, Some(guild_id));

    println!(
        "{YELLOW}[{}] {} (ID:{}) has joined!{RESET}",
        guild_name,
        new_member.user.tag(),
        joined_user_id
    );
    Ok(())
}

pub async fn join_time(ctx: &serenity::Context, data: &Data, user_id: UserId) {
    let mut groups = Vec::new();

    {
        let mut lock = data.osu_game_joins.lock();

        if !lock.contains(&user_id) {
            lock.push_back(user_id);
        }

        while lock.len() > 500 {
            lock.pop_front();
        }

        let mut users_sorted = lock.iter().copied().collect::<Vec<_>>();
        users_sorted.sort_by_key(|uid| uid.created_at().timestamp());

        let timestamps: Vec<_> = users_sorted
            .iter()
            .map(|uid| uid.created_at().timestamp())
            .collect();

        let mut start = 0;
        for end in 0..users_sorted.len() {
            while timestamps[end] - timestamps[start] > 86400 {
                start += 1;
            }

            if end - start + 1 > 1 {
                groups.push(users_sorted[start..=end].to_vec());
            }
        }
    }

    let relevant_groups: Vec<_> = groups
        .into_iter()
        .filter(|group| group.contains(&user_id))
        .collect();

    for group in relevant_groups {
        let mut string = String::new();

        for uid in group {
            write!(string, "<@{uid}> ").unwrap();
        }

        let _ = GenericChannelId::new(158484765136125952)
            .send_message(
                &ctx.http,
                CreateMessage::new().content(format!(
                    "group detected with join date within 1 day of each \
                     other: {string}(possible raid, please watch vcs!)"
                )),
            )
            .await;
    }
}

pub async fn guild_member_removal(
    ctx: &serenity::Context,
    guild_id: &GuildId,
    user: &User,
    data: Arc<Data>,
) -> Result<(), Error> {
    let guild_name = get_guild_name_override(ctx, &data, Some(*guild_id));

    println!(
        "{YELLOW}[{}] {} (ID:{}) has left!{RESET}",
        guild_name,
        user.tag(),
        user.id
    );

    Ok(())
}

pub async fn guild_audit_log_entry_create(
    ctx: &serenity::Context,
    entry: &AuditLogEntry,
    guild_id: &GuildId,
) -> Result<(), Error> {
    member_roles::handle(ctx, entry, *guild_id).await;

    if *guild_id != 98226572468690944 {
        return Ok(());
    }

    if !matches!(entry.action, Action::AutoMod(AutoModAction::FlagToChannel)) {
        return Ok(());
    }

    let Some(reason) = &entry.reason else {
        return Ok(());
    };

    if !reason.starts_with("Voice Channel Status") {
        return Ok(());
    }

    let (user_name, avatar_url) = {
        // TODO: i'm not happy with the unwrap but i'd rather avoid the http request now.
        let user = get_user(ctx, *guild_id, entry.user_id.unwrap())
            .await
            .unwrap();
        (user.tag(), user.face())
    };

    let (check_contents, culprit_channel_id): (Option<u64>, Option<ChannelId>) =
        if let Some(options) = &entry.options {
            (
                match &options.auto_moderation_rule_name {
                    Some(rule_name) => match rule_name.as_str() {
                        "Bad Words ❌ [BLOCKED]" => Some(697738506944118814),
                        _ => None,
                    },
                    None => None,
                },
                options.channel_id.map(GenericChannelId::expect_channel), // culprit.
            )
        } else {
            (None, None)
        };

    // use channel_id instead.
    if let Some(id) = check_contents {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let mut status = format!(
            "Unknown (check #{})",
            get_channel_name(ctx, Some(*guild_id), GenericChannelId::new(id)).await
        )
        .to_string();

        if let Some(msgs) = ctx.cache.channel_messages(id.into()) {
            for msg in msgs
                .iter()
                .rev()
                .filter(|m| m.author.id == entry.user_id.unwrap())
            {
                if let Some(description) = msg
                    .embeds
                    .first()
                    .filter(|e| e.kind.as_deref() == Some("auto_moderation_message"))
                    .and_then(|e| e.description.as_ref())
                {
                    status = description.to_string();
                    break;
                }
            }
        }

        let author_title = format!("{user_name} tried to set an inappropriate status");
        let footer = serenity::CreateEmbedFooter::new(format!(
            "User ID: {} • Please check status manually in #{}",
            entry.user_id.unwrap(),
            get_channel_name(ctx, Some(*guild_id), GenericChannelId::new(id)).await
        ));
        let mut embed = serenity::CreateEmbed::default()
            .author(CreateEmbedAuthor::new(author_title).icon_url(avatar_url))
            .field("Status", status, true)
            .footer(footer);

        if let Some(channel_id) = culprit_channel_id {
            embed = embed.field("Channel", format!("<#{channel_id}>"), true);
        }

        let builder = serenity::CreateMessage::default()
            .embed(embed)
            .content(format!("<@{}>", entry.user_id.unwrap()));
        // this is gg/osu only, so i won't enable configurable stuff for this.
        GenericChannelId::new(158484765136125952)
            .send_message(&ctx.http, builder.clone())
            .await?;
        GenericChannelId::new(1163544192866336808)
            .send_message(&ctx.http, builder)
            .await?;
    }
    Ok(())
}
