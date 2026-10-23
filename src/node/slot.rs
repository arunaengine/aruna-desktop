//! Who owns the node's run slot. The slot stays taken until the run loop has
//! ended, so a stop, a quit or a start can never overtake a child that is
//! still shutting down.

use tokio::sync::{oneshot, watch};

use super::NodeStatus;

#[derive(Default)]
pub struct Slot {
    stop: Option<oneshot::Sender<()>>,
    done: Option<watch::Receiver<()>>,
    /// Installed in the instant the slot frees, so no start comes in between.
    blocked: Option<NodeStatus>,
}

/// What the run loop holds: the stop signal, and a sender whose drop tells
/// every waiting stop that the run is over.
pub struct Claim {
    pub stop: oneshot::Receiver<()>,
    pub done: watch::Sender<()>,
}

impl Slot {
    pub fn busy(&self) -> bool {
        self.done.is_some()
    }

    /// `None` while another run owns the slot.
    pub fn claim(&mut self) -> Option<Claim> {
        if self.busy() {
            return None;
        }
        let (stop_sender, stop) = oneshot::channel();
        let (done, done_receiver) = watch::channel(());
        self.stop = Some(stop_sender);
        self.done = Some(done_receiver);
        Some(Claim { stop, done })
    }

    /// Signals the run once and hands back what waits for its end, which every
    /// caller gets, not only the first. `blocked` is the status to install when
    /// the slot frees. `None` means no run exists.
    pub fn request_stop(&mut self, blocked: Option<NodeStatus>) -> Option<watch::Receiver<()>> {
        let done = self.done.clone()?;
        if blocked.is_some() {
            self.blocked = blocked;
        }
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        Some(done)
    }

    /// Frees the slot and returns the status that must replace the run's own.
    pub fn release(&mut self) -> Option<NodeStatus> {
        self.stop = None;
        self.done = None;
        self.blocked.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::NodeState;

    fn blocked() -> NodeStatus {
        NodeStatus {
            state: NodeState::Stopped,
            detail: Some("blocked".into()),
            ..NodeStatus::default()
        }
    }

    #[test]
    fn holds_one_run() {
        let mut slot = Slot::default();
        assert!(!slot.busy());
        let claim = slot.claim().expect("a free slot");
        assert!(slot.busy());
        assert!(slot.claim().is_none());

        // A stop leaves the slot taken: only the run ending frees it.
        let _ = slot.request_stop(None);
        assert!(slot.claim().is_none());

        slot.release();
        drop(claim);
        assert!(slot.claim().is_some());
    }

    #[test]
    fn signals_the_run_once() {
        let mut slot = Slot::default();
        let mut claim = slot.claim().expect("a free slot");
        assert!(slot.request_stop(None).is_some());
        assert_eq!(claim.stop.try_recv(), Ok(()));
        assert!(slot.request_stop(None).is_some());
        assert!(Slot::default().request_stop(None).is_none());
    }

    #[tokio::test]
    async fn waits_for_the_end() {
        // A second stop, such as a quit during a stop, must not return early.
        let mut slot = Slot::default();
        let claim = slot.claim().expect("a free slot");
        let mut first = slot.request_stop(None).expect("a run");
        let mut second = slot.request_stop(None).expect("a run");

        let waiting = tokio::spawn(async move {
            let _ = first.changed().await;
            let _ = second.changed().await;
        });
        assert!(!waiting.is_finished());
        slot.release();
        drop(claim);
        waiting.await.expect("both stops end with the run");
    }

    #[test]
    fn installs_blocked_on_release() {
        let mut slot = Slot::default();
        let _claim = slot.claim().expect("a free slot");
        assert!(slot.request_stop(Some(blocked())).is_some());
        // A later plain stop keeps the status that was asked for.
        assert!(slot.request_stop(None).is_some());

        let installed = slot.release().expect("the blocked status");
        assert_eq!(installed.detail.as_deref(), Some("blocked"));
        assert!(slot.release().is_none());
        assert!(!slot.busy());
    }
}
