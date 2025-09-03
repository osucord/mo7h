use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use aformat::{CapStr, aformat, aformat_into};
use rand::RngCore;

use std::fmt::Write;

use serenity::{
    all::{
        ChannelId, ChannelType, Colour, Context, CreateActionRow, CreateAllowedMentions,
        CreateButton, CreateChannel, CreateComponent, CreateContainer, CreateMessage,
        CreateSelectMenu, CreateSelectMenuKind, CreateSeparator, CreateTextDisplay, EditMessage,
        GuildId, MessageFlags, MessageId, PermissionOverwrite, PermissionOverwriteType,
        Permissions, UserId,
    },
    futures::StreamExt,
    small_fixed_array::FixedString,
};
use tokio_util::time::{DelayQueue, delay_queue::Key};

use crate::data::{
    database::{
        PrivateVc,
        private_vcs::{
            HandlerCommand,
            interactions::{apply_channel_permissions, build_permission_overwrites},
        },
    },
    structs::Data,
};

pub const VC_CHANNEL: ChannelId = ChannelId::new(1399817426723668039);
pub const GUILD: GuildId = GuildId::new(98226572468690944);

// cooldown system setup when?

#[derive(Hash, Eq, PartialEq, Clone, Debug)]
enum QueueType {
    Cooldown((UserId, FixedString<u8>)),
    Leave(ChannelId),
    OwnerLeave((ChannelId, UserId)),
}

pub(super) async fn start(
    ctx: Context,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<HandlerCommand>,
) {
    let mut delay_queue = DelayQueue::new();
    let mut keys = HashMap::new();
    let mut join_times = HashMap::new();

    // Populate on task startup to clearup old channels.
    let data = ctx.data_ref::<Data>();
    if let Ok(private_vcs) = data.database.get_all_private_vcs().await {
        for (id, _) in private_vcs {
            let key = delay_queue.insert(QueueType::Leave(id), Duration::from_secs(5));
            keys.insert(QueueType::Leave(id), key);
        }
    }

    loop {
        tokio::select! {
            Some(cmd) = rx.recv() => {
                // exit the task, we have shutdown
                if !handle_command(cmd, &ctx, &mut delay_queue, &mut keys, &mut join_times).await {
                    break
                }
            },
            Some(expired) = delay_queue.next() => {
                let meta = expired.into_inner();
                keys.remove(&meta);

                handle_expired(&ctx, meta).await;
            }
        }
    }
}

async fn handle_expired(ctx: &Context, queue_type: QueueType) {
    async fn maybe_del_vc(ctx: &Context, id: ChannelId) {
        if id == VC_CHANNEL {
            return; // DO NOT
        }

        // TODO: determine permission failure from not existing
        if !vc_has_people(ctx, id)
            && ctx
                .http
                .delete_channel(id.widen(), Some("Private VC no longer active."))
                .await
                .is_ok()
        {
            let _ = ctx
                .data_ref::<Data>()
                .database
                .delete_private_vc(id, GUILD)
                .await;
        }
    }

    match queue_type {
        QueueType::Cooldown((user_id, username)) => {
            if vc_has_user(ctx, VC_CHANNEL, user_id) {
                create_channel(ctx, user_id, username).await;
            }
        }
        QueueType::Leave(channel_id) => maybe_del_vc(ctx, channel_id).await,
        QueueType::OwnerLeave((channel_id, user_id)) => {
            if !vc_has_user(ctx, channel_id, user_id) {
                let data = ctx.data_ref::<Data>();

                let Some(vc) = data.database.get_private_vc(channel_id, Some(GUILD)).await else {
                    return;
                };

                // in future reschedule
                let users = get_vc_users(ctx, channel_id);
                if users.is_empty() {
                    return;
                }

                let chosen_owner = {
                    let mut rng = rand::rng();
                    let random_index = rng.next_u32() as usize % users.len();
                    users[random_index]
                };

                #[expect(unused_braces)]
                if data
                    .database
                    .create_private_vc(
                        channel_id,
                        vc.message_id,
                        Some(GUILD),
                        chosen_owner,
                        vc.allowlist_roles.clone(),
                        vc.allowlist_users.clone(),
                        vc.trusted_users.clone(),
                        vc.denylist_users.clone(),
                        { ctx.cache.current_user().id },
                    )
                    .await
                    .is_err()
                {
                    return;
                }

                // get new state
                let Some(new_vc) = data.database.get_private_vc(channel_id, Some(GUILD)).await
                else {
                    return;
                };

                // TODO: rerun if they aren't in the VC by this point, its a race condition but very rare.
                message(ctx, channel_id, vc.message_id, &new_vc).await;

                let overwrites = build_permission_overwrites(
                    &new_vc,
                    get_parent_permissions(ctx, &channel_id)
                        .unwrap_or_default()
                        .as_ref(),
                    &mut vec![],
                    &mut vec![],
                );

                apply_channel_permissions(ctx, channel_id, overwrites).await;
            }
        }
    }
}

