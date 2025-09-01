use crate::{Context, Error};

use lumi::{
    CreateReply,
    serenity_prelude::{self as serenity, UserId},
};
use moth_core::data::database::StarboardStatus;

use ::serenity::all::{
    ChannelType, Colour, CreateComponent, CreateContainer, CreateSeparator, CreateTextDisplay,
    GenericInteractionChannel, MessageFlags,
};

use itertools::Itertools;
use std::{
    collections::{HashMap, hash_map::Entry},
    fmt::Write,
};

pub mod admin;

#[lumi::command(
    prefix_command,
    hide_in_help,
    guild_only,
    check = "allowed_user",
    rename = "list-queued",
    aliases("list_queued")
)]
pub async fn list_queued(ctx: Context<'_>) -> Result<(), Error> {
    let sorted_starboard = ctx
        .data()
        .database
        .get_all_starboard()
        .await?
        .iter()
        .filter(|m| m.starboard_status == StarboardStatus::InReview)
        .sorted_by(|a, b| b.star_count.cmp(&a.star_count))
        .cloned()
        .collect::<Vec<_>>();

    let mut description = String::new();

    for entry in sorted_starboard {
        // hardcoded GuildId because its a single guild bot
        let link = format!(
            "https://discord.com/channels/{}/{}/{}",
            ctx.data().starboard_config.guild_id,
            *entry.starboard_message_channel,
            *entry.starboard_message_id
        );
        writeln!(description, "{} ⭐ {link}", entry.star_count).unwrap();
    }

    // TODO: won't be a problem for some time but paginating this command would be good, but i'm too lazy.
    if description.len() > 4000 {
        ctx.say(
            "Output is too long, printed into terminal instead. Ask her for the output then force \
             them to paginate this",
        )
        .await?;
        println!("{description}");
    } else {
        let embed = serenity::CreateEmbed::new()
            .title("Starboard entries in review")
            .description(description)
            .colour(serenity::Colour::BLUE);
        let builder = lumi::CreateReply::new().embed(embed);
        ctx.send(builder).await?;
    }

    Ok(())
}

#[lumi::command(
    slash_command,
    hide_in_help,
    guild_only,
    check = "allowed_user",
    rename = "add-starboard-override"
)]
pub async fn add_starboard_override(
    ctx: Context<'_>,
    channel: GenericInteractionChannel,
    count: u8,
) -> Result<(), Error> {
    if let GenericInteractionChannel::Channel(c) = &channel
        && c.base.kind == ChannelType::Category
    {
        ctx.say("Cannot use this on a category yet.").await?;
        return Ok(());
    }

    ctx.data()
        .database
        .add_starboard_override(&ctx.data().database.starboard, channel.id(), count)
        .await?;

    ctx.say("Done.").await?;

    Ok(())
}

#[lumi::command(
    slash_command,
    hide_in_help,
    guild_only,
    check = "allowed_user",
    rename = "remove-starboard-override"
)]
pub async fn remove_starboard_override(
    ctx: Context<'_>,
    channel: GenericInteractionChannel,
) -> Result<(), Error> {
    if let GenericInteractionChannel::Channel(c) = &channel
        && c.base.kind == ChannelType::Category
    {
        ctx.say("Categories can't have overrides.").await?;
        return Ok(());
    }

    let present = ctx
        .data()
        .database
        .remove_starboard_override(&ctx.data().database.starboard, channel.id())
        .await?;

    if present {
        ctx.say("Successfully removed override").await?;
    } else {
        ctx.say("Cannot remove something that does not exist.")
            .await?;
    }

    Ok(())
}

