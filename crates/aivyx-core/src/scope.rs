use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Defines the domain a capability governs.
///
/// Each variant constrains a different resource type. Attenuation can only
/// narrow scope within the same variant — cross-variant attenuation always
/// returns `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapabilityScope {
    /// Access to a filesystem subtree rooted at `root`.
    Filesystem { root: PathBuf },
    /// Access to specific network hosts and ports.
    Network { hosts: Vec<String>, ports: Vec<u16> },
    /// Permission to execute specific shell commands.
    Shell { allowed_commands: Vec<String> },
    /// Permission to send email to specific recipients.
    Email { allowed_recipients: Vec<String> },
    /// Access to calendar operations (no further subdivision).
    Calendar,
    /// User-defined scope identified by name.
    Custom(String),
}

impl CapabilityScope {
    /// Attempt to narrow this scope to `narrower`. Returns `Some` only when
    /// the child is a strict subset (or equal) of `self`. Cross-variant
    /// attenuation always returns `None`.
    pub fn attenuate(&self, narrower: &Self) -> Option<Self> {
        if narrower.is_subset_of(self) {
            Some(narrower.clone())
        } else {
            None
        }
    }

    /// For Shell scopes, returns the allowed commands list.
    /// Empty list means "all commands allowed" (wildcard).
    pub fn shell_allowed_commands(&self) -> Option<&[String]> {
        match self {
            Self::Shell { allowed_commands } => Some(allowed_commands),
            _ => None,
        }
    }

    /// For Network scopes, returns the allowed hosts.
    /// Empty list means "all hosts allowed" (wildcard).
    pub fn network_allowed_hosts(&self) -> Option<&[String]> {
        match self {
            Self::Network { hosts, .. } => Some(hosts),
            _ => None,
        }
    }

    /// Returns `true` if `self` is contained within `other`.
    pub fn is_subset_of(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Filesystem { root: child }, Self::Filesystem { root: parent }) => {
                child.starts_with(parent)
            }
            (
                Self::Network {
                    hosts: ch,
                    ports: cp,
                },
                Self::Network {
                    hosts: ph,
                    ports: pp,
                },
            ) => {
                (ph.is_empty() || ch.iter().all(|h| ph.contains(h)))
                    && (pp.is_empty() || cp.iter().all(|p| pp.contains(p)))
            }
            (
                Self::Shell {
                    allowed_commands: cc,
                },
                Self::Shell {
                    allowed_commands: pc,
                },
            ) => pc.is_empty() || cc.iter().all(|c| pc.contains(c)),
            (
                Self::Email {
                    allowed_recipients: cr,
                },
                Self::Email {
                    allowed_recipients: pr,
                },
            ) => pr.is_empty() || cr.iter().all(|r| pr.contains(r)),
            (Self::Calendar, Self::Calendar) => true,
            (Self::Custom(cn), Self::Custom(pn)) => {
                if pn.ends_with(":*") {
                    // Prefix wildcard: "mcp:*" matches "mcp:github", "mcp:slack", etc.
                    let prefix = &pn[..pn.len() - 1];
                    cn.starts_with(prefix)
                } else {
                    cn == pn
                }
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_attenuate_narrows() {
        let parent = CapabilityScope::Filesystem {
            root: PathBuf::from("/home"),
        };
        let child = CapabilityScope::Filesystem {
            root: PathBuf::from("/home/user"),
        };
        assert!(parent.attenuate(&child).is_some());
    }

    #[test]
    fn filesystem_attenuate_rejects_broader() {
        let parent = CapabilityScope::Filesystem {
            root: PathBuf::from("/home/user"),
        };
        let wider = CapabilityScope::Filesystem {
            root: PathBuf::from("/home"),
        };
        assert!(parent.attenuate(&wider).is_none());
    }

    #[test]
    fn filesystem_attenuate_rejects_disjoint() {
        let parent = CapabilityScope::Filesystem {
            root: PathBuf::from("/home/alice"),
        };
        let other = CapabilityScope::Filesystem {
            root: PathBuf::from("/home/bob"),
        };
        assert!(parent.attenuate(&other).is_none());
    }

    #[test]
    fn network_attenuate_subset() {
        let parent = CapabilityScope::Network {
            hosts: vec!["a.com".into(), "b.com".into()],
            ports: vec![80, 443],
        };
        let child = CapabilityScope::Network {
            hosts: vec!["a.com".into()],
            ports: vec![443],
        };
        assert!(parent.attenuate(&child).is_some());
    }

    #[test]
    fn network_attenuate_rejects_extra_host() {
        let parent = CapabilityScope::Network {
            hosts: vec!["a.com".into()],
            ports: vec![80],
        };
        let wider = CapabilityScope::Network {
            hosts: vec!["a.com".into(), "evil.com".into()],
            ports: vec![80],
        };
        assert!(parent.attenuate(&wider).is_none());
    }

    #[test]
    fn network_attenuate_rejects_extra_port() {
        let parent = CapabilityScope::Network {
            hosts: vec!["a.com".into()],
            ports: vec![80],
        };
        let wider = CapabilityScope::Network {
            hosts: vec!["a.com".into()],
            ports: vec![80, 8080],
        };
        assert!(parent.attenuate(&wider).is_none());
    }

    #[test]
    fn shell_attenuate_subset() {
        let parent = CapabilityScope::Shell {
            allowed_commands: vec!["ls".into(), "cat".into(), "grep".into()],
        };
        let child = CapabilityScope::Shell {
            allowed_commands: vec!["ls".into()],
        };
        assert!(parent.attenuate(&child).is_some());
    }

    #[test]
    fn shell_attenuate_rejects_extra_command() {
        let parent = CapabilityScope::Shell {
            allowed_commands: vec!["ls".into()],
        };
        let wider = CapabilityScope::Shell {
            allowed_commands: vec!["ls".into(), "rm".into()],
        };
        assert!(parent.attenuate(&wider).is_none());
    }

    #[test]
    fn email_attenuate_subset() {
        let parent = CapabilityScope::Email {
            allowed_recipients: vec!["a@x.com".into(), "b@x.com".into()],
        };
        let child = CapabilityScope::Email {
            allowed_recipients: vec!["a@x.com".into()],
        };
        assert!(parent.attenuate(&child).is_some());
    }

    #[test]
    fn email_attenuate_rejects_extra_recipient() {
        let parent = CapabilityScope::Email {
            allowed_recipients: vec!["a@x.com".into()],
        };
        let wider = CapabilityScope::Email {
            allowed_recipients: vec!["a@x.com".into(), "evil@x.com".into()],
        };
        assert!(parent.attenuate(&wider).is_none());
    }

    #[test]
    fn calendar_attenuates_to_calendar() {
        let parent = CapabilityScope::Calendar;
        let child = CapabilityScope::Calendar;
        assert!(parent.attenuate(&child).is_some());
    }

    #[test]
    fn custom_same_name_attenuates() {
        let parent = CapabilityScope::Custom("foo".into());
        let child = CapabilityScope::Custom("foo".into());
        assert!(parent.attenuate(&child).is_some());
    }

    #[test]
    fn custom_different_name_rejects() {
        let parent = CapabilityScope::Custom("foo".into());
        let child = CapabilityScope::Custom("bar".into());
        assert!(parent.attenuate(&child).is_none());
    }

    #[test]
    fn cross_variant_always_none() {
        let fs = CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        };
        let net = CapabilityScope::Network {
            hosts: vec![],
            ports: vec![],
        };
        assert!(fs.attenuate(&net).is_none());
        assert!(net.attenuate(&fs).is_none());

        let shell = CapabilityScope::Shell {
            allowed_commands: vec![],
        };
        assert!(fs.attenuate(&shell).is_none());
        assert!(CapabilityScope::Calendar.attenuate(&fs).is_none());
    }

