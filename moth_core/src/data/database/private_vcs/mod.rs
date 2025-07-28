use std::sync::Arc;

use serenity::{
    all::{ChannelId, Context, UserId},
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
            task::start(ctx, rx).await;
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
