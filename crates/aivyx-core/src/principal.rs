use serde::{Deserialize, Serialize};

use crate::id::AgentId;

/// Represents who is performing an action in the system.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Principal {
    Agent(AgentId),
    User(String),
    System,
}

impl std::fmt::Display for Principal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Principal::Agent(id) => write!(f, "agent:{id}"),
            Principal::User(name) => write!(f, "user:{name}"),
            Principal::System => write!(f, "system"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn principal_display() {
        assert_eq!(Principal::System.to_string(), "system");
        assert!(
            Principal::User("alice".into())
                .to_string()
                .contains("alice")
        );
    }

    #[test]
    fn principal_serde_roundtrip() {
        let p = Principal::Agent(AgentId::new());
        let json = serde_json::to_string(&p).unwrap();
        let parsed: Principal = serde_json::from_str(&json).unwrap();
        assert_eq!(p, parsed);
    }
}
