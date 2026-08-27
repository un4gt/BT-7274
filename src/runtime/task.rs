//! 运行时任务取消原语。所有长任务共享同一显式 token，而不是只依赖 JoinHandle abort。

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CancellationReason {
    User,
    ApplicationExit,
    SessionChanged,
    SessionRemoved,
    #[default]
    Unknown,
}

impl CancellationReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::ApplicationExit => "application_exit",
            Self::SessionChanged => "session_changed",
            Self::SessionRemoved => "session_removed",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    inner: Arc<CancellationInner>,
}

#[derive(Debug, Default)]
struct CancellationInner {
    cancelled: AtomicBool,
    reason: Mutex<Option<CancellationReason>>,
    notify: Notify,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancel_with(CancellationReason::Unknown);
    }

    pub fn cancel_with(&self, reason: CancellationReason) {
        let mut stored_reason = self
            .inner
            .reason
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !self.inner.cancelled.swap(true, Ordering::AcqRel) {
            *stored_reason = Some(reason);
            drop(stored_reason);
            self.inner.notify.notify_waiters();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    pub fn reason(&self) -> CancellationReason {
        self.inner
            .reason
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .unwrap_or_default()
    }

    pub async fn cancelled(&self) {
        loop {
            if self.is_cancelled() {
                return;
            }
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancellation_is_shared_and_idempotent() {
        let first = CancellationToken::new();
        let second = first.clone();
        first.cancel();
        first.cancel();
        second.cancelled().await;
        assert!(second.is_cancelled());
        assert_eq!(second.reason(), CancellationReason::Unknown);
    }

    #[tokio::test]
    async fn first_cancellation_reason_wins() {
        let token = CancellationToken::new();
        token.cancel_with(CancellationReason::User);
        token.cancel_with(CancellationReason::ApplicationExit);
        token.cancelled().await;
        assert_eq!(token.reason(), CancellationReason::User);
    }

    #[tokio::test]
    async fn registered_waiters_are_all_released() {
        let token = CancellationToken::new();
        let first = tokio::spawn({
            let token = token.clone();
            async move { token.cancelled().await }
        });
        let second = tokio::spawn({
            let token = token.clone();
            async move { token.cancelled().await }
        });
        tokio::task::yield_now().await;
        token.cancel_with(CancellationReason::User);
        tokio::time::timeout(std::time::Duration::from_secs(1), first)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), second)
            .await
            .unwrap()
            .unwrap();
    }
}