#[must_use]
pub fn get_parent_permissions(
    ctx: &Context,
    channel_id: &ChannelId,
) -> Option<Vec<PermissionOverwrite>> {
    let guild = ctx.cache.guild(GUILD)?;
    let channel = guild.channels.get(channel_id)?;
    let parent_id = channel.parent_id?;
    let parent_channel = guild.channels.get(&parent_id)?;

    Some(parent_channel.permission_overwrites.to_vec())
}

const COOLDOWN: Duration = Duration::from_secs(30);

async fn handle_command(
    cmd: HandlerCommand,
    ctx: &Context,
    delay_queue: &mut DelayQueue<QueueType>,
    keys: &mut HashMap<QueueType, Key>,
    join_times: &mut HashMap<UserId, Instant>,
) -> bool {
    let data = ctx.data_ref::<Data>();
    let database = &data.database;

    match cmd {
        HandlerCommand::JoinSpecial((channel_id, user_id, username)) => {
            // first we need to figure out the origin, *we know* its either special or a private VC already.
            if let Some(vc) = database.get_private_vc(channel_id, Some(GUILD)).await {
                if vc.owner_id == user_id
                    && let Some(key) = keys.get(&QueueType::OwnerLeave((channel_id, user_id)))
                {
                    delay_queue.remove(key);
                }

                return true;
            }

            // probably isn't possible to reach here given the circumstances, but we very much should check anyway.
            if channel_id != VC_CHANNEL {
                return true;
            }

            // don't create a channel when they already created one in the last 30 seconds.
            if let Some(join_time) = join_times.get(&user_id) {
                let elapsed = join_time.elapsed();

                if elapsed < COOLDOWN {
                    let remaining = COOLDOWN - elapsed;
                    // TODO: technically a bad idea to put the username inside the keymap but later me problem.
                    let value = QueueType::Cooldown((user_id, username));

                    let key = delay_queue.insert(value.clone(), remaining);
                    keys.insert(value, key);

                    return true;
                }
            }

            join_times.insert(user_id, Instant::now());
            create_channel(ctx, user_id, username).await;
        }
        HandlerCommand::LeaveVc((channel_id, user_id)) => {
            // TODO: figure out if owner, then insert right type
            if !vc_has_people(ctx, channel_id) {
                let key = delay_queue.insert(QueueType::Leave(channel_id), COOLDOWN);
                keys.insert(QueueType::Leave(channel_id), key);
            }

            if let Some(vc) = database.get_private_vc(channel_id, Some(GUILD)).await
                && vc.owner_id == user_id
            {
                let key = delay_queue.insert(
                    QueueType::OwnerLeave((channel_id, user_id)),
                    Duration::from_secs(300),
                );
                keys.insert(QueueType::OwnerLeave((channel_id, user_id)), key);
            }
        }
        HandlerCommand::Shutdown => return false,
    }
    // gotta keep running
    true
}

fn get_vc_users(ctx: &Context, channel_id: ChannelId) -> Vec<UserId> {
    let Some(guild) = ctx.cache.guild(GUILD) else {
        return vec![];
    };

    guild
        .voice_states
        .iter()
        .filter(|v| v.channel_id == Some(channel_id))
        .map(|v| v.user_id)
        .collect()
}

fn vc_has_people(ctx: &Context, channel_id: ChannelId) -> bool {
    let Some(guild) = ctx.cache.guild(GUILD) else {
        return true;
    };

    guild
        .voice_states
        .iter()
        .any(|v| v.channel_id == Some(channel_id))
}

pub(super) fn vc_has_user(ctx: &Context, channel_id: ChannelId, user_id: UserId) -> bool {
    let Some(guild) = ctx.cache.guild(GUILD) else {
        return true;
    };

    guild
        .voice_states
        .iter()
        .any(|v| v.channel_id == Some(channel_id) && v.user_id == user_id)
}

