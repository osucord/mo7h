use crate::{Context, Error};

#[lumi::command(
    prefix_command,
    hide_in_help,
    aliases("t", "talkmore"),
    member_cooldown = "300"
)]
pub async fn talk(ctx: Context<'_>) -> Result<(), Error> {
    ctx.say(
        "https://cdn.discordapp.com/attachments/780131105725480972/792982063707848754/talkmore.gif",
    )
    .await?;
    Ok(())
}

#[lumi::command(
    prefix_command,
    hide_in_help,
    aliases("p", "playmore"),
    member_cooldown = "300"
)]
pub async fn play(ctx: Context<'_>) -> Result<(), Error> {
    ctx.say(
        "https://cdn.discordapp.com/attachments/158484765136125952/740942824341766316/play_more.gif",
    )
    .await?;
    Ok(())
}

#[lumi::command(
    prefix_command,
    hide_in_help,
    aliases("pl", "playless"),
    member_cooldown = "300"
)]
pub async fn playless(ctx: Context<'_>) -> Result<(), Error> {
    ctx.say(
        "https://cdn.discordapp.com/attachments/747469894639157392/1424042070565326878/play_less.gif",
    )
    .await?;
    Ok(())
}

#[must_use]
pub fn commands() -> [crate::Command; 3] {
    [talk(), play(), playless()]
}
