use moth_core::data::{
    database::private_vcs::task::{GUILD, VC_CHANNEL},
    structs::Data,
};
use serenity::all::{Context, User, VoiceState};

/// Check if a user is in the channel creation channel, and moves them if they are.
pub async fn check_channel(
    ctx: &Context,
    old_state: Option<&VoiceState>,
    new_state: &VoiceState,
    user: Option<&User>,
) {
    if new_state.guild_id != Some(GUILD) {
        return;
    }

    let (joined_channel_id, left_channel_id) = match old_state {
        Some(old) => {
            let joined = match (old.channel_id, new_state.channel_id) {
                (Some(old), Some(new)) if old != new => Some(new),
                (None, Some(new)) => Some(new),
                _ => None,
            };

            let left = match (old.channel_id, new_state.channel_id) {
                (Some(old), Some(new)) if old != new => Some(old),
                (Some(old), None) => Some(old),
                _ => None,
            };

            (joined, left)
        }
        None => (new_state.channel_id, None),
    };

    let data = ctx.data_ref::<Data>();

    // Handle joins
    if let Some(joined) = joined_channel_id
        && (data
            .database
            .get_private_vc(joined, Some(GUILD))
            .await
            .is_some()
            || joined == VC_CHANNEL)
    {
        let Some(user) = user else { return };

        data.private_vc
            .sender
            .join(
                joined,
                new_state.user_id,
                small_fixed_array::FixedString::from_str_trunc(user.display_name()),
            )
            .await;
    }

    // Handle leaves
    if let Some(left) = left_channel_id
        && (data
            .database
            .get_private_vc(left, Some(GUILD))
            .await
            .is_some()
            || left == VC_CHANNEL)
    {
        data.private_vc.sender.leave(left, new_state.user_id).await;
    }
}
