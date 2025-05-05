use std::borrow::Cow;

use aformat::ToArrayString;
use moth_ansi::MAGENTA;
use serenity::all::audit_log::Action;
use serenity::all::{AffectedRole, AuditLogEntry, Context, GuildId, MemberAction, UserId};
use small_fixed_array::{FixedArray, FixedString};

use crate::helper::{get_guild_name_override, get_user};

pub(super) async fn handle(ctx: &Context, entry: &AuditLogEntry, guild_id: GuildId) {
    if let Action::Member(member_action) = &entry.action {
        member_role_update(ctx, entry, guild_id, *member_action).await;
    }
}

async fn member_role_update(
    ctx: &Context,
    entry: &AuditLogEntry,
    guild_id: GuildId,
    member_action: MemberAction,
) {
    match member_action {
        MemberAction::Prune
        | MemberAction::BanAdd
        | MemberAction::BanRemove
        | MemberAction::Kick => kick_prune_ban(ctx, entry, guild_id, member_action).await,
        MemberAction::RoleUpdate => {}
        _ => return,
    }

    let guild_name = get_guild_name_override(ctx, &ctx.data(), Some(guild_id));

    for change in &entry.changes {
        match change {
            serenity::all::Change::RolesAdded { old: _, new } => {
                log_role_change(ctx, guild_id, entry, new, &guild_name, "added").await;
            }
            serenity::all::Change::RolesRemove { old: _, new } => {
                log_role_change(ctx, guild_id, entry, new, &guild_name, "removed").await;
            }
            _ => {}
        }
    }
}

// serenity arguments.
#[allow(clippy::ref_option)]
async fn log_role_change(
    ctx: &Context,
    guild_id: GuildId,
    entry: &AuditLogEntry,
    roles: &Option<FixedArray<AffectedRole>>,
    guild_name: &str,
    action: &str,
) {
    if let Some(roles) = roles {
        if roles.is_empty() {
            return;
        }

        let mod_name = get_user(ctx, guild_id, entry.user_id.unwrap())
            .await
            .map_or(Cow::Borrowed("UNKNOWN_USER"), |u| {
                Cow::Owned(u.name.to_string())
            });

        let user_name = if let Some(target) = entry.target_id {
            Some(get_username(ctx, guild_id, Some(UserId::new(target.get()))).await)
        } else {
            None
        };

        if roles.len() > 1 {
            let mut names_array = Vec::new();
            let mut roles_array = Vec::new();

            for affected_role in roles {
                names_array.push(affected_role.name.clone());
                roles_array.push(affected_role.id);
            }

            let names_string = names_array.join(", ");
            let roles_string = format!(
                "({})",
                roles_array
                    .iter()
                    .map(|r| r.to_arraystring())
                    .collect::<Vec<_>>()
                    .join(", ")
            );

            if let Some(user_name) = user_name {
                println!(
                    "{MAGENTA}[{guild_name}] {mod_name} {action} {user_name}'s roles: \
                     {names_string} {roles_string}"
                );
            } else {
                println!(
                    "{MAGENTA}[{guild_name}] {mod_name} {action} their own roles: {names_string} \
                     {roles_string}"
                );
            }
        } else {
            let role = roles.first().unwrap();

            if let Some(user_name) = user_name {
                println!(
                    "{MAGENTA}[{guild_name}] {mod_name} {action}: {} ({}) for {user_name}",
                    role.name, role.id
                );
            } else {
                println!(
                    "{MAGENTA}[{guild_name}] {mod_name} {action} {} ({}) for themselves",
                    role.name, role.id
                );
            }
        }
    }
}

async fn kick_prune_ban(
    ctx: &Context,
    entry: &AuditLogEntry,
    guild_id: GuildId,
    member_action: MemberAction,
) {
    let guild_name = get_guild_name_override(ctx, &ctx.data(), Some(guild_id));

    let user = get_username(ctx, guild_id, entry.user_id).await;
    let target = get_username(ctx, guild_id, entry.target_id.map(|t| UserId::new(t.get()))).await;

    let reason = entry
        .reason
        .clone()
        .unwrap_or_else(|| FixedString::from_static_trunc("No reason given."));

    match member_action {
        MemberAction::Kick => {
            println!("{MAGENTA}[{guild_name}] {target} was kicked by {user}.\nReason: {reason}");
        }
        // TODO: prune support
        MemberAction::Prune => {}
        MemberAction::BanAdd => {
            println!("{MAGENTA}[{guild_name}] {target} was banned by {user}.\nReason: {reason}");
        }
        MemberAction::BanRemove => {
            println!("{MAGENTA}[{guild_name}] {target} was unbanned by {user}.\nReason: {reason}");
        }
        _ => unreachable!(),
    }
}

async fn get_username(
    ctx: &Context,
    guild_id: GuildId,
    user_id: Option<UserId>,
) -> Cow<'static, str> {
    if let Some(user_id) = user_id {
        match get_user(ctx, guild_id, user_id).await {
            Some(user) => Cow::Owned(user.name.to_string()),
            None => Cow::Borrowed("UNKNOWN_USER"),
        }
    } else {
        Cow::Borrowed("UNKNOWN_USER")
    }
}
