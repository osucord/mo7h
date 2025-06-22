use lumi::CreateReply;
use serenity::all::{CreateAttachment, EditMember, GuildMemberFlags};

use crate::{Context, Error};

#[lumi::command(
    rename = "unverify-all",
    prefix_command,
    required_bot_permissions = "MODERATE_MEMBERS",
    hide_in_help,
    owners_only,
    guild_only
)]
pub async fn unverify_all(ctx: Context<'_>) -> Result<(), Error> {
    let no_nos = [
        442299248172597258_u64,
        306853470638440460,
        685796754939052047,
        314081534824939530,
        871823102068260915,
        198903530394877952,
        575119446134226964,
        838585979610726400,
        724445579811487775,
        668541709017153592,
        793282666367680564,
        906664596679577620,
        208101469365207041,
        815083930591297546,
        1136901818882994207,
        1011348779976372296,
        109630313449148416,
        1353182867781455985,
    ];

    let users = {
        let Some(cache) = ctx.guild() else {
            ctx.say("Cannot find guild in cache.").await?;
            return Ok(());
        };

        cache
            .members
            .iter()
            .filter(|m| m.flags.contains(GuildMemberFlags::BYPASSES_VERIFICATION))
            .map(|m| m.user.id)
            .collect::<Vec<_>>()
    };

    ctx.say("Unverifying all accounts but mod alts...").await?;

    let guild_id = ctx.guild_id().unwrap();
    for user_id in users {
        if !no_nos.contains(&user_id.get()) {
            let _ = guild_id
                .edit_member(
                    ctx.http(),
                    user_id,
                    // if discord ever adds other flags I should store them beforehand.
                    EditMember::new().flags(GuildMemberFlags::empty()),
                )
                .await;
        }
    }

    ctx.say("complete!").await?;

    Ok(())
}

#[lumi::command(
    rename = "count-verified",
    prefix_command,
    hide_in_help,
    owners_only,
    guild_only
)]
pub async fn count_verified(ctx: Context<'_>) -> Result<(), Error> {
    let count = {
        let Some(cache) = ctx.guild() else {
            ctx.say("Cannot find guild in cache.").await?;
            return Ok(());
        };

        cache
            .members
            .iter()
            .filter(|m| m.flags.contains(GuildMemberFlags::BYPASSES_VERIFICATION))
            .count()
    };

    ctx.say(format!("{count} users are bypassing verification"))
        .await?;

    Ok(())
}

#[lumi::command(
    rename = "count-tagged",
    prefix_command,
    hide_in_help,
    owners_only,
    guild_only
)]
pub async fn count_tagged(ctx: Context<'_>) -> Result<(), Error> {
    use std::fmt::Write;

    let (user_count, tagged_count, user_list, tagged_list) = {
        let Some(cache) = ctx.guild() else {
            ctx.say("Cannot find guild in cache.").await?;
            return Ok(());
        };

        let mut user_count = 0;
        let mut tagged_count = 0;
        let mut user_list = String::new();
        let mut tagged_list = String::new();

        for member in &cache.members {
            if let Some(primary) = &member.user.primary_guild {
                if primary.identity_guild_id == Some(cache.id) {
                    user_count += 1;
                    writeln!(user_list, "{}", member.user.id)?;

                    if primary.tag.is_some() {
                        tagged_count += 1;
                        writeln!(tagged_list, "{}", member.user.id)?;
                    }
                }
            }
        }

        (user_count, tagged_count, user_list, tagged_list)
    };

    ctx.send(
        CreateReply::new()
            .content(format!(
                "{user_count} users represent us, {tagged_count} have our tag (in this server)."
            ))
            .attachment(CreateAttachment::bytes(user_list.into_bytes(), "users.txt"))
            .attachment(CreateAttachment::bytes(
                tagged_list.into_bytes(),
                "tagged_users.txt",
            )),
    )
    .await?;

    Ok(())
}

#[must_use]
pub fn commands() -> [crate::Command; 3] {
    [unverify_all(), count_verified(), count_tagged()]
}
