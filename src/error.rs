use ::serenity::{all::CreateAllowedMentions, small_fixed_array::FixedString};
use lumi::{CreateReply, serenity_prelude as serenity};
use moth_commands::utils::{handle_cooldown, prefix_bot_perms};
use moth_core::data::structs::{Context, Data, Error, InvocationData};

async fn handle_command_error(ctx: Context<'_>, error: Error) {
    if let Some(invocation_data) = ctx.invocation_data::<InvocationData>().await
        && let Some(duration) = invocation_data.cooldown_remaining
    {
        let _ = handle_cooldown(duration, ctx).await;
        return;
    }

    // surely this isn't a bad idea?
    let _ = ctx
        .send(
            CreateReply::new()
                .content(error.to_string())
                .ephemeral(true),
        )
        .await;
    println!("Error in command `{}`: {:?}", ctx.command().name, error);
}

async fn handle_command_check_failed(ctx: Context<'_>, error: Option<Error>) {
    async fn text_response(ctx: Context<'_>, error: Option<Error>) {
        let mut embed = serenity::CreateEmbed::new()
            .title("You do not have permission to access this command.")
            .colour(serenity::Colour::RED);
        if let Some(err) = error {
            embed = embed.description(err.to_string());
        }
        let msg = lumi::CreateReply::new().embed(embed);
        let _ = ctx.send(msg).await;
    }

    match ctx {
        lumi::Context::Application(_) => text_response(ctx, error).await,
        lumi::Context::Prefix(pctx) => {
            if let Ok(permissions) = prefix_bot_perms(pctx).await {
                if permissions.send_messages() {
                    text_response(ctx, error).await;
                } else if permissions.add_reactions() {
                    let _ = pctx
                        .msg
                        .react(
                            ctx.http(),
                            serenity::ReactionType::Unicode(FixedString::from_static_trunc("💢")),
                        )
                        .await;
                }
            }
        }
    }
}

async fn handle_argument_parse_error(ctx: Context<'_>, input: Option<String>, error: Error) {
    async fn text_response(ctx: Context<'_>, input: Option<String>, error: Error) {
        let usage = ctx
            .command()
            .help_text
            .as_deref()
            .unwrap_or("Please check the help menu for usage information");
        let response = match input {
            Some(input) => format!("**Cannot parse `{input}` as argument: {error}**\n{usage}"),
            None => format!("**{error}**\n{usage}"),
        };
        let mentions = CreateAllowedMentions::new()
            .everyone(false)
            .all_roles(false)
            .all_users(false);
        let _ = ctx
            .send(
                lumi::CreateReply::default()
                    .content(response)
                    .allowed_mentions(mentions),
            )
            .await;
    }

    match ctx {
        lumi::Context::Application(_) => text_response(ctx, input, error).await,
        lumi::Context::Prefix(pctx) => {
            if let Ok(permissions) = prefix_bot_perms(pctx).await {
                if permissions.send_messages() {
                    text_response(ctx, input, error).await;
                } else if permissions.add_reactions() {
                    let _ = pctx
                        .msg
                        .react(
                            ctx.http(),
                            serenity::ReactionType::Unicode(FixedString::from_static_trunc("❓")),
                        )
                        .await;
                }
            }
        }
    }
}

pub async fn handler(error: lumi::FrameworkError<'_, Data, Error>) {
    match error {
        lumi::FrameworkError::Command { error, ctx, .. } => handle_command_error(ctx, error).await,
        lumi::FrameworkError::NotAnOwner { .. } | lumi::FrameworkError::UnknownCommand { .. } => {}
        lumi::FrameworkError::CommandCheckFailed { error, ctx, .. } => {
            handle_command_check_failed(ctx, error).await;
        }
        lumi::FrameworkError::ArgumentParse {
            error, input, ctx, ..
        } => handle_argument_parse_error(ctx, input, error).await,
        lumi::FrameworkError::CooldownHit {
            remaining_cooldown,
            ctx,
            ..
        } => {
            let _ = handle_cooldown(remaining_cooldown, ctx).await;
        }
        error => {
            if let Err(e) = lumi::builtins::on_error(error).await {
                println!("Error while handling error: {e}");
            }
        }
    }
}
