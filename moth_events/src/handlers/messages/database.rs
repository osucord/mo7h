use regex::Regex;
use serenity::all::{EmojiId, ReactionType};
use small_fixed_array::FixedString;
use sqlx::query;
use std::sync::LazyLock;
use unicode_segmentation::UnicodeSegmentation;

use crate::Error;
use lumi::serenity_prelude::Message;
use moth_core::data::structs::Data;

pub static EMOJI_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<(a)?:([a-zA-Z0-9_]{2,32}):(\d{1,20})>").unwrap());

fn get_emojis_in_msg(msg: &str) -> impl Iterator<Item = &str> {
    msg.graphemes(true)
        .filter(|g| emojis::get(g).is_some())
        .take(3)
}

pub(super) async fn insert_message(data: &Data, message: &Message) -> Result<(), Error> {
    let mut transaction = data.database.db.begin().await?;

    let mut unicode_emojis = get_emojis_in_msg(&message.content).peekable();
    let has_unicode_emoji = unicode_emojis.peek().is_some();

    let mut custom_captures = EMOJI_REGEX.captures_iter(&message.content).peekable();
    let has_custom_emote = custom_captures.peek().is_some();

    if !has_unicode_emoji && !has_custom_emote && message.sticker_items.is_empty() {
        return Ok(());
    }

    let message_data = data
        .database
        .get_message(
            message.id,
            message.channel_id,
            message.guild_id,
            message.author.id,
        )
        .await?;

    if message.guild_id.is_some() {
        for sticker in &message.sticker_items {
            let sticker_id = sticker.id.get() as i64;
            query!(
                "INSERT INTO stickers (sticker_id, sticker_name) VALUES ($1, $2) ON CONFLICT \
                 (sticker_id) DO NOTHING",
                sticker_id,
                &*sticker.name
            )
            .execute(&mut *transaction)
            .await?;

            query!(
                "INSERT INTO sticker_usage (message_id, sticker_id, user_id, guild_id, used_at) \
                 VALUES ($1, $2, $3, $4, $5)",
                message_data.id,
                sticker_id,
                message_data.user_id,
                message_data.guild_id,
                *message.id.created_at()
            )
            .execute(&mut *transaction)
            .await?;
        }

        // this should be rewritten as to not attempt to insert the same emoji back to back into the emotes table.
        // i can support this later when i do caches (TODO)
        for captures in custom_captures.take(3) {
            let Ok(id) = &captures[3].parse::<u64>() else {
                println!("Failed to parse id for custom emote: {}", &captures[3]);
                continue;
            };

            let reaction_type = ReactionType::Custom {
                animated: captures.get(1).is_some(),
                id: EmojiId::new(*id),
                name: Some(FixedString::from_str_trunc(&captures[2])),
            };

            data.emote_processor
                .sender
                .message_add(message, reaction_type)
                .await;
        }

        for emoji in unicode_emojis {
            data.emote_processor
                .sender
                .message_add(
                    message,
                    ReactionType::Unicode(FixedString::from_str_trunc(emoji)),
                )
                .await;
        }

        transaction.commit().await?;
    }

    Ok(())
}