#[lumi::command(
    slash_command,
    hide_in_help,
    guild_only,
    check = "allowed_user",
    rename = "list-overrides"
)]
pub async fn list_overrides(ctx: Context<'_>) -> Result<(), Error> {
    // TODO: add add functionality to this function

    // pretty sure i could make threads group a vec in like 5 minutes if i wanted.
    let mut thread_groups = HashMap::new();
    let mut threads_no_parent: HashMap<serenity::ChannelId, Vec<serenity::ThreadId>> =
        HashMap::new();
    let mut unknowns = vec![];
    let overrides = ctx.data().database.starboard.lock().overrides.clone();

    {
        if ctx.guild_id() != Some(ctx.data().starboard_config.guild_id) {
            ctx.say("Not the right guild.").await?;
            return Ok(());
        }

        let Some(guild) = ctx.guild() else {
            ctx.say("Cannot run without a cached guild.").await?;
            return Ok(());
        };
        // populate channels first.
        for channel in overrides.keys() {
            let Some(channel) = guild.channel(*channel) else {
                unknowns.push(channel);
                continue;
            };

            match channel {
                serenity::GenericGuildChannelRef::Channel(guild_channel) => {
                    thread_groups.insert(guild_channel.id, vec![]);
                }
                serenity::GenericGuildChannelRef::Thread(_) => {}
            }
        }

        // populate threads after.
        for channel in overrides.keys() {
            let Some(channel) = guild.channel(*channel) else {
                continue;
            };

            match channel {
                serenity::GenericGuildChannelRef::Channel(_) => {}
                serenity::GenericGuildChannelRef::Thread(guild_thread) => {
                    if let Some(group) = thread_groups.get_mut(&guild_thread.parent_id) {
                        group.push(guild_thread.id);
                    } else {
                        match threads_no_parent.entry(guild_thread.parent_id) {
                            Entry::Occupied(mut o) => o.get_mut().push(guild_thread.id),
                            Entry::Vacant(e) => {
                                e.insert(vec![guild_thread.id]);
                            }
                        }
                    }
                }
            }
        }
    }

    let title = CreateComponent::TextDisplay(CreateTextDisplay::new(format!(
        "Starboard default requirement: {} {}",
        ctx.data().starboard_config.threshold,
        ctx.data().starboard_config.star_emoji
    )));

    let mut content = String::new();
    let mut all_groups: Vec<(_, Vec<_>, ScoreDisplay)> = Vec::new();

    for (parent, threads) in &thread_groups {
        let parent_score = *overrides.get(&parent.widen()).unwrap_or(&0);
        all_groups.push((*parent, threads.clone(), ScoreDisplay::Count(parent_score)));
    }

    for (parent, threads) in &threads_no_parent {
        all_groups.push((*parent, threads.clone(), ScoreDisplay::Default));
    }

    all_groups.sort_by_key(|(_, _, score)| match score {
        ScoreDisplay::Default => std::cmp::Reverse(0),
        ScoreDisplay::Count(val) => std::cmp::Reverse(*val),
    });

    // Write output
    for (parent, threads, parent_score) in all_groups {
        writeln!(
            content,
            "<#{parent}>: **{}** {}",
            parent_score,
            ctx.data().starboard_config.star_emoji
        )
        .unwrap();

        let mut sorted_threads: Vec<_> = threads
            .into_iter()
            .map(|thread| {
                let score = *overrides.get(&thread.widen()).unwrap_or(&0);
                (thread, score)
            })
            .collect();

        sorted_threads.sort_by_key(|&(_, score)| std::cmp::Reverse(score));

        for (i, (thread, score)) in sorted_threads.iter().enumerate() {
            let emoji = if (i + 1) == sorted_threads.len() {
                "<:end:1367578699045798089>"
            } else {
                "<:cont:1367578704624095253>"
            };

            writeln!(
                content,
                "{emoji} <#{thread}>: **{}** {}",
                score,
                ctx.data().starboard_config.star_emoji
            )
            .unwrap();
        }
    }

    if content.is_empty() {
        write!(content, "No overrides!").unwrap();
    }

    let components = &[
        title,
        CreateComponent::Separator(CreateSeparator::new(true)),
        CreateComponent::TextDisplay(CreateTextDisplay::new(content)),
    ];

    let container = CreateContainer::new(components).accent_colour(Colour::DARK_ORANGE);

    ctx.send(
        CreateReply::new()
            .flags(MessageFlags::empty() | MessageFlags::IS_COMPONENTS_V2)
            .components(&[CreateComponent::Container(container)]),
    )
    .await?;

    Ok(())
}

enum ScoreDisplay {
    Default,
    Count(u8),
}

impl std::fmt::Display for ScoreDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScoreDisplay::Default => write!(f, "DEFAULT"),
            ScoreDisplay::Count(val) => write!(f, "{val}"),
        }
    }
}

#[must_use]
pub fn commands() -> [crate::Command; 5] {
    [
        list_queued(),
        add_starboard_override(),
        remove_starboard_override(),
        list_overrides(),
        admin::starboard_admin(),
    ]
}

// TODO: dedupe this with moth_core
async fn allowed_user(ctx: Context<'_>) -> Result<bool, Error> {
    // Phil, Ruben
    let a = [
        UserId::new(101090238067113984),
        UserId::new(291089948709486593),
    ];

    Ok(a.contains(&ctx.author().id))
}
