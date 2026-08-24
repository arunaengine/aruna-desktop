use serde::Serialize;

use crate::node::NodeState;

/// How far the enrollment has come. The variants are ordered, so progress is
/// monotonic and a flickering probe cannot walk the view backwards.
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum EnrollPhase {
    /// Nothing enrolled and no invitation waiting.
    #[default]
    Idle,
    /// An invitation was accepted; its secret is stored.
    Received,
    /// The node is restarting with the enrollment environment.
    Starting,
    /// The node answers on the ops port, which it only does once the seed has
    /// accepted the secret and the join finished.
    Waiting,
    /// The node serves the realm the invitation named.
    Visible,
    /// The secret is spent and cleared.
    Done,
    /// The secret was refused, or the node cannot stay up.
    Failed,
}

/// One round of observations: the supervisor's state, the ops readiness probe,
/// the realm the node serves, and whatever the node printed about the join.
#[derive(Clone, Copy, Debug, Default)]
pub struct Observed {
    pub state: NodeState,
    pub ready: bool,
    pub in_realm: bool,
    pub refusal: Option<&'static str>,
}

/// The enrollment state machine. It holds no secret: the seed and the realm are
/// public, and the secret lives only in the settings file and the node's env.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollProgress {
    pub phase: EnrollPhase,
    pub realm: Option<String>,
    pub seed: Option<String>,
    /// Why the enrollment failed, once it has.
    pub detail: Option<String>,
}

impl EnrollProgress {
    /// Starts a fresh run for an accepted invitation, discarding any earlier
    /// outcome so a retry is not stuck on the previous failure.
    pub fn accept(&mut self, seed: &str, realm: Option<&str>) {
        self.phase = EnrollPhase::Received;
        self.seed = Some(seed.to_string());
        self.realm = realm.map(str::to_string);
        self.detail = None;
    }

    /// Marks an already enrolled node, so a later launch does not re-run a
    /// progression it has no secret for.
    pub fn settle(&mut self, seed: Option<String>, realm: Option<String>) {
        self.phase = EnrollPhase::Done;
        self.seed = seed;
        self.realm = realm;
        self.detail = None;
    }

    pub fn fail(&mut self, detail: impl Into<String>) {
        self.phase = EnrollPhase::Failed;
        self.detail = Some(detail.into());
    }

    /// True while observations still matter, so the view can stop polling.
    pub fn active(&self) -> bool {
        matches!(
            self.phase,
            EnrollPhase::Received | EnrollPhase::Starting | EnrollPhase::Waiting
        )
    }

    /// Folds one round of observations into the phase. Failures win; otherwise
    /// the phase only ever moves forward.
    pub fn observe(&mut self, seen: Observed) {
        if !self.active() {
            return;
        }
        if let Some(refusal) = seen.refusal {
            self.fail(refusal);
            return;
        }
        match seen.state {
            NodeState::BinaryMissing => {
                self.fail("no node binary to enroll with");
                return;
            }
            NodeState::Failed => {
                self.fail("the node could not stay up long enough to enroll");
                return;
            }
            _ => {}
        }

        let reached = if seen.in_realm {
            EnrollPhase::Visible
        } else if seen.ready {
            EnrollPhase::Waiting
        } else {
            EnrollPhase::Starting
        };
        self.phase = self.phase.max(reached);
    }

    /// Called once the spent secret has been cleared from the settings.
    pub fn finish(&mut self) {
        if self.phase == EnrollPhase::Visible {
            self.phase = EnrollPhase::Done;
            self.detail = None;
        }
    }
}

/// Backend-side bookkeeping: the progress plus how far the refusal scan has
/// read the node's output.
#[derive(Debug, Default)]
pub struct Tracker {
    pub progress: EnrollProgress,
    pub cursor: u64,
}

