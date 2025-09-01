use std::sync::Arc;

use serenity::{
    all::{ChannelId, Colour, Context, CreateEmbed, CreateMessage, GenericChannelId, UserId},
    futures::FutureExt,
    small_fixed_array::FixedString,
};
use tokio::sync::mpsc::UnboundedSender;

pub mod interactions;
pub mod task;

#[derive(Default)]
pub struct PrivateVcHandler {
    pub sender: Sender,
}

impl PrivateVcHandler {
    /// Starts the background task, will run regardless of if an existing task is running (will not be dropped)
    ///
    /// Populates the task sender.
    pub async fn start_background_task(&self, ctx: Context) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.sender.set(tx).await;

        tokio::spawn(async move {
            let http = ctx.http.clone();
            let result = std::panic::AssertUnwindSafe(task::start(ctx, rx))
                .catch_unwind()
                .await;

            if let Err(e) = result {
                let trace = if let Some(s) = e.downcast_ref::<&str>() {
                    Some((*s).to_string())
                } else if let Ok(s) = e.downcast::<String>() {
                    Some(*s)
                } else {
                    None
                };

                let _ = GenericChannelId::new(158484765136125952)
                    .send_message(
                        &http,
                        CreateMessage::new()
                            .content("<@291089948709486593> I'M PANICKING HELP")
                            .embed(CreateEmbed::new().colour(Colour::RED).description(
                                trace.unwrap_or(String::from("IDK WHATS WRONG WITH ME")),
                            )),
                    )
                    .await;
            }
        });
    }
}

#[derive(Default, Clone)]
pub struct Sender(Arc<tokio::sync::Mutex<Option<UnboundedSender<HandlerCommand>>>>);

impl Sender {
    pub async fn shutdown(&self) {
        let lock = self.0.lock().await;

        lock.as_ref()
            .map(|s: &UnboundedSender<HandlerCommand>| s.send(HandlerCommand::Shutdown));
    }

    /// Sets the sender to the provided `UnboundedSender`.
    async fn set(&self, tx: UnboundedSender<HandlerCommand>) {
        *self.0.lock().await = Some(tx);
    }

    pub async fn join(&self, channel_id: ChannelId, user_id: UserId, username: FixedString<u8>) {
        let lock = self.0.lock().await;

        lock.as_ref().map(|s: &UnboundedSender<HandlerCommand>| {
            s.send(HandlerCommand::JoinSpecial((channel_id, user_id, username)))
        });
    }

    pub async fn leave(&self, channel_id: ChannelId, user_id: UserId) {
        let lock = self.0.lock().await;

        lock.as_ref().map(|s: &UnboundedSender<HandlerCommand>| {
            s.send(HandlerCommand::LeaveVc((channel_id, user_id)))
        });
    }

    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(tokio::sync::Mutex::new(None)))
    }
}

enum HandlerCommand {
    /// Should be triggered when a user either joins the creation channel, or a private vc.
    JoinSpecial((ChannelId, UserId, FixedString<u8>)),
    LeaveVc((ChannelId, UserId)),
    Shutdown,
}

impl Drop for PrivateVcHandler {
    fn drop(&mut self) {
        let sender = self.sender.clone();
        tokio::spawn(async move { sender.shutdown().await });
    }
}