async fn create_channel(ctx: &Context, user_id: UserId, username: FixedString<u8>) {
    let data = ctx.data_ref::<Data>();

    let Some((position, mut overwrites, category_id)) = ctx.cache.guild(GUILD).and_then(|g| {
        g.channels.get(&VC_CHANNEL).and_then(|c| {
            let position = c.position;
            let parent_id = c.parent_id?;
            let overwrites = g.channels.get(&parent_id)?.permission_overwrites.clone();
            Some((position, overwrites, parent_id))
        })
    }) else {
        return;
    };

    let Some(overwrite_index) = overwrites
        .iter()
        .position(|o| o.kind == PermissionOverwriteType::Role(GUILD.get().into()))
    else {
        return;
    };

    // allow users to send messages inside the voice channel.
    if let Some(o) = overwrites.get_mut(overwrite_index) {
        o.deny.remove(Permissions::SEND_MESSAGES);
        o.deny.remove(Permissions::SPEAK);
    }

    let mut channel_name = aformat::ArrayString::<43>::new();
    if username.ends_with('s') {
        aformat_into!(channel_name, "👥{}' room", CapStr::<32>(&username));
    } else {
        aformat_into!(channel_name, "👥{}'s room", CapStr::<32>(&username));
    }

    let audit_log_reason = aformat!("{} created a Private VC", CapStr::<32>(&username));

    // maybe i need to do a pos fix at some point? will experiment later.
    let builder = CreateChannel::new(channel_name.as_str())
        .audit_log_reason(audit_log_reason.as_str())
        .permissions(overwrites)
        .category(category_id)
        .kind(ChannelType::Voice)
        .position(position + 1)
        .user_limit(5.into());

    if let Ok(channel) = GUILD.create_channel(&ctx.http, builder).await {
        #[expect(unused_braces)]
        let _ = data
            .database
            .create_private_vc(
                channel.id,
                None,
                Some(GUILD),
                user_id,
                vec![],
                vec![],
                vec![],
                vec![],
                { ctx.cache.current_user().id },
            )
            .await;

        let _ = GUILD.move_member(&ctx.http, user_id, channel.id).await;

        if let Some(msg) = data.database.get_private_vc(channel.id, Some(GUILD)).await {
            message(ctx, channel.id, None, &msg).await;
        }
    }
}

