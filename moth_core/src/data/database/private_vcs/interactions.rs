use std::{sync::Arc, time::Duration};

use serenity::all::{
    ChannelId, ComponentInteraction, Context, CreateAllowedMentions, CreateInputText,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateQuickModal, EditChannel,
    InputTextStyle, ModalInteraction, PermissionOverwrite, PermissionOverwriteType, Permissions,
    QuickModal, RoleId, UserId,
};

use crate::data::{
    database::{
        PrivateVc,
        private_vcs::task::{GUILD, get_parent_permissions, message, vc_has_user},
    },
    structs::Data,
};

enum Kind {
    Owner,
    Size,
    Disconnect,
    Allowlist,
    Denylist,
    Region,
}

impl Kind {
    fn from_id(id: &str) -> Option<Self> {
        // Split once at "_pvc_" and get the prefix
        let kind_str = id.split_once("_pvc_")?.0;

        match kind_str {
            "owner" => Some(Kind::Owner),
            "size" => Some(Kind::Size),
            "disconnect" => Some(Kind::Disconnect),
            "allowlist" => Some(Kind::Allowlist),
            "denylist" => Some(Kind::Denylist),
            "region" => Some(Kind::Region),
            _ => None,
        }
    }
}

pub async fn handle_interaction(ctx: &Context, interaction: &ComponentInteraction) {
    let Some(kind) = Kind::from_id(&interaction.data.custom_id) else {
        return;
    };

    let Some(private_vc) = ctx
        .data_ref::<Data>()
        .database
        .private_vc
        .get(&interaction.channel_id.expect_channel())
        .flatten()
    else {
        return;
    };

    if private_vc.message_id != Some(interaction.message.id) {
        return;
    }

    let is_mod = match &interaction.member.as_ref() {
        Some(member) => MOD_ROLES
            .iter()
            .any(|mod_role| member.roles.contains(mod_role)),
        None => false,
    };

    if private_vc.owner_id != interaction.user.id && !is_mod {
        let _ = interaction
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("You need to be the owner of the private vc in order to do this!")
                        .ephemeral(true),
                ),
            )
            .await;

        return;
    }

    match kind {
        Kind::Owner => owner(ctx, interaction, private_vc).await,
        Kind::Size => size(ctx, interaction).await,
        Kind::Allowlist => allow(ctx, interaction, (*private_vc).clone()).await,
        Kind::Denylist => deny(ctx, interaction, (*private_vc).clone()).await,
        Kind::Disconnect => disconnect(ctx, interaction).await,
        Kind::Region => unreachable!(),
    }
}

// really should set permissions here to prevent lockout lmao
async fn owner(ctx: &Context, interaction: &ComponentInteraction, private_vc: Arc<PrivateVc>) {
    let user_id = match &interaction.data.kind {
        serenity::all::ComponentInteractionDataKind::UserSelect { values } => {
            values.first().copied()
        }
        _ => None,
    };

    let Some(user_id) = user_id else {
        return;
    };

    // technically a race condition when setting the owner but its so minimal...
    let user_in_vc =
        super::task::vc_has_user(ctx, interaction.channel_id.expect_channel(), user_id);

    if !user_in_vc {
        let _ = interaction
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("The user is not in the VC and therefore owner cannot be changed.")
                        .ephemeral(true),
                ),
            )
            .await;

        return;
    }

    let data: &Data = ctx.data_ref::<Data>();

    #[expect(unused_braces)]
    if data
        .database
        .create_private_vc(
            interaction.channel_id.expect_channel(),
            private_vc.message_id,
            Some(GUILD),
            user_id,
            private_vc.allowlist_roles.clone(),
            private_vc.allowlist_users.clone(),
            private_vc.trusted_users.clone(),
            private_vc.denylist_users.clone(),
            { ctx.cache.current_user().id },
        )
        .await
        .is_err()
    {
        return;
    }

    // new instance
    let Some(private_vc) = data
        .database
        .get_private_vc(interaction.channel_id.expect_channel(), Some(GUILD))
        .await
    else {
        return;
    };

    message(
        ctx,
        interaction.channel_id.expect_channel(),
        private_vc.message_id,
        &private_vc,
    )
    .await;

    let _ = interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(format!("Changed owner to <@{user_id}>"))
                    .ephemeral(true)
                    .allowed_mentions(CreateAllowedMentions::new()),
            ),
        )
        .await;

    let overwrites = build_permission_overwrites(
        &private_vc,
        get_parent_permissions(ctx, &interaction.channel_id.expect_channel())
            .unwrap_or_default()
            .as_ref(),
        &mut vec![],
        &mut vec![],
    );

    apply_channel_permissions(ctx, interaction.channel_id.expect_channel(), overwrites).await;
}

