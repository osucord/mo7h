use crate::{Context, Error};

use lumi::CreateReply;

use ::serenity::all::{Colour, CreateEmbed, CreateEmbedFooter};
use moth_core::verification::update_roles;
use rosu_v2::model::GameMode;
use serenity::all::{CreateEmbedAuthor, CreateMessage, UserId};

// TODO: osu guild only

#[lumi::command(slash_command, guild_only)]
pub async fn verify(ctx: Context<'_>) -> Result<(), Error> {
    let fut = ctx.data().web.auth_standby.wait_for_osu();

    let embed = CreateEmbed::new().title("osu! verification").description(format!("<:moth:1369814651193397338> [click here](https://osu.ppy.sh/oauth/authorize?client_id={}&response_type=code&scope=identify&redirect_uri=https://verify.osucord.moe&state={}) to verify your osu! profile!", ctx.data().web.osu_client_id, fut.state)).footer(CreateEmbedFooter::new("contact Moxy if you have any issues with verification")).colour(Colour::DARK_TEAL);

    let handle = ctx
        .send(CreateReply::new().embed(embed).ephemeral(true))
        .await?;

    match fut.await {
        Ok(profile) => {
            handle
                .edit(
                    ctx,
                    CreateReply::new().embed(
                        CreateEmbed::new()
                            .title(profile.username.as_str())
                            .thumbnail(profile.avatar_url)
                            .description("FUCK OFF I DON'T WANT A DESCRIPTION RIGHT NOW"),
                    ),
                )
                .await?;

            // TODO: really should just have one method for this.
            ctx.data()
                .database
                .verify_user(ctx.author().id, profile.user_id)
                .await?;

            ctx.data()
                .web
                .task_sender
                .verify(ctx.author().id, profile.user_id)
                .await;

            update_roles(
                ctx.serenity_context(),
                ctx.author().id,
                Some(rosu_v2::model::GameMode::Osu),
                profile.statistics.expect("ALWAYS SENT").global_rank,
                "User has verified their osu account.",
            )
            .await;

            let mentions = serenity::all::CreateAllowedMentions::new()
                .all_users(false)
                .everyone(false)
                .all_roles(false);

            let _ = moth_core::verification::LOG_CHANNEL
                .send_message(
                    &ctx.serenity_context().http,
                    CreateMessage::new()
                        .content(format!(
                            "✅ <@{}> has verified as {} (osu ID: {})",
                            ctx.author().id,
                            profile.username,
                            profile.user_id
                        ))
                        .allowed_mentions(mentions),
                )
                .await;
        }
        Err(_) => {
            handle
                .edit(
                    ctx,
                    CreateReply::new().content("You did not verify in time."),
                )
                .await?;
        }
    }

    Ok(())
}

#[lumi::command(slash_command, prefix_command, guild_only)]
pub async fn update(ctx: Context<'_>) -> Result<(), Error> {
    let Some((osu_id, gamemode)) = ctx.data().database.get_osu_user_id(ctx.author().id).await
    else {
        ctx.say("You are not verified!").await?;
        return Ok(());
    };

    let Ok(osu_user) = ctx.data().web.osu.user(osu_id).mode(gamemode).await else {
        ctx.say("Cannot find user? restricted?").await?;
        return Ok(());
    };

    update_roles(
        ctx.serenity_context(),
        ctx.author().id,
        Some(rosu_v2::model::GameMode::Osu),
        osu_user.statistics.expect("ALWAYS SENT").global_rank,
        "User has requested a rank update.",
    )
    .await;

    let mentions = serenity::all::CreateAllowedMentions::new()
        .all_users(false)
        .everyone(false)
        .all_roles(false);

    let _ = moth_core::verification::LOG_CHANNEL
        .send_message(
            &ctx.serenity_context().http,
            CreateMessage::new()
                .content(format!("✅ updating <@{}> manually.", ctx.author().id,))
                .allowed_mentions(mentions),
        )
        .await;

    // TODO: embed.
    ctx.say("Updated!").await?;

    Ok(())
}

