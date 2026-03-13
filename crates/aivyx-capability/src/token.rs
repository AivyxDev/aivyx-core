use aivyx_core::{CapabilityId, Principal};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::pattern::ActionPattern;
use crate::scope::CapabilityScope;

/// An unforgeable capability token.
///
/// Capabilities are the sole mechanism for authorising actions. New
/// capabilities can only be derived via [`attenuate`](Self::attenuate), which
/// structurally guarantees that the child is never broader than the parent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    /// Unique identifier for this capability token.
    pub id: CapabilityId,
    /// The domain this capability governs.
    pub scope: CapabilityScope,
    /// Glob pattern specifying which actions are allowed.
    pub pattern: ActionPattern,
    /// Principals authorized to exercise this capability.
    pub granted_to: Vec<Principal>,
    /// The principal that created this capability.
    pub granted_by: Principal,
    /// When this capability was created.
    pub created_at: DateTime<Utc>,
    /// Optional expiration time; `None` means no expiry.
    pub expires_at: Option<DateTime<Utc>>,
    /// Whether this capability has been explicitly revoked.
    pub revoked: bool,
    /// The capability this was attenuated from, if any.
    pub parent_id: Option<CapabilityId>,
}

impl Capability {
    /// Derive a strictly narrower child capability.
    ///
    /// Returns `None` if any of the requested parameters would broaden the
    /// parent's authority:
    /// - `new_scope` must be a subset of the current scope
    /// - `new_pattern` must be a subset of the current pattern
    /// - `new_principals` must be a subset of the current granted_to
    /// - `new_expiry` (if provided) must not extend past the parent's expiry
    ///
    /// The parent must also be valid (not revoked, not expired).
    pub fn attenuate(
        &self,
        new_scope: CapabilityScope,
        new_pattern: ActionPattern,
        new_principals: Vec<Principal>,
        new_expiry: Option<DateTime<Utc>>,
    ) -> Option<Capability> {
        // Cannot attenuate an invalid capability.
        if !self.is_valid() {
            return None;
        }

        // Scope must narrow.
        self.scope.attenuate(&new_scope)?;

        // Pattern must narrow.
        if !new_pattern.is_subset_of(&self.pattern) {
            return None;
        }

        // Principals must be a subset.
        if !new_principals.iter().all(|p| self.granted_to.contains(p)) {
            return None;
        }

        // Expiry must not extend beyond parent.
        if let Some(parent_exp) = self.expires_at {
            match new_expiry {
                Some(child_exp) if child_exp > parent_exp => return None,
                Some(_) => {}        // child expires at or before parent — ok
                None => return None, // removing expiry would broaden
            }
        }

        Some(Capability {
            id: CapabilityId::new(),
            scope: new_scope,
            pattern: new_pattern,
            granted_to: new_principals,
            granted_by: self.granted_by.clone(),
            created_at: Utc::now(),
            expires_at: new_expiry,
            revoked: false,
            parent_id: Some(self.id),
        })
    }

    /// A capability is valid when it has not been revoked and has not expired.
    pub fn is_valid(&self) -> bool {
        if self.revoked {
            return false;
        }
        if let Some(exp) = self.expires_at
            && Utc::now() >= exp
        {
            return false;
        }
        true
    }

