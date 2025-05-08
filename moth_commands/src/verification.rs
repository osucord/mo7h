use crate::{Context, Error};

use lumi::CreateReply;

use ::serenity::all::{Colour, CreateEmbed, CreateEmbedFooter};
use moth_core::verification::update_roles;
use serenity::all::CreateMessage;

#[lumi::command(slash_command, hide_in_help, guild_only)]
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

#[must_use]
pub fn commands() -> [crate::Command; 1] {
    [verify()]
}