    #[test]
    fn custom_prefix_wildcard_matches() {
        let parent = CapabilityScope::Custom("mcp:*".into());
        let child = CapabilityScope::Custom("mcp:github".into());
        assert!(child.is_subset_of(&parent));
        assert!(parent.attenuate(&child).is_some());
    }

    #[test]
    fn custom_prefix_wildcard_rejects_wrong_prefix() {
        let parent = CapabilityScope::Custom("mcp:*".into());
        let child = CapabilityScope::Custom("other:github".into());
        assert!(!child.is_subset_of(&parent));
        assert!(parent.attenuate(&child).is_none());
    }

    #[test]
    fn custom_exact_still_works() {
        let parent = CapabilityScope::Custom("memory".into());
        let child = CapabilityScope::Custom("memory".into());
        assert!(child.is_subset_of(&parent));
        // Non-wildcard doesn't match different names
        let other = CapabilityScope::Custom("memory:sub".into());
        assert!(!other.is_subset_of(&parent));
    }

    #[test]
    fn custom_wildcard_attenuate_narrows() {
        let parent = CapabilityScope::Custom("mcp:*".into());
        // Narrowing from wildcard to specific should succeed
        let specific = CapabilityScope::Custom("mcp:github".into());
        assert!(parent.attenuate(&specific).is_some());
        // Narrowing from specific back to wildcard should fail (broadening)
        assert!(specific.attenuate(&parent).is_none());
    }

    #[test]
    fn is_subset_reflexive() {
        let scope = CapabilityScope::Network {
            hosts: vec!["a.com".into()],
            ports: vec![80],
        };
        assert!(scope.is_subset_of(&scope));
    }

    #[test]
    fn shell_empty_parent_allows_specific_child() {
        let parent = CapabilityScope::Shell {
            allowed_commands: vec![],
        };
        let child = CapabilityScope::Shell {
            allowed_commands: vec!["ls".into(), "cat".into()],
        };
        // Empty parent = wildcard, so any child is a subset
        assert!(child.is_subset_of(&parent));
        assert!(parent.attenuate(&child).is_some());
    }

    #[test]
    fn network_empty_parent_allows_specific_child() {
        let parent = CapabilityScope::Network {
            hosts: vec![],
            ports: vec![],
        };
        let child = CapabilityScope::Network {
            hosts: vec!["example.com".into()],
            ports: vec![443],
        };
        // Empty parent = wildcard
        assert!(child.is_subset_of(&parent));
        assert!(parent.attenuate(&child).is_some());
    }

    #[test]
    fn shell_accessor_returns_commands() {
        let scope = CapabilityScope::Shell {
            allowed_commands: vec!["ls".into()],
        };
        assert_eq!(
            scope.shell_allowed_commands(),
            Some(vec!["ls".to_string()].as_slice())
        );
        assert_eq!(scope.network_allowed_hosts(), None);
    }

    #[test]
    fn network_accessor_returns_hosts() {
        let scope = CapabilityScope::Network {
            hosts: vec!["example.com".into()],
            ports: vec![443],
        };
        assert_eq!(
            scope.network_allowed_hosts(),
            Some(vec!["example.com".to_string()].as_slice())
        );
        assert_eq!(scope.shell_allowed_commands(), None);
    }

    #[test]
    fn email_empty_parent_allows_specific_child() {
        let parent = CapabilityScope::Email {
            allowed_recipients: vec![],
        };
        let child = CapabilityScope::Email {
            allowed_recipients: vec!["user@example.com".into()],
        };
        assert!(child.is_subset_of(&parent));
    }
}
