use crate::{Context, Error};

use lumi::serenity_prelude::{self as serenity};

use super::allowed_user;

// in the future i will use autocomplete
// like... pop the start of a string, if its not a valid snowflake, parse as the actual starboard id
// the response to the user will be like id: username: first bit of the content (attachment fallback)

#[lumi::command(
    rename = "starboard-admin",
    slash_command,
    hide_in_help,
    guild_only,
    check = "allowed_user",
    subcommands("attachments", "content", "reset"),
    install_context = "Guild"
)]
pub async fn starboard_admin(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Swapout an attachment
#[lumi::command(slash_command)]
pub async fn attachments(
    ctx: Context<'_>,
    starboard_link: serenity::Message,
    #[description = "If none, resets array, otherwise replaces element or insert."]
    attachment_index: Option<u8>,
    url: String,
) -> Result<(), Error> {
    let Ok(mut starboard) = ctx
        .data()
        .database
        .get_starboard_msg_by_starboard_id(starboard_link.id)
        .await
    else {
        ctx.say("The provided link is not a starboard message.")
            .await?;
        return Ok(());
    };

    if let Some(index) = attachment_index {
        if let Some(entry) = starboard.attachment_urls.get_mut(index as usize) {
            *entry = url
                .split_once('?')
                .map_or_else(|| url.clone(), |(before, _)| before.to_string());
        } else {
            starboard.attachment_urls.push(url);
        }
    } else {
        starboard.attachment_urls.push(url);
    }

    ctx.data()
        .database
        .update_starboard_fields(&starboard)
        .await?;

    ctx.say("Updated, please update starboard to see changes.")
        .await?;

    Ok(())
}

/// Change the content of a starboard entry.
#[lumi::command(slash_command)]
pub async fn content(
    ctx: Context<'_>,
    starboard_link: serenity::Message,
    content: String,
) -> Result<(), Error> {
    let Ok(mut starboard) = ctx
        .data()
        .database
        .get_starboard_msg_by_starboard_id(starboard_link.id)
        .await
    else {
        ctx.say("The provided link is not a starboard message.")
            .await?;
        return Ok(());
    };

    starboard.content = content;

    ctx.data()
        .database
        .update_starboard_fields(&starboard)
        .await?;

    ctx.say("Updated, please update starboard to see changes.")
        .await?;

    Ok(())
}

/// Reset a starboard back to its "review" state, good for if you accidentally deny, not for accept.
#[lumi::command(slash_command)]
pub async fn reset(ctx: Context<'_>, starboard_link: serenity::Message) -> Result<(), Error> {
    let Ok(mut starboard) = ctx
        .data()
        .database
        .get_starboard_msg_by_starboard_id(starboard_link.id)
        .await
    else {
        ctx.say("The provided link is not a starboard message.")
            .await?;
        return Ok(());
    };

    starboard.starboard_status = moth_core::data::database::StarboardStatus::InReview;

    ctx.data()
        .database
        .update_starboard_fields(&starboard)
        .await?;

    ctx.say("Updated, please update starboard to see changes.")
        .await?;

    Ok(())
}
