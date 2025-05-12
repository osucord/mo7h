use std::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicU8, Ordering},
    task::{Context, Poll},
    time::Duration,
};

use dashmap::DashMap;
use rosu_v2::prelude::UserExtended;
use serenity::futures::FutureExt;
use tokio::{
    sync::oneshot::{self, Receiver, Sender},
    time::{self, Timeout},
};

const DEADLINE: Duration = Duration::from_secs(120);

pub enum AuthenticationStandbyError {
    Canceled,
    Timeout,
}

#[derive(Default)]
pub struct AuthenticationStandby {
    current_state: AtomicU8,
    osu: DashMap<u8, Sender<UserExtended>>,
}

impl AuthenticationStandby {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Wait for an osu! username to be authenticated.
    pub fn wait_for_osu(&self) -> WaitForOsuAuth {
        let (tx, rx) = oneshot::channel();
        let state = self.generate_state();
        let fut = Box::pin(time::timeout(DEADLINE, rx));
        self.osu.insert(state, tx);

        WaitForOsuAuth { state, fut }
    }

    fn generate_state(&self) -> u8 {
        self.current_state.fetch_add(1, Ordering::SeqCst)
    }

    pub fn process_osu(&self, user: UserExtended, state: u8) {
        if let Some((_, tx)) = self.osu.remove(&state) {
            let _ = tx.send(user);
        }
    }
}

pub struct WaitForOsuAuth {
    pub state: u8,
    fut: Pin<Box<Timeout<Receiver<UserExtended>>>>,
}

impl Future for WaitForOsuAuth {
    type Output = Result<UserExtended, AuthenticationStandbyError>;

    #[inline]
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.fut.poll_unpin(cx) {
            Poll::Ready(Ok(Ok(user))) => Poll::Ready(Ok(user)),
            Poll::Ready(Ok(Err(_))) => Poll::Ready(Err(AuthenticationStandbyError::Canceled)),
            Poll::Ready(Err(_)) => Poll::Ready(Err(AuthenticationStandbyError::Timeout)),
            Poll::Pending => Poll::Pending,
        }
    }
}
