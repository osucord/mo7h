#![warn(clippy::pedantic)]
#![allow(clippy::unreadable_literal)]

mod data;
mod error;

use lumi::serenity_prelude::{self as serenity};
use moth_core::data::structs::Error;
use std::{sync::Arc, time::Duration};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    dotenvy::dotenv().unwrap();

    let options = lumi::FrameworkOptions {
        commands: moth_commands::commands(),
        prefix_options: lumi::PrefixFrameworkOptions {
            edit_tracker: Some(Arc::new(lumi::EditTracker::for_timespan(
                Duration::from_secs(600),
            ))),
            stripped_dynamic_prefix: Some(|_, msg, _| Box::pin(try_strip_prefix(msg))),
            ..Default::default()
        },

        on_error: |error| Box::pin(error::handler(error)),

        command_check: Some(|ctx| Box::pin(moth_commands::command_check(ctx))),

        skip_checks_for_owners: false,
        ..Default::default()
    };

    let framework = lumi::Framework::new(options);

    let token = serenity::Token::from_env("MOTH_TOKEN")
        .expect("Missing `MOTH_TOKEN` environment variable.");
    let intents = serenity::GatewayIntents::non_privileged()
        | serenity::GatewayIntents::MESSAGE_CONTENT
        | serenity::GatewayIntents::GUILD_MEMBERS
        | serenity::GatewayIntents::GUILD_PRESENCES;

    let mut settings = serenity::Settings::default();
    settings.max_messages = 1000;

    let data = data::setup().await;

    /*     {
        let mut joins = data.osu_game_joins.lock();
        joins.push_back(UserId::new(111111111111111111));
        joins.push_back(UserId::new(111111111111111112));
        joins.push_back(UserId::new(111111111111111113));
        joins.push_back(UserId::new(111111111111111114));
        joins.push_back(UserId::new(111111111111111115));
        joins.push_back(UserId::new(222222222222222221));
        joins.push_back(UserId::new(222222222222222222));
        joins.push_back(UserId::new(222222222222222223));
        joins.push_back(UserId::new(222222222222222224));
        joins.push_back(UserId::new(222222222222222225));
    } */

    let mut client = serenity::Client::builder(token, intents)
        .framework(framework)
        .data(data)
        .cache_settings(settings)
        .event_handler(moth_events::Handler)
        .await
        .unwrap();

    client.start().await.unwrap();
}

// i don't want Accelas commands using my norm prefix, this is jank.
#[expect(clippy::unused_async)]
async fn try_strip_prefix(msg: &serenity::Message) -> Result<Option<(&str, &str)>, Error> {
    // accela stuff

    let accela_prefix = ">>";
    let accela_commands = ["playmore", "play", "p", "talkmore", "talk", "t"];

    if let Some(stripped) = msg.content.strip_prefix(accela_prefix) {
        if let Some(first_word) = stripped.split_whitespace().next() {
            if accela_commands
                .iter()
                .any(|cmd| cmd.eq_ignore_ascii_case(first_word))
            {
                return Ok(Some(msg.content.split_at(accela_prefix.len())));
            }
        }
    }

    let normal_prefixes = ["-", "m!", "m"];
    for prefix in normal_prefixes {
        if let Some(stripped) = msg.content.strip_prefix(prefix) {
            if let Some(first_word) = stripped.split_whitespace().next() {
                if accela_commands
                    .iter()
                    .any(|cmd| cmd.eq_ignore_ascii_case(first_word))
                {
                    return Ok(None);
                }
                return Ok(Some(msg.content.split_at(prefix.len())));
            }
        }
    }

    Ok(None)
}