/// Turns one line of the node's output into a targeted enrollment failure.
///
/// The joiner logs nothing through tracing about the join: it prints a single
/// line to stderr and exits, so these are matched as plain text. `None` means
/// the line says nothing about the enrollment.
pub fn refusal(line: &str) -> Option<&'static str> {
    const BOOTSTRAP: &str = "onboarding bootstrap failed:";

    if line.contains("onboarding mode mismatch") {
        return Some("the secret was minted for a different kind of node");
    }
    if line.contains("missing onboarding bootstrap material") {
        return Some("the seed sent no bootstrap material for this kind of node");
    }
    if line.contains("waiting for onboarding placement") {
        return Some("the realm accepted the device but never placed it");
    }
    if !line.contains(BOOTSTRAP) {
        return None;
    }
    if line.contains("requires ONBOARDING_SECRET") {
        return Some("the previous join was interrupted; enroll again with a fresh secret");
    }
    if line.contains("401") {
        return Some("the seed refused the secret: it is expired, already used, or not valid");
    }
    if line.contains("409") {
        return Some("the realm will not take another device for this owner");
    }
    Some("the seed refused the enrollment")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started() -> EnrollProgress {
        let mut progress = EnrollProgress::default();
        progress.accept("https://seed.example", Some("realm-1"));
        progress
    }

    #[test]
    fn walks_the_progression() {
        let mut progress = started();
        progress.observe(Observed {
            state: NodeState::Starting,
            ..Observed::default()
        });
        assert_eq!(progress.phase, EnrollPhase::Starting);

        progress.observe(Observed {
            state: NodeState::Running,
            ready: true,
            ..Observed::default()
        });
        assert_eq!(progress.phase, EnrollPhase::Waiting);

        progress.observe(Observed {
            state: NodeState::Running,
            ready: true,
            in_realm: true,
            ..Observed::default()
        });
        assert_eq!(progress.phase, EnrollPhase::Visible);

        progress.finish();
        assert_eq!(progress.phase, EnrollPhase::Done);
    }

    #[test]
    fn keeps_progress_monotonic() {
        // A readiness probe that blips must not walk the view back a step.
        let mut progress = started();
        progress.observe(Observed {
            state: NodeState::Running,
            ready: true,
            ..Observed::default()
        });
        progress.observe(Observed {
            state: NodeState::Restarting,
            ..Observed::default()
        });
        assert_eq!(progress.phase, EnrollPhase::Waiting);
    }

    #[test]
    fn fails_on_refusal() {
        let mut progress = started();
        progress.observe(Observed {
            state: NodeState::Running,
            ready: true,
            refusal: Some("refused"),
            ..Observed::default()
        });
        assert_eq!(progress.phase, EnrollPhase::Failed);
        assert_eq!(progress.detail.as_deref(), Some("refused"));
    }

    #[test]
    fn fails_when_node_gives_up() {
        let mut progress = started();
        progress.observe(Observed {
            state: NodeState::Failed,
            ..Observed::default()
        });
        assert_eq!(progress.phase, EnrollPhase::Failed);
    }

    #[test]
    fn ignores_late_observations() {
        // Nothing after a terminal phase may reopen the run.
        let mut progress = started();
        progress.fail("refused");
        progress.observe(Observed {
            state: NodeState::Running,
            ready: true,
            in_realm: true,
            ..Observed::default()
        });
        assert_eq!(progress.phase, EnrollPhase::Failed);
    }

    #[test]
    fn retry_clears_the_failure() {
        let mut progress = started();
        progress.fail("refused");
        progress.accept("https://seed.example", Some("realm-1"));
        assert_eq!(progress.phase, EnrollPhase::Received);
        assert!(progress.detail.is_none());
    }

    #[test]
    fn reads_the_node_refusal() {
        // The exact stderr lines the aruna joiner prints before it exits.
        assert_eq!(
            refusal("onboarding bootstrap failed: bootstrap endpoint returned 401 Unauthorized"),
            Some("the seed refused the secret: it is expired, already used, or not valid")
        );
        assert_eq!(
            refusal("onboarding bootstrap failed: bootstrap endpoint returned 409 Conflict"),
            Some("the realm will not take another device for this owner")
        );
        assert_eq!(
            refusal("onboarding mode mismatch between secret and bootstrap response"),
            Some("the secret was minted for a different kind of node")
        );
        assert!(
            refusal(
                "onboarding bootstrap failed: pending bootstrapped onboarding requires \
                 ONBOARDING_SECRET to refresh bootstrap material"
            )
            .is_some()
        );
        assert!(refusal("onboarding bootstrap failed: connection refused").is_some());
    }

    #[test]
    fn ignores_unrelated_output() {
        assert!(refusal("INFO Ops server listening ops_address=127.0.0.1:34117").is_none());
        assert!(refusal("").is_none());
    }
}
