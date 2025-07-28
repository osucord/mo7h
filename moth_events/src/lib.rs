#![warn(clippy::pedantic)]
// clippy warns for u64 -> i64 conversions despite this being totally okay in this scenario.
#![allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::wildcard_imports,
    clippy::module_name_repetitions,
    clippy::too_many_lines,
    clippy::unreadable_literal,
    clippy::unused_async, // fix.
)]

use lumi::serenity_prelude::{self as serenity, FullEvent};
use moth_core::data::structs::{Data, Error};

pub mod helper;

pub mod handlers;
use handlers::*;

pub struct Handler;

#[serenity::async_trait]
impl serenity::EventHandler for Handler {
    async fn dispatch(&self, ctx: &serenity::Context, event: &serenity::FullEvent) {
        let _ = event_handler(ctx, event).await;
    }
}

pub async fn event_handler(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
) -> Result<(), Error> {
    let data = ctx.data::<Data>();

    match event {
        FullEvent::Message { new_message, .. } => {
            Box::pin(messages::message(ctx, new_message, data)).await?;
        }
        FullEvent::MessageUpdate {
            old_if_available,
            event,
            ..
        } => {
            messages::message_edit(ctx, old_if_available, &event.message, data).await?;
        }
        FullEvent::MessageDelete {
            channel_id,
            deleted_message_id,
            guild_id,
            ..
        } => {
            messages::message_delete(ctx, *channel_id, *deleted_message_id, *guild_id, data)
                .await?;
        }
        FullEvent::ReactionAdd { add_reaction, .. } => {
            reactions::reaction_add(ctx, add_reaction, data).await?;
        }
        FullEvent::ReactionRemove {
            removed_reaction, ..
        } => {
            reactions::reaction_remove(ctx, removed_reaction, data).await?;
        }
        FullEvent::GuildCreate { guild, is_new, .. } => {
            guilds::guild_create(ctx, guild, is_new).await?;
        }
        FullEvent::GuildMemberAddition { new_member, .. } => {
            guilds::guild_member_addition(ctx, data, new_member).await?;
        }
        FullEvent::GuildMemberRemoval { guild_id, user, .. } => {
            guilds::guild_member_removal(ctx, guild_id, user, data).await?;
        }
        FullEvent::GuildAuditLogEntryCreate {
            entry, guild_id, ..
        } => {
            guilds::guild_audit_log_entry_create(ctx, entry, guild_id).await?;
        }
        FullEvent::GuildRoleCreate { new, .. } => {
            guilds::roles::role_create(ctx, new).await?;
        }
        FullEvent::GuildRoleDelete {
            guild_id,
            removed_role_id,
            removed_role_data_if_available,
            ..
        } => {
            guilds::roles::role_delete(
                ctx,
                *guild_id,
                *removed_role_id,
                removed_role_data_if_available.as_ref(),
            )
            .await?;
        }
        FullEvent::GuildRoleUpdate {
            old_data_if_available,
            new,
            ..
        } => {
            guilds::roles::role_update(ctx, old_data_if_available.as_ref(), new).await?;
        }
        FullEvent::ChannelCreate { channel, .. } => {
            channels::channel_create(ctx, data, channel).await?;
        }
        FullEvent::ChannelDelete { channel, .. } => {
            channels::channel_delete(ctx, data, channel).await?;
        }
        FullEvent::ChannelUpdate { old, new, .. } => {
            channels::channel_update(ctx, data, old, new).await?;
        }
        FullEvent::ThreadCreate {
            thread,
            newly_created,
            ..
        } => {
            channels::thread_create(ctx, data, thread, newly_created).await?;
        }
        FullEvent::ThreadDelete {
            thread,
            full_thread_data,
            ..
        } => {
            channels::thread_delete(ctx, data, thread, full_thread_data).await?;
        }
        FullEvent::ThreadUpdate { old, new, .. } => {
            channels::thread_update(ctx, data, old, new).await?;
        }
        FullEvent::VoiceChannelStatusUpdate {
            old,
            status,
            id,
            guild_id,
            ..
        } => {
            let guilds = { data.config.read().vcstatus.guilds.clone() };
            if let Some(guilds) = guilds
                && guilds.contains(guild_id)
            {
                channels::voice_channel_status_update(ctx, old, status, id, guild_id, data).await?;
            }
        }
        FullEvent::VoiceStateUpdate { old, new, .. } => {
            voice::voice_state_update(ctx, old, new).await?;
        }
        FullEvent::GuildMemberUpdate {
            old_if_available,
            new,
            event,
            ..
        } => {
            users::guild_member_update(ctx, old_if_available, new, event, data).await?;
        }
        FullEvent::Ready { data_about_bot, .. } => {
            misc::ready(ctx, data_about_bot, data).await?;
        }
        FullEvent::GuildMembersChunk { chunk, .. } => {
            println!(
                "Chunk recieved containing {} members: {}/{}",
                chunk.members.len(),
                chunk.chunk_index + 1,
                chunk.chunk_count
            );
        }
        FullEvent::InteractionCreate { interaction, .. } => {
            if let Some(component) = interaction.as_message_component() {
                moth_starboard::handle_component(ctx, data, component).await?;
                moth_core::data::database::private_vcs::interactions::handle_interaction(
                    ctx, component,
                )
                .await;
            }
        }
        _ => {}
    }
    Ok(())
}