#[lumi::command(slash_command, prefix_command, guild_only)]
pub async fn unlink(ctx: Context<'_>) -> Result<(), Error> {
    let osu_id = ctx.data().database.unlink_user(ctx.author().id).await?;

    ctx.data()
        .web
        .task_sender
        .unverify(ctx.author().id, osu_id)
        .await;

    update_roles(
        ctx.serenity_context(),
        ctx.author().id,
        None,
        None,
        "User has unlinked their account.",
    )
    .await;

    let mentions = serenity::all::CreateAllowedMentions::new()
        .all_users(false)
        .everyone(false)
        .all_roles(false);

    let _ = moth_core::verification::LOG_CHANNEL
        .send_message(
            &ctx.serenity_context().http,
            CreateMessage::new()
                .content(format!(
                    "✅ <@{}> has unlinked their account.",
                    ctx.author().id,
                ))
                .allowed_mentions(mentions),
        )
        .await;

    ctx.say("Successfully unlinked.").await?;

    Ok(())
}

#[derive(Debug, lumi::ChoiceParameter)]
enum GameModeChoice {
    Osu,
    Mania,
    Taiko,
    Catch,
}

impl From<GameModeChoice> for GameMode {
    fn from(val: GameModeChoice) -> Self {
        match val {
            GameModeChoice::Osu => GameMode::Osu,
            GameModeChoice::Mania => GameMode::Mania,
            GameModeChoice::Taiko => GameMode::Taiko,
            GameModeChoice::Catch => GameMode::Catch,
        }
    }
}

#[lumi::command(slash_command, prefix_command, guild_only)]
pub async fn osu(
    ctx: Context<'_>,
    user: Option<UserId>,
    gamemode: Option<GameModeChoice>,
) -> Result<(), Error> {
    let user = match ctx {
        lumi::Context::Application(_) => user.unwrap_or(ctx.author().id),
        lumi::Context::Prefix(prefix_context) => user
            .or_else(|| {
                prefix_context
                    .msg
                    .referenced_message
                    .as_ref()
                    .map(|m| m.author.id)
            })
            .unwrap_or(ctx.author().id),
    };

    let Some((osu_id, preferred_gamemode)) = ctx.data().database.get_osu_user_id(user).await else {
        ctx.say("User is not verified!").await?;
        return Ok(());
    };

    let gamemode = gamemode.map_or(preferred_gamemode, std::convert::Into::into);

    let Ok(user) = ctx.data().web.osu.user(osu_id).mode(gamemode).await else {
        ctx.say("Cannot fetch osu user. Restricted?").await?;
        return Ok(());
    };

    let stats = user.statistics.expect("always sent");

    // TODO: CV2?
    let embed = CreateEmbed::new()
        .author(
            CreateEmbedAuthor::new(user.username.as_str())
                .url(format!("https://osu.ppy.sh/u/{}", user.user_id)),
        )
        .thumbnail(user.avatar_url)
        .description(format!(
            "**Level** {} | **Global Rank** {} | **:flag_{}: Rank** {}\n\n**PP** {} | \
             **Accuracy** {:.2} | **Play Count** {}",
            stats.level.current,
            stats.global_rank.unwrap_or(0),
            user.country_code.to_lowercase(),
            stats.country_rank.unwrap(),
            stats.pp.round(),
            stats.accuracy,
            stats.playcount
        ))
        .colour(Colour::FADED_PURPLE);

    ctx.send(CreateReply::new().embed(embed)).await?;

    Ok(())
}

#[lumi::command(aliases("mode"), slash_command, prefix_command, guild_only)]
pub async fn gamemode(ctx: Context<'_>, gamemode: GameModeChoice) -> Result<(), Error> {
    if ctx
        .data()
        .database
        .get_osu_user_id(ctx.author().id)
        .await
        .is_none()
    {
        ctx.say("You are not verified!").await?;
        return Ok(());
    }

    ctx.data()
        .database
        .change_mode(ctx.author().id, gamemode.into())
        .await?;

    ctx.say("Successfully changed mode.").await?;

    Ok(())
}

#[must_use]
pub fn commands() -> [crate::Command; 5] {
    [verify(), update(), gamemode(), unlink(), osu()]
}