    /// Check whether `principal` is authorised to perform `action` under this
    /// capability.
    pub fn check(&self, principal: &Principal, action: &str) -> bool {
        self.is_valid() && self.granted_to.contains(principal) && self.pattern.matches(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivyx_core::AgentId;
    use chrono::Duration;
    use std::path::PathBuf;

    fn make_cap(
        scope: CapabilityScope,
        pattern: &str,
        expires: Option<DateTime<Utc>>,
    ) -> Capability {
        Capability {
            id: CapabilityId::new(),
            scope,
            pattern: ActionPattern::new(pattern).unwrap(),
            granted_to: vec![Principal::User("alice".into())],
            granted_by: Principal::System,
            created_at: Utc::now(),
            expires_at: expires,
            revoked: false,
            parent_id: None,
        }
    }

    #[test]
    fn attenuate_narrows_scope() {
        let cap = make_cap(
            CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            "*",
            None,
        );
        let child = cap.attenuate(
            CapabilityScope::Filesystem {
                root: PathBuf::from("/home/alice"),
            },
            ActionPattern::new("read:*").unwrap(),
            vec![Principal::User("alice".into())],
            None,
        );
        assert!(child.is_some());
        let child = child.unwrap();
        assert_eq!(child.parent_id, Some(cap.id));
    }

    #[test]
    fn attenuate_rejects_broader_scope() {
        let cap = make_cap(
            CapabilityScope::Filesystem {
                root: PathBuf::from("/home/alice"),
            },
            "*",
            None,
        );
        let child = cap.attenuate(
            CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            ActionPattern::new("*").unwrap(),
            vec![Principal::User("alice".into())],
            None,
        );
        assert!(child.is_none());
    }

    #[test]
    fn attenuate_rejects_broader_pattern() {
        let cap = make_cap(
            CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            "read:*",
            None,
        );
        let child = cap.attenuate(
            CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            ActionPattern::new("*").unwrap(),
            vec![Principal::User("alice".into())],
            None,
        );
        assert!(child.is_none());
    }

    #[test]
    fn attenuate_rejects_extra_principal() {
        let cap = make_cap(
            CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            "*",
            None,
        );
        let child = cap.attenuate(
            CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            ActionPattern::new("*").unwrap(),
            vec![
                Principal::User("alice".into()),
                Principal::User("eve".into()),
            ],
            None,
        );
        assert!(child.is_none());
    }

    #[test]
    fn attenuate_rejects_extended_expiry() {
        let soon = Utc::now() + Duration::hours(1);
        let later = Utc::now() + Duration::hours(2);
        let cap = make_cap(
            CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            "*",
            Some(soon),
        );
        let child = cap.attenuate(
            CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            ActionPattern::new("*").unwrap(),
            vec![Principal::User("alice".into())],
            Some(later),
        );
        assert!(child.is_none());
    }

    #[test]
    fn attenuate_rejects_removing_expiry() {
        let soon = Utc::now() + Duration::hours(1);
        let cap = make_cap(
            CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            "*",
            Some(soon),
        );
        let child = cap.attenuate(
            CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            ActionPattern::new("*").unwrap(),
            vec![Principal::User("alice".into())],
            None, // removing expiry would broaden
        );
        assert!(child.is_none());
    }

    #[test]
    fn attenuate_revoked_cap_fails() {
        let mut cap = make_cap(
            CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            "*",
            None,
        );
        cap.revoked = true;
        let child = cap.attenuate(
            CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            ActionPattern::new("*").unwrap(),
            vec![Principal::User("alice".into())],
            None,
        );
        assert!(child.is_none());
    }

    #[test]
    fn is_valid_not_revoked_not_expired() {
        let cap = make_cap(
            CapabilityScope::Calendar,
            "*",
            Some(Utc::now() + Duration::hours(1)),
        );
        assert!(cap.is_valid());
    }

    #[test]
    fn is_valid_revoked() {
        let mut cap = make_cap(CapabilityScope::Calendar, "*", None);
        cap.revoked = true;
        assert!(!cap.is_valid());
    }

    #[test]
    fn is_valid_expired() {
        let cap = make_cap(
            CapabilityScope::Calendar,
            "*",
            Some(Utc::now() - Duration::hours(1)),
        );
        assert!(!cap.is_valid());
    }

    #[test]
    fn check_authorized() {
        let cap = make_cap(CapabilityScope::Calendar, "read:*", None);
        assert!(cap.check(&Principal::User("alice".into()), "read:events"));
    }

    #[test]
    fn check_wrong_principal() {
        let cap = make_cap(CapabilityScope::Calendar, "read:*", None);
        assert!(!cap.check(&Principal::User("bob".into()), "read:events"));
    }

    #[test]
    fn check_wrong_action() {
        let cap = make_cap(CapabilityScope::Calendar, "read:*", None);
        assert!(!cap.check(&Principal::User("alice".into()), "write:events"));
    }

    #[test]
    fn check_revoked_rejects() {
        let mut cap = make_cap(CapabilityScope::Calendar, "read:*", None);
        cap.revoked = true;
        assert!(!cap.check(&Principal::User("alice".into()), "read:events"));
    }

    #[test]
    fn attenuate_adds_expiry_to_no_expiry_parent() {
        // Parent has no expiry — child can freely add one (narrowing, not broadening).
        let cap = make_cap(
            CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            "*",
            None,
        );
        let child_exp = Utc::now() + Duration::hours(1);
        let child = cap.attenuate(
            CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            ActionPattern::new("*").unwrap(),
            vec![Principal::User("alice".into())],
            Some(child_exp),
        );
        assert!(child.is_some());
        let child = child.unwrap();
        assert_eq!(child.expires_at, Some(child_exp));
        assert_eq!(child.parent_id, Some(cap.id));
    }

    #[test]
    fn check_agent_principal() {
        let agent_id = AgentId::new();
        let mut cap = make_cap(CapabilityScope::Calendar, "*", None);
        cap.granted_to = vec![Principal::Agent(agent_id)];
        assert!(cap.check(&Principal::Agent(agent_id), "anything"));
        assert!(!cap.check(&Principal::User("alice".into()), "anything"));
    }
}