async fn size(ctx: &Context, interaction: &ComponentInteraction) {
    let Ok(Some(modal)) = interaction
        .quick_modal(
            ctx,
            CreateQuickModal::new("Set VC size")
                .field(CreateInputText::new(InputTextStyle::Short, "Size", "").required(true))
                .timeout(Duration::from_secs(60)),
        )
        .await
    else {
        return;
    };

    let Ok(input) = modal
        .inputs
        .first()
        .expect("Field set as required")
        .parse::<u8>()
    else {
        respond_text(
            ctx,
            &modal.interaction,
            "Input could not be parsed as a number between 0 and 99.",
        )
        .await;
        return;
    };

    if !(0..=99).contains(&input) {
        respond_text(
            ctx,
            &modal.interaction,
            "Input could not be parsed as a number between 0 and 99.",
        )
        .await;
        return;
    }

    // TODO: channel editing should probably be done by a central task?
    let _ = interaction
        .channel_id
        .expect_channel()
        .edit(&ctx.http, EditChannel::new().user_limit(input.into()))
        .await;

    respond_text(
        ctx,
        &modal.interaction,
        aformat::aformat!("Successfully updated user limit to {}", input).as_str(),
    )
    .await;
}

async fn respond_text(ctx: &Context, interaction: &ModalInteraction, msg: &str) {
    let _ = interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(msg)
                    .ephemeral(true),
            ),
        )
        .await;
}

async fn allow(ctx: &Context, interaction: &ComponentInteraction, mut private_vc: PrivateVc) {
    let disallowed_roles_users = {
        let Some(guild) = ctx.cache.guild(GUILD) else {
            return;
        };

        let Some(Some(parent_id)) = guild
            .channels
            .get(&interaction.channel_id.expect_channel())
            .map(|c| c.parent_id)
        else {
            return; // should be unreachable/not designed to work this way.
        };

        let Some(parent) = guild.channels.get(&parent_id) else {
            return;
        };

        parent
            .permission_overwrites
            .iter()
            .cloned()
            .collect::<Vec<_>>()
    };

    let mut stripped_users = Vec::new();
    let mut stripped_roles = Vec::new();
    let mut allowed_overwrites = Vec::new();

    for member_id in interaction.data.resolved.members.keys() {
        let is_disallowed = disallowed_roles_users.iter().any(|d| match d.kind {
            PermissionOverwriteType::Member(user_id) => user_id == *member_id,
            _ => false,
        });

        if is_disallowed {
            stripped_users.push(*member_id);
        } else {
            allowed_overwrites.push(PermissionOverwriteType::Member(*member_id));
        }
    }

    for role in &interaction.data.resolved.roles {
        let is_disallowed = disallowed_roles_users.iter().any(|d| match d.kind {
            PermissionOverwriteType::Role(role_id) => role_id == role.id,
            _ => false,
        });

        if is_disallowed {
            stripped_roles.push(role.id);
        } else {
            allowed_overwrites.push(PermissionOverwriteType::Role(role.id));
        }
    }

    // toggle values
    for overwrite in &allowed_overwrites {
        match overwrite {
            PermissionOverwriteType::Member(user_id) => {
                if let Some(pos) = private_vc.allowlist_users.iter().position(|u| u == user_id) {
                    private_vc.allowlist_users.remove(pos);
                } else {
                    private_vc.allowlist_users.push(*user_id);
                }
            }
            PermissionOverwriteType::Role(role_id) => {
                if let Some(pos) = private_vc.allowlist_roles.iter().position(|r| r == role_id) {
                    private_vc.allowlist_roles.remove(pos);
                } else {
                    private_vc.allowlist_roles.push(*role_id);
                }
            }
            _ => {}
        }
    }

    update_permissions(
        ctx,
        interaction,
        stripped_users,
        stripped_roles,
        private_vc,
        &disallowed_roles_users,
    )
    .await;
}

