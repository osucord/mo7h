use lumi::CreateReply;
use serenity::all::{
    CreateAllowedMentions, CreateAttachment, CreateComponent, CreateTextDisplay, MessageFlags,
};

use crate::{Context, Error};
use std::fmt::Write;

/// Get the info of all characters in a message.
#[lumi::command(
    slash_command,
    prefix_command,
    category = "Utility",
    install_context = "Guild|User",
    interaction_context = "Guild|BotDm|PrivateChannel",
    aliases("chars", "char-info")
)]
pub async fn charinfo(
    ctx: Context<'_>,
    hidden: Option<bool>,
    #[description = "String containing characters"]
    #[rest]
    string: String,
) -> Result<(), Error> {
    let mut result = String::new();
    for c in string.chars() {
        let digit = c as u32;
        if let Some(name) = unicode_names2::name(c) {
            writeln!(
                result,
                "[`\\U{digit:08x}`](<http://www.fileformat.info/info/unicode/char/{digit:08x}>): \
                 {name} — {c}",
            )
            .unwrap();
        } else {
            writeln!(
                result,
                "[`\\U{digit:08x}`](<http://www.fileformat.info/info/unicode/char/{digit:08x}>): \
                 Name not found. — {c}"
            )
            .unwrap();
        }
    }

    if result.len() > 4000 {
        ctx.send(CreateReply::new().attachment(CreateAttachment::bytes(result, "chars.txt")))
            .await?;
        return Ok(());
    }

    // fix upstream first
    let mut flags = MessageFlags::IS_COMPONENTS_V2;

    if hidden.unwrap_or(true) {
        flags |= MessageFlags::EPHEMERAL;
    }

    ctx.send(
        CreateReply::new()
            .components(&[CreateComponent::TextDisplay(CreateTextDisplay::new(result))])
            .allowed_mentions(
                CreateAllowedMentions::new()
                    .everyone(false)
                    .all_roles(false)
                    .all_users(false),
            )
            .flags(flags),
    )
    .await?;

    Ok(())
}

#[must_use]
pub fn commands() -> [crate::Command; 1] {
    [charinfo()]
}
