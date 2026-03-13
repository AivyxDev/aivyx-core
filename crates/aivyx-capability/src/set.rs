use aivyx_core::{AivyxError, CapabilityId, Principal, Result};

use crate::scope::CapabilityScope;
use crate::token::Capability;

/// A collection of capabilities that can be queried for authorisation checks.
#[derive(Debug, Clone, Default)]
pub struct CapabilitySet {
    capabilities: Vec<Capability>,
}

impl CapabilitySet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a capability to the set.
    pub fn grant(&mut self, cap: Capability) {
        self.capabilities.push(cap);
    }

    /// Revoke a capability by its id.
    pub fn revoke(&mut self, id: CapabilityId) {
        for cap in &mut self.capabilities {
            if cap.id == id {
                cap.revoked = true;
            }
        }
    }

    /// Find the first valid capability that authorises `principal` to perform
    /// `action` within the given `scope`.
    pub fn check(
        &self,
        principal: &Principal,
        scope: &CapabilityScope,
        action: &str,
    ) -> Result<&Capability> {
        self.capabilities
            .iter()
            .find(|cap| cap.check(principal, action) && scope.is_subset_of(&cap.scope))
            .ok_or_else(|| {
                AivyxError::CapabilityDenied(format!("{principal} not authorised for {action}"))
            })
    }

    /// Produce a new set containing only capabilities that are valid in both
    /// `self` and `other`. A capability from `self` is included if `other`
    /// contains a valid capability whose scope and pattern cover it.
    pub fn intersect(&self, other: &CapabilitySet) -> CapabilitySet {
        let mut result = CapabilitySet::new();
        for cap in &self.capabilities {
            if !cap.is_valid() {
                continue;
            }
            let dominated = other.capabilities.iter().any(|o| {
                o.is_valid()
                    && cap.scope.is_subset_of(&o.scope)
                    && cap.pattern.is_subset_of(&o.pattern)
            });
            if dominated {
                result.grant(cap.clone());
            }
        }
        result
    }

    /// Number of capabilities in the set.
    pub fn len(&self) -> usize {
        self.capabilities.len()
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty()
    }

    /// Iterate over the capabilities.
    pub fn iter(&self) -> impl Iterator<Item = &Capability> {
        self.capabilities.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::ActionPattern;
    use chrono::{Duration, Utc};
    use std::path::PathBuf;

    fn fs_cap(root: &str, pattern: &str, principal: &str) -> Capability {
        Capability {
            id: CapabilityId::new(),
            scope: CapabilityScope::Filesystem {
                root: PathBuf::from(root),
            },
            pattern: ActionPattern::new(pattern).unwrap(),
            granted_to: vec![Principal::User(principal.into())],
            granted_by: Principal::System,
            created_at: Utc::now(),
            expires_at: None,
            revoked: false,
            parent_id: None,
        }
    }

    #[test]
    fn check_finds_matching_cap() {
        let mut set = CapabilitySet::new();
        set.grant(fs_cap("/home", "read:*", "alice"));

        let result = set.check(
            &Principal::User("alice".into()),
            &CapabilityScope::Filesystem {
                root: PathBuf::from("/home/alice"),
            },
            "read:file",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn check_rejects_wrong_principal() {
        let mut set = CapabilitySet::new();
        set.grant(fs_cap("/home", "read:*", "alice"));

        let result = set.check(
            &Principal::User("bob".into()),
            &CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            "read:file",
        );
        assert!(result.is_err());
    }

    #[test]
    fn check_rejects_wrong_action() {
        let mut set = CapabilitySet::new();
        set.grant(fs_cap("/home", "read:*", "alice"));

        let result = set.check(
            &Principal::User("alice".into()),
            &CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            "write:file",
        );
        assert!(result.is_err());
    }

    #[test]
    fn check_rejects_scope_mismatch() {
        let mut set = CapabilitySet::new();
        set.grant(fs_cap("/home/alice", "read:*", "alice"));

        // Requesting a broader scope than the capability allows.
        let result = set.check(
            &Principal::User("alice".into()),
            &CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            "read:file",
        );
        assert!(result.is_err());
    }

    #[test]
    fn revoke_makes_check_fail() {
        let mut set = CapabilitySet::new();
        let cap = fs_cap("/home", "*", "alice");
        let id = cap.id;
        set.grant(cap);

        assert!(
            set.check(
                &Principal::User("alice".into()),
                &CapabilityScope::Filesystem {
                    root: PathBuf::from("/home"),
                },
                "read:file",
            )
            .is_ok()
        );

        set.revoke(id);

        assert!(
            set.check(
                &Principal::User("alice".into()),
                &CapabilityScope::Filesystem {
                    root: PathBuf::from("/home"),
                },
                "read:file",
            )
            .is_err()
        );
    }

    #[test]
    fn expired_cap_rejected_by_check() {
        let mut set = CapabilitySet::new();
        let mut cap = fs_cap("/home", "*", "alice");
        cap.expires_at = Some(Utc::now() - Duration::hours(1));
        set.grant(cap);

        let result = set.check(
            &Principal::User("alice".into()),
            &CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            "read:file",
        );
        assert!(result.is_err());
    }

    #[test]
    fn intersect_keeps_common() {
        let mut a = CapabilitySet::new();
        a.grant(fs_cap("/home", "*", "alice"));
        a.grant(fs_cap("/tmp", "*", "alice"));

        let mut b = CapabilitySet::new();
        b.grant(fs_cap("/home", "*", "bob")); // covers /home scope

        let inter = a.intersect(&b);
        assert_eq!(inter.len(), 1);
        // The /home cap is kept, /tmp is dropped.
        let kept = inter.iter().next().unwrap();
        assert_eq!(
            kept.scope,
            CapabilityScope::Filesystem {
                root: PathBuf::from("/home")
            }
        );
    }

    #[test]
    fn intersect_excludes_revoked() {
        let mut a = CapabilitySet::new();
        a.grant(fs_cap("/home", "*", "alice"));

        let mut b = CapabilitySet::new();
        let mut cap = fs_cap("/home", "*", "bob");
        cap.revoked = true;
        b.grant(cap);

        let inter = a.intersect(&b);
        assert!(inter.is_empty());
    }

    #[test]
    fn empty_set_check_fails() {
        let set = CapabilitySet::new();
        let result = set.check(
            &Principal::User("alice".into()),
            &CapabilityScope::Calendar,
            "read",
        );
        assert!(result.is_err());
    }

    #[test]
    fn intersect_both_empty() {
        let a = CapabilitySet::new();
        let b = CapabilitySet::new();
        let inter = a.intersect(&b);
        assert!(inter.is_empty());
    }

    #[test]
    fn revoke_nonexistent_id_is_noop() {
        let mut set = CapabilitySet::new();
        set.grant(fs_cap("/home", "*", "alice"));
        let phantom_id = CapabilityId::new();
        set.revoke(phantom_id);

        // The original capability should remain valid
        let result = set.check(
            &Principal::User("alice".into()),
            &CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            "read:file",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn multiple_caps_first_match_returned() {
        let mut set = CapabilitySet::new();
        let cap1 = fs_cap("/home", "read:*", "alice");
        let id1 = cap1.id;
        set.grant(cap1);
        set.grant(fs_cap("/home", "*", "alice"));

        let result = set.check(
            &Principal::User("alice".into()),
            &CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            "read:file",
        );
        assert!(result.is_ok());
        assert_eq!(result.unwrap().id, id1);
    }
}
