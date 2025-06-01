use ::serenity::all::{CreateEmbed, CreateMessage, EditMessage, GenericChannelId};
use chrono::{Duration, Utc};
use std::borrow::Cow;

use crate::{
    helper::{get_guild_name_override, get_user},
    Error,
};
use lumi::serenity_prelude::{self as serenity, VoiceState};
use moth_ansi::{GREEN, RESET};
use moth_core::data::structs::Data;

pub async fn voice_state_update(
    ctx: &serenity::Context,
    old: &Option<VoiceState>,
    new: &VoiceState,
) -> Result<(), Error> {
    if let Some(old) = old {
        if old.channel_id != new.channel_id && new.channel_id.is_some() {
            handle_switch(ctx, old, new).await?;
        } else if new.channel_id.is_none() {
            handle_leave(ctx, old, new).await?;
        }

        if !old.self_video()
            && new.self_video()
            && new.guild_id == Some(serenity::GuildId::new(98226572468690944))
            && ctx.data::<Data>().new_join_vc.contains_key(&new.user_id)
        {
            GenericChannelId::new(158484765136125952)
                .send_message(
                    &ctx.http,
                    CreateMessage::new().content(format!(
                        "<@158567567487795200> <@{}> turned on their camera in <#{}>!",
                        new.user_id,
                        new.channel_id.unwrap_or_default()
                    )),
                )
                .await?;
        }

        // third case where mutes and other changes happen.
    } else {
        handle_joins(ctx, new).await?;
    }

    Ok(())
}

async fn handle_switch(
    ctx: &serenity::Context,
    old: &VoiceState,
    new: &VoiceState,
) -> Result<(), Error> {
    // unwrapping this is probably fine considering i already handle this before?
    // I don't think i've seen a panic here?
    let old_id = old.channel_id.unwrap();

    // Ditto.
    let new_id = new.channel_id.unwrap();

    let user_name = match get_user(ctx, new.guild_id.unwrap(), new.user_id).await {
        Some(user) => user.tag(),
        None => return Ok(()),
    };

    {
        let guild_cache = ctx.cache.guild(new.guild_id.unwrap());
        // will fire real error in the future.

        let Some(guild_cache) = guild_cache else {
            return Ok(());
        };

        let channel_old_name = guild_cache.channels.get(&old_id).map(|c| &c.base.name);
        let channel_new_name = guild_cache.channels.get(&new_id).map(|c| &c.base.name);

        // maybe i should use fixedstring directly?
        let old_name: Cow<str> = if let Some(channel_name) = channel_old_name {
            Cow::Borrowed(channel_name)
        } else {
            Cow::Borrowed("None")
        };

        // ditto
        let new_name: Cow<str> = if let Some(channel_name) = channel_new_name {
            Cow::Borrowed(channel_name)
        } else {
            Cow::Borrowed("None")
        };

        let guild_name = get_guild_name_override(ctx, &ctx.data(), new.guild_id);

        println!(
            "{GREEN}[{guild_name}] {user_name}: {old_name} (ID:{old_id}) -> {new_name} \
             (ID:{new_id}){RESET}"
        );
    }

    maybe_handle(ctx, new).await?;

    Ok(())
}
async fn handle_leave(
    ctx: &serenity::Context,
    old: &VoiceState,
    new: &VoiceState,
) -> Result<(), Error> {
    // There is no new channel ID.
    let channel_id = old.channel_id.unwrap();
    // they are leaving so old should hold the guild_id, see handle_joins for justification.
    let user_name = match get_user(ctx, new.guild_id.unwrap(), new.user_id).await {
        Some(user) => user.tag(),
        None => return Ok(()),
    };

    let guild_cache = ctx.cache.guild(new.guild_id.unwrap());
    // will fire real error in the future.
    let Some(guild_cache) = guild_cache else {
        return Ok(());
    };

    let channel_name = guild_cache
        .channels
        .get(&channel_id)
        .map_or_else(|| "None", |c| c.base.name.as_str());

    let guild_name = get_guild_name_override(ctx, &ctx.data(), new.guild_id);

    println!("{GREEN}[{guild_name}] {user_name} left {channel_name} (ID:{channel_id}){RESET}");
    Ok(())
}
async fn handle_joins(ctx: &serenity::Context, new: &VoiceState) -> Result<(), Error> {
    let channel_id = new.channel_id.unwrap();

    // unwrapping the guild should be fine here unless the discord api is being funky
    // they are joining, so a guild_id is present.
    let user_name = match get_user(ctx, new.guild_id.unwrap(), new.user_id).await {
        Some(user) => user.tag(),
        None => return Ok(()),
    };

    {
        let guild_cache = ctx.cache.guild(new.guild_id.unwrap());
        // will fire real error in the future.

        let Some(guild_cache) = guild_cache else {
            return Ok(());
        };

        let channel = guild_cache.channels.get(&channel_id).unwrap();

        let channel_name = &channel.base.name;

        let guild_name = get_guild_name_override(ctx, &ctx.data(), Some(channel.base.guild_id));

        println!(
            "{GREEN}[{guild_name}] {user_name} joined {channel_name} (ID:{channel_id}){RESET}"
        );
    }

    maybe_handle(ctx, new).await?;

    Ok(())
}

#[expect(clippy::similar_names)]
pub async fn maybe_handle(ctx: &serenity::Context, new: &VoiceState) -> Result<(), Error> {
    let data = ctx.data::<Data>();

    let to_handle = {
        let voice_fuckery = data.new_join_vc.get(&new.user_id);
        if let Some(voice) = voice_fuckery {
            if voice.cleared {
                None
            } else {
                let current_time = Utc::now();
                let timestamp = voice.member.joined_at.unwrap_or_default();
                let time_diff = current_time.signed_duration_since(*timestamp);

                if time_diff <= Duration::hours(1) {
                    Some(voice.clone())
                } else {
                    None
                }
            }
        } else {
            None
        }
    };

    let Some(to_handle) = to_handle else {
        return Ok(());
    };

    let new_message = if let Some(announce) = to_handle.announce_msg {
        let now = Utc::now();
        *announce.created_at() < now - Duration::minutes(30)
    } else {
        true
    };

    let content = format!("<@158567567487795200>: <@{}>", to_handle.member.user.id);

    let embed = CreateEmbed::new()
        .title(to_handle.member.user.name.clone())
        .description("New join joined VC!")
        .field(
            "Joined at",
            format!(
                "<t:{}:R>",
                to_handle.member.joined_at.unwrap_or_default().timestamp()
            ),
            true,
        )
        .field("VC", format!("<#{}>", new.channel_id.unwrap()), true)
        .thumbnail(to_handle.member.user.face());

    if new_message {
        let val = GenericChannelId::new(158484765136125952)
            .send_message(
                &ctx.http,
                CreateMessage::new().content(content).embed(embed),
            )
            .await?;

        if let Some(mut m) = data.new_join_vc.get_mut(&new.user_id) {
            m.announce_msg = Some(val.id);
        };
    } else {
        let _ = GenericChannelId::new(158484765136125952)
            .edit_message(
                &ctx.http,
                to_handle.announce_msg.unwrap(),
                EditMessage::new().embed(embed).content(content),
            )
            .await;
    }

    Ok(())
}