#[expect(clippy::too_many_lines)] // will split out later
pub(super) async fn message(
    ctx: &Context,
    channel_id: ChannelId,
    message_id: Option<MessageId>,
    private_vc: &PrivateVc,
) {
    let mentions = CreateAllowedMentions::new()
        .users(vec![private_vc.owner_id])
        .all_roles(false)
        .everyone(false);

    let main = CreateComponent::TextDisplay(CreateTextDisplay::new(format!(
        "## 📢 Voice Channel Controls\n**Current owner:** <@{}>\n-# ⏱ Owner will transfer after 5 \
         minutes of inactivity.",
        private_vc.owner_id
    )));

    let owner_select_menu = CreateComponent::ActionRow(CreateActionRow::SelectMenu(
        CreateSelectMenu::new(
            "owner_pvc_",
            CreateSelectMenuKind::User {
                default_users: None,
            },
        )
        .min_values(1)
        .max_values(1)
        .placeholder("Select new owner"),
    ));

    let sep = CreateComponent::Separator(CreateSeparator::new(true));

    let general_settings_header = CreateComponent::TextDisplay(CreateTextDisplay::new(
        "🔧 **General Settings**\n-# General settings of the voice channel, mainly for \
         convenience to everyone.\n🎛 **Voice Channel Size**",
    ));

    let voice_channel_size_action_row =
        CreateComponent::ActionRow(CreateActionRow::Buttons(std::borrow::Cow::Borrowed(&[
            CreateButton::new("size_pvc_").label("Change size"),
        ])));

    // let region_section =
    //     CreateComponent::TextDisplay(CreateTextDisplay::new("🌍 **Region**\n**Coming soon**!"));

    let private_text =
        if private_vc.allowlist_roles.is_empty() && private_vc.allowlist_users.is_empty() {
            "(VC is currently public)"
        } else {
            "(VC is currently private)"
        };

    let mut access_control_text = format!(
        "👥 **Access Control**\n-# Selecting users will toggle their access, Moderators cannot be \
         blocked from joining the VC.\n✅ **Allowlist** {private_text}"
    );

    if !private_vc.allowlist_roles.is_empty() {
        let formatted_roles: String = private_vc
            .allowlist_roles
            .iter()
            .map(|role_id| aformat!("<@&{}>", *role_id))
            .collect::<Vec<_>>()
            .join(", ");

        write!(
            access_control_text,
            "\n**Allowed roles:** {formatted_roles}"
        )
        .unwrap();
    }

    if !private_vc.allowlist_users.is_empty() {
        let formatted_users = private_vc
            .allowlist_users
            .iter()
            .map(|user_id| aformat!("<@{}>", *user_id))
            .collect::<Vec<_>>()
            .join(", ");

        write!(
            access_control_text,
            "\n**Allowed users:** {formatted_users}"
        )
        .unwrap();
    }

    let access_control = CreateComponent::TextDisplay(CreateTextDisplay::new(access_control_text));
    let allowlist_select_menu = CreateComponent::ActionRow(CreateActionRow::SelectMenu(
        CreateSelectMenu::new(
            "allowlist_pvc_",
            CreateSelectMenuKind::Mentionable {
                default_users: None,
                default_roles: None,
            },
        )
        .min_values(0)
        .max_values(25)
        .placeholder("Toggle allowed users/roles"),
    ));

    // display allowed users/roles, explain how whitelist works
    let mut denylist_text = String::from("🚫 **Denylist**");

    if !private_vc.denylist_users.is_empty() {
        let formatted_users = private_vc
            .denylist_users
            .iter()
            .map(|user_id| aformat!("<@{}>", *user_id))
            .collect::<Vec<_>>()
            .join(", ");

        write!(denylist_text, "\n**Blocked users:** {formatted_users}").unwrap();
    }
    let denylist = CreateComponent::TextDisplay(CreateTextDisplay::new(denylist_text));
    let denylist_select_menu = CreateComponent::ActionRow(CreateActionRow::SelectMenu(
        CreateSelectMenu::new(
            "denylist_pvc_",
            CreateSelectMenuKind::User {
                default_users: None,
            },
        )
        .min_values(0)
        .max_values(25)
        .placeholder("Toggle denied users"),
    ));

    let disconnect_text = CreateComponent::TextDisplay(CreateTextDisplay::new(
        "🛑 **User Management**\n-# Somebody bothering you? You can remove them if you like!\n❌ \
         **Disconnect Users**",
    ));

    let disconnect_select_menu = CreateComponent::ActionRow(CreateActionRow::SelectMenu(
        CreateSelectMenu::new(
            "disconnect_pvc_",
            CreateSelectMenuKind::User {
                default_users: None,
            },
        )
        .min_values(1)
        .max_values(1)
        .placeholder("Disconnect user"),
    ));

    let components = &[CreateComponent::Container(
        CreateContainer::new(vec![
            main,
            owner_select_menu,
            sep.clone(),
            general_settings_header,
            voice_channel_size_action_row,
            // region_section,
            sep.clone(),
            access_control,
            allowlist_select_menu,
            denylist,
            denylist_select_menu,
            sep,
            disconnect_text,
            disconnect_select_menu,
        ])
        .accent_colour(Colour::BLITZ_BLUE),
    )];

    let send_new_message = async || {
        channel_id
            .widen()
            .send_message(
                &ctx.http,
                CreateMessage::new()
                    .flags(MessageFlags::IS_COMPONENTS_V2)
                    .components(components)
                    .allowed_mentions(mentions.clone()),
            )
            .await
            .map(|m| m.id)
    };

    if let Some(message_id) = message_id {
        let http = &ctx.http;
        let mentions = mentions.clone();

        let blank_edit = channel_id
            .widen()
            .edit_message(
                http,
                message_id,
                EditMessage::new()
                    .components(vec![CreateComponent::TextDisplay(CreateTextDisplay::new(
                        "_ _",
                    ))])
                    .allowed_mentions(mentions.clone()),
            )
            .await;

        let full_edit = channel_id
            .widen()
            .edit_message(
                http,
                message_id,
                EditMessage::new()
                    .components(components)
                    .allowed_mentions(mentions),
            )
            .await;

        if blank_edit.is_err() || full_edit.is_err() {
            let Ok(m_id) = send_new_message().await else {
                return;
            };
            #[expect(unused_braces)] // needed for force copy
            let _ = ctx
                .data_ref::<Data>()
                .database
                .set_vc_message_id(channel_id, m_id, GUILD, { ctx.cache.current_user().id })
                .await;
        }
    } else {
        let Ok(m_id) = send_new_message().await else {
            return;
        };
        #[expect(unused_braces)] // needed for force copy
        let _ = ctx
            .data_ref::<Data>()
            .database
            .set_vc_message_id(channel_id, m_id, GUILD, { ctx.cache.current_user().id })
            .await;
    }
}