async fn deny(ctx: &Context, interaction: &ComponentInteraction, mut private_vc: PrivateVc) {
    let disallowed_roles_users = {
        let Some(guild) = ctx.cache.guild(GUILD) else {
            return;
        };

        let Some(Some(parent_id)) = guild
            .channels
            .get(&interaction.channel_id.expect_channel())
            .map(|c| c.parent_id)
        else {
            return; // should be unreachable/not designed to work this way.
        };

        let Some(parent) = guild.channels.get(&parent_id) else {
            return;
        };

        parent
            .permission_overwrites
            .iter()
            .cloned()
            .collect::<Vec<_>>()
    };

    let mut stripped_users = Vec::new();
    let mut denied_overwrites = Vec::new();

    for (member_id, partial_member) in &interaction.data.resolved.members {
        let is_mod = MOD_ROLES
            .iter()
            .any(|mod_role| partial_member.roles.contains(mod_role));

        if is_mod {
            stripped_users.push(*member_id);
            continue;
        }

        if private_vc.owner_id == *member_id {
            continue;
        }

        let is_disallowed = disallowed_roles_users.iter().any(|d| match d.kind {
            PermissionOverwriteType::Member(user_id) => user_id == *member_id,
            _ => false,
        });

        if is_disallowed {
            stripped_users.push(*member_id);
        } else {
            denied_overwrites.push(PermissionOverwriteType::Member(*member_id));
        }
    }

    // toggle values
    for overwrite in &denied_overwrites {
        if let PermissionOverwriteType::Member(user_id) = overwrite {
            if let Some(pos) = private_vc.allowlist_users.iter().position(|u| u == user_id) {
                private_vc.allowlist_users.remove(pos);
                private_vc.denylist_users.push(*user_id);
            } else {
                private_vc.denylist_users.push(*user_id);
            }
        }
    }

    update_permissions(
        ctx,
        interaction,
        stripped_users,
        vec![],
        private_vc,
        &disallowed_roles_users,
    )
    .await;
}

// mod role users need to not be allowed to be denied.

const MOD_ROLES: [RoleId; 5] = [
    // community
    RoleId::new(98459030455853056),
    // voice
    RoleId::new(723115326195367936),
    // trial
    RoleId::new(781213498998915123),
    // moth
    RoleId::new(1062803266636873781),
    // mod bot
    RoleId::new(150811709009821696),
];

pub(super) async fn update_permissions(
    ctx: &Context,
    interaction: &ComponentInteraction,
    mut stripped_users: Vec<UserId>,
    mut stripped_roles: Vec<RoleId>,
    private_vc: PrivateVc,
    parent_permissions: &[PermissionOverwrite],
) {
    let permissions = build_permission_overwrites(
        &private_vc,
        parent_permissions,
        &mut stripped_users,
        &mut stripped_roles,
    );

    let data = ctx.data_ref::<Data>();
    #[expect(unused_braces)]
    if data
        .database
        .create_private_vc(
            interaction.channel_id.expect_channel(),
            private_vc.message_id,
            Some(GUILD),
            private_vc.owner_id,
            private_vc.allowlist_roles.clone(),
            private_vc.allowlist_users.clone(),
            private_vc.trusted_users.clone(),
            private_vc.denylist_users.clone(),
            { ctx.cache.current_user().id },
        )
        .await
        .is_err()
    {
        return;
    }

    let _ = tokio::join!(
        super::task::message(
            ctx,
            interaction.channel_id.expect_channel(),
            private_vc.message_id,
            &private_vc,
        ),
        apply_channel_permissions(ctx, interaction.channel_id.expect_channel(), permissions),
        send_update_message(ctx, interaction, stripped_users, stripped_roles),
    );
}

