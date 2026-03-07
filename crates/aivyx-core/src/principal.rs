use serde::{Deserialize, Serialize};

use crate::id::{AgentId, TenantId};

/// Represents who is performing an action in the system.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Principal {
    /// An autonomous agent.
    Agent(AgentId),
    /// A named user (single-user / legacy mode).
    User(String),
    /// A user within a specific tenant (multi-tenant mode).
    TenantUser {
        tenant_id: TenantId,
        user_id: String,
    },
    /// The system itself.
    System,
}

impl std::fmt::Display for Principal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Principal::Agent(id) => write!(f, "agent:{id}"),
            Principal::User(name) => write!(f, "user:{name}"),
            Principal::TenantUser {
                tenant_id,
                user_id,
            } => write!(f, "tenant:{tenant_id}:user:{user_id}"),
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

    #[test]
    fn tenant_user_serde_roundtrip() {
        let p = Principal::TenantUser {
            tenant_id: TenantId::new(),
            user_id: "alice".into(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let parsed: Principal = serde_json::from_str(&json).unwrap();
        assert_eq!(p, parsed);
    }

    #[test]
    fn tenant_user_display() {
        let tenant_id = TenantId::new();
        let p = Principal::TenantUser {
            tenant_id,
            user_id: "bob".into(),
        };
        let display = p.to_string();
        assert!(display.contains("tenant:"));
        assert!(display.contains("user:bob"));
    }

    #[test]
    fn tenant_id_serde_roundtrip() {
        let id = TenantId::new();
        let json = serde_json::to_string(&id).unwrap();
        let parsed: TenantId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }
}
