use std::sync::Arc;

use rosu_v2::model::GameMode;
use serenity::all::UserId;
use tokio::sync::mpsc::UnboundedSender;

pub enum VerificationCommand {
    Link((serenity::all::UserId, u32, GameMode)),
    Unlink((serenity::all::UserId, u32)),
    // changes gamemode, assigns new metadata and recalcs.
    GameModeChange(serenity::all::UserId, GameMode),
    Shutdown,
}

#[derive(Default, Clone)]
pub struct VerificationSender {
    sender: Arc<tokio::sync::Mutex<Option<UnboundedSender<VerificationCommand>>>>,
}

impl VerificationSender {
    pub async fn shutdown(&self) {
        let lock = self.sender.lock().await;

        lock.as_ref().map(|s| s.send(VerificationCommand::Shutdown));
    }

    pub async fn verify(&self, user_id: UserId, osu_id: u32, gamemode: GameMode) {
        let lock = self.sender.lock().await;

        lock.as_ref()
            .map(|s| s.send(VerificationCommand::Link((user_id, osu_id, gamemode))));
    }

    pub async fn unverify(&self, user_id: UserId, osu_id: u32) {
        let lock = self.sender.lock().await;

        lock.as_ref()
            .map(|s| s.send(VerificationCommand::Unlink((user_id, osu_id))));
    }

    pub async fn gamemode_change(&self, user_id: UserId, gamemode: GameMode) {
        let lock = self.sender.lock().await;

        lock.as_ref()
            .map(|s| s.send(VerificationCommand::GameModeChange(user_id, gamemode)));
    }

    /// Sets the sender to the provided `UnboundedSender`.
    pub async fn set(&self, tx: UnboundedSender<VerificationCommand>) {
        *self.sender.lock().await = Some(tx);
    }

    #[must_use]
    pub fn new(tx: Option<UnboundedSender<VerificationCommand>>) -> Self {
        Self {
            sender: Arc::new(tokio::sync::Mutex::new(tx)),
        }
    }
}
