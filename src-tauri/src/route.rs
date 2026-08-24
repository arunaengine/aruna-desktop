use serde::Serialize;

/// The view a launch opens on.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Landing {
    /// Nothing is remembered yet, so the first thing to do is name a realm.
    Welcome,
    Status,
    Enroll,
}

/// A waiting invitation, or an enrollment still running, wins: it is what the
/// owner is in the middle of. Otherwise a remembered realm means the setup is
/// done and the node is what is left to watch.
pub fn landing(realm: Option<&str>, invited: bool, enrolling: bool) -> Landing {
    if invited || enrolling {
        Landing::Enroll
    } else if realm.is_some() {
        Landing::Status
    } else {
        Landing::Welcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lands_on_welcome() {
        assert_eq!(landing(None, false, false), Landing::Welcome);
    }

    #[test]
    fn lands_on_status() {
        assert_eq!(
            landing(Some("https://aruna.example"), false, false),
            Landing::Status
        );
    }

    #[test]
    fn lands_on_enroll() {
        // Both an unanswered invitation and a running join outrank the realm.
        assert_eq!(landing(None, true, false), Landing::Enroll);
        assert_eq!(
            landing(Some("https://aruna.example"), false, true),
            Landing::Enroll
        );
    }
}
