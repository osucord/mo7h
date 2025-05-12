use crate::{helper::get_guild_name_override, Error};
use moth_ansi::{HI_GREEN, MAGENTA, RED, RESET};
use serenity::all::{Context, GuildId, Permissions, Role, RoleId};

use std::fmt::Write;

pub(crate) async fn role_create(ctx: &Context, role: &Role) -> Result<(), Error> {
    let guild_name = get_guild_name_override(ctx, &ctx.data(), Some(role.guild_id));

    // TODO: log details.

    println!(
        "{MAGENTA}[{guild_name}] A role called {} was created.",
        role.name
    );

    Ok(())
}

pub(crate) async fn role_delete(
    ctx: &Context,
    guild_id: GuildId,
    role_id: RoleId,
    role: Option<&Role>,
) -> Result<(), Error> {
    let guild_name = get_guild_name_override(ctx, &ctx.data(), Some(guild_id));

    if let Some(role) = role {
        println!(
            "{MAGENTA}[{guild_name}] A role called {} was deleted! (ID:{role_id}){RESET}",
            role.name
        );
    } else {
        println!(
            "{MAGENTA}[{guild_name}] A role called {role_id} was deleted, but was not \
             cached!{RESET}"
        );
    }

    Ok(())
}

pub(crate) async fn role_update(
    ctx: &Context,
    old_role: Option<&Role>,
    role: &Role,
) -> Result<(), Error> {
    let guild_name = get_guild_name_override(ctx, &ctx.data(), Some(role.guild_id));
    let mut modified = false;

    let Some(old_role) = old_role else {
        println!(
            "{MAGENTA}[{guild_name}] {} (ID:{}) was updated but was not in the cache!",
            role.name, role.id
        );
        return Ok(());
    };

    let mut string = format!(
        "{MAGENTA}[{guild_name}] A role {} (ID:{}) was updated!{RESET}",
        role.name, role.id
    );

    if old_role.name != role.name {
        writeln!(string, "\nname: {} -> {}", old_role.name, role.name).unwrap();
        modified = true;
    }

    if old_role.colour != role.colour {
        modified = true;

        let old = moth_ansi::from_colour(old_role.colour.0);
        let new = moth_ansi::from_colour(role.colour.0);

        // Basically, its only none if there was no colour, so basically don't print if no colour?
        // kinda because... theres no point
        if let (Some(old_col), Some(new_col)) = (old, new) {
            write!(
                string,
                "\ncolour: #{old_col}{:06X}{RESET} -> #{new_col}{:06X}{RESET}",
                old_role.colour.0, role.colour.0
            )
            .unwrap();
        } else if let Some(new_col) = new {
            write!(
                string,
                "\ncolour: None -> #{new_col}{:06X}{RESET}",
                role.colour.0
            )
            .unwrap();
        } else if let Some(old_col) = old {
            write!(
                string,
                "\ncolour: #{old_col}{:06X}{RESET} -> None",
                old_role.colour.0
            )
            .unwrap();
        }
    }

    /*     if old_role.position != role.position {
        writeln!(
            string,
            "position: {} -> {}",
            old_role.position, role.position
        )
        .unwrap();
    } */

    // TODO: write some stuff for the RoleTags even though it'll hardly ever change.

    if old_role.icon != role.icon {
        modified = true;
        write!(string, "\nIcon has changed!").unwrap();
    }

    if old_role.unicode_emoji != role.unicode_emoji {
        modified = true;
        let old = &old_role.unicode_emoji;
        let new = &role.unicode_emoji;

        if let (Some(old_emoji), Some(new_emoji)) = (old, new) {
            write!(string, "\nEmoji changed: {old_emoji} -> {new_emoji}").unwrap();
        } else if let Some(new_emoji) = new {
            write!(string, "\nemoji: None -> {new_emoji}").unwrap();
        } else if let Some(old_emoji) = old {
            write!(string, "\nold emoji: {old_emoji} -> None").unwrap();
        }
    }

    if old_role.permissions != role.permissions {
        modified = true;
        let changes = permission_changes(old_role.permissions, role.permissions);
        write!(string, "\n{changes}").unwrap();
    }

    if old_role.hoist() != role.hoist() {
        modified = true;
        write!(
            string,
            "\nhoisted: {} -> {}",
            old_role.hoist(),
            role.hoist()
        )
        .unwrap();
    }

    if old_role.managed() != role.managed() {
        modified = true;
        write!(
            string,
            "\nmanaged: {} -> {}",
            old_role.managed(),
            role.managed()
        )
        .unwrap();
    }

    if old_role.mentionable() != role.mentionable() {
        modified = true;
        write!(
            string,
            "\nmentionable: {} -> {}",
            old_role.mentionable(),
            role.mentionable()
        )
        .unwrap();
    }

    if modified {
        string.strip_suffix('\n').unwrap_or(&string);
        println!("{string}");
    }

    Ok(())
}

fn permission_changes(old: Permissions, new: Permissions) -> String {
    let added = new - old;
    let removed = old - new;

    let mut changes = String::new();

    for add in added {
        writeln!(
            changes,
            "{HI_GREEN}+ {}",
            add.get_permission_names().first().unwrap()
        )
        .unwrap();
    }

    for remove in removed {
        writeln!(
            changes,
            "{RED}- {}",
            remove.get_permission_names().first().unwrap()
        )
        .unwrap();
    }

    changes.pop();
    changes
}