pub(super) fn build_permission_overwrites(
    private_vc: &PrivateVc,
    parent_permissions: &[PermissionOverwrite],
    stripped_users: &mut Vec<UserId>,
    stripped_roles: &mut Vec<RoleId>,
) -> Vec<PermissionOverwrite> {
    let mut permissions = parent_permissions.to_vec();

    for role_id in &private_vc.allowlist_roles {
        if parent_permissions
            .iter()
            .any(|p| p.kind == PermissionOverwriteType::Role(*role_id))
        {
            stripped_roles.push(*role_id);
        } else {
            permissions.push(PermissionOverwrite {
                allow: Permissions::CONNECT,
                deny: Permissions::empty(),
                kind: PermissionOverwriteType::Role(*role_id),
            });
        }
    }

    for user in &private_vc.denylist_users {
        permissions.push(PermissionOverwrite {
            allow: Permissions::empty(),
            deny: Permissions::CONNECT,
            kind: PermissionOverwriteType::Member(*user),
        });
    }

    for user_id in &private_vc.allowlist_users {
        if private_vc.denylist_users.contains(user_id) {
            continue;
        }

        if parent_permissions
            .iter()
            .any(|p| p.kind == PermissionOverwriteType::Member(*user_id))
        {
            stripped_users.push(*user_id);
        } else {
            permissions.push(PermissionOverwrite {
                allow: Permissions::CONNECT,
                deny: Permissions::empty(),
                kind: PermissionOverwriteType::Member(*user_id),
            });
        }
    }

    permissions.push(PermissionOverwrite {
        allow: Permissions::CONNECT,
        deny: Permissions::empty(),
        kind: PermissionOverwriteType::Member(private_vc.owner_id),
    });

    if let Some(p) = permissions
        .iter_mut()
        .find(|p| p.kind == PermissionOverwriteType::Role(GUILD.get().into()))
    {
        if private_vc.allowlist_roles.is_empty() && private_vc.allowlist_users.is_empty() {
            p.allow.insert(Permissions::CONNECT);
            p.deny.remove(Permissions::CONNECT);
        } else {
            p.allow.remove(Permissions::CONNECT);
            p.deny.insert(Permissions::CONNECT);
        }
    }

    permissions
}

pub(super) async fn apply_channel_permissions(
    ctx: &Context,
    channel_id: ChannelId,
    permissions: Vec<PermissionOverwrite>,
) {
    let _ = channel_id
        .edit(&ctx.http, EditChannel::new().permissions(permissions))
        .await;
}

async fn send_update_message(
    ctx: &Context,
    interaction: &ComponentInteraction,
    stripped_users: Vec<UserId>,
    stripped_roles: Vec<RoleId>,
) {
    use std::fmt::Write;

    let mut msg = String::from("Successfully updated permissions for the voice channel.\n");

    if !stripped_users.is_empty() {
        let mention_list = stripped_users
            .iter()
            .map(|u| aformat::aformat!("<@{}>", u.get()))
            .collect::<Vec<_>>()
            .join(", ");

        writeln!(
            msg,
            "The below user(s) have been removed from your selection because they are either a \
             moderator or otherwise disallowed.\n{mention_list}"
        )
        .unwrap();
    }

    if !stripped_roles.is_empty() {
        let mention_list = stripped_roles
            .iter()
            .map(|r| aformat::aformat!("<@&{}>", r.get()))
            .collect::<Vec<_>>()
            .join(", ");

        writeln!(
            msg,
            "The below role(s) have been removed from your selection because they are either a \
             moderator, muted or otherwise disallowed.\n{mention_list}"
        )
        .unwrap();
    }

    let _ = interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(msg)
                    .allowed_mentions(CreateAllowedMentions::new())
                    .ephemeral(true),
            ),
        )
        .await;
}

async fn disconnect(ctx: &Context, interaction: &ComponentInteraction) {
    let (user_id, partial_member) = interaction
        .data
        .resolved
        .members
        .iter()
        .next()
        .expect("select menu should contain at least 1 user");

    if !vc_has_user(ctx, interaction.channel_id.expect_channel(), *user_id) {
        let _ = interaction
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("This user is not in the voice channel.")
                        .ephemeral(true),
                ),
            )
            .await;
        return;
    }

    let is_mod = MOD_ROLES
        .iter()
        .any(|mod_role| partial_member.roles.contains(mod_role));

    if is_mod {
        let _ = interaction
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content(
                            "It is not possible to disconnect moderators, please ask them kindly \
                             to leave.",
                        )
                        .ephemeral(true),
                ),
            )
            .await;
        return;
    }

    if GUILD.disconnect_member(&ctx.http, *user_id).await.is_ok() {
        let _ = interaction
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content(format!("Successfully disconnected <@{user_id}>"))
                        .ephemeral(true)
                        .allowed_mentions(CreateAllowedMentions::new()),
                ),
            )
            .await;
    } else {
        let _ = interaction
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content(format!(
                            "Could not disconnect <@{user_id}> for unknown reason."
                        ))
                        .ephemeral(true)
                        .allowed_mentions(CreateAllowedMentions::new()),
                ),
            )
            .await;
    }
}
