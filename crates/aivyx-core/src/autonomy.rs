use serde::{Deserialize, Serialize};

/// Four-tier autonomy model controlling how much freedom an agent has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AutonomyTier {
    /// No autonomous actions allowed. All tool calls require explicit approval.
    Locked = 0,
    /// Agent can propose actions but needs human confirmation for most operations.
    Leash = 1,
    /// Agent can act autonomously within granted capabilities, with audit logging.
    Trust = 2,
    /// Full autonomy within capability scope. Use with caution.
    Free = 3,
}

impl std::fmt::Display for AutonomyTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AutonomyTier::Locked => write!(f, "Locked"),
            AutonomyTier::Leash => write!(f, "Leash"),
            AutonomyTier::Trust => write!(f, "Trust"),
            AutonomyTier::Free => write!(f, "Free"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_ordering() {
        assert!(AutonomyTier::Locked < AutonomyTier::Leash);
        assert!(AutonomyTier::Leash < AutonomyTier::Trust);
        assert!(AutonomyTier::Trust < AutonomyTier::Free);
    }

    #[test]
    fn tier_serde_roundtrip() {
        let tier = AutonomyTier::Trust;
        let json = serde_json::to_string(&tier).unwrap();
        let parsed: AutonomyTier = serde_json::from_str(&json).unwrap();
        assert_eq!(tier, parsed);
    }
}
