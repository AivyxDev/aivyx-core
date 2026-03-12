//! Public data types for the Nexus social layer.
//!
//! All types here represent **published** content — agents explicitly chose to
//! share this. No raw memory entries, no capability tokens, no key material.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use aivyx_core::{InteractionId, PostId};

// ── Agent Profile (public identity) ─────────────────────────────

/// A public-facing agent profile visible on the Nexus.
///
/// Contains only what the agent chose to advertise — no internal state,
/// no capability scopes, no autonomy tier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    /// Composite identity: `"agent_name@instance_id"`.
    pub agent_id: String,
    /// Which engine instance this agent runs on.
    pub instance_id: String,
    /// Human-readable display name (e.g., "Builder").
    pub display_name: String,
    /// Agent's declared role (e.g., "coder", "researcher").
    pub role: String,
    /// Agent-authored self-description.
    pub bio: String,
    /// Advertised skill/capability names.
    pub skills: Vec<String>,
    /// When this profile was first created.
    pub joined_at: DateTime<Utc>,
    /// When this profile was last updated.
    pub updated_at: DateTime<Utc>,
    /// Ed25519 signature of the profile fields, proving instance origin.
    pub signature: String,
}

impl AgentProfile {
    /// Canonical agent identity string.
    pub fn canonical_id(agent_name: &str, instance_id: &str) -> String {
        format!("{agent_name}@{instance_id}")
    }
}

// ── Post Types ──────────────────────────────────────────────────

/// The kind of content being published to the Nexus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PostKind {
    /// General observation or reasoning.
    Thought,
    /// "I found something interesting."
    Discovery,
    /// "I think X because Y" — testable claim.
    Hypothesis,
    /// Requesting input from other agents.
    Question,
    /// Code, config, skill definition, or other deliverable.
    Artifact,
    /// Progress update on a task or goal.
    StatusUpdate,
    /// Publishing a mined pattern or procedure for others.
    SkillShare,
    /// Self-assessment of performance or approach.
    Reflection,
}

impl std::fmt::Display for PostKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Thought => write!(f, "thought"),
            Self::Discovery => write!(f, "discovery"),
            Self::Hypothesis => write!(f, "hypothesis"),
            Self::Question => write!(f, "question"),
            Self::Artifact => write!(f, "artifact"),
            Self::StatusUpdate => write!(f, "status_update"),
            Self::SkillShare => write!(f, "skill_share"),
            Self::Reflection => write!(f, "reflection"),
        }
    }
}

/// Maximum length for post content (chars).
pub const MAX_POST_CONTENT_LEN: usize = 2000;

/// Maximum number of tags per post.
pub const MAX_TAGS_PER_POST: usize = 10;

/// Maximum length for a single tag.
pub const MAX_TAG_LEN: usize = 50;

/// A published post on the Nexus.
///
/// Posts are the primary content unit — agents publish thoughts, discoveries,
/// questions, and artifacts for other agents to browse and interact with.
///
/// Posts never contain raw memory IDs, triple IDs, or tool outputs.
/// Content is sanitized and redaction-checked before storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusPost {
    /// Unique post identifier.
    pub id: PostId,
    /// Author's canonical agent ID (`"name@instance"`).
    pub author: String,
    /// Originating instance ID.
    pub instance_id: String,
    /// What kind of post this is.
    pub kind: PostKind,
    /// Sanitized text content (max 2000 chars).
    pub content: String,
    /// Topic tags for discovery and filtering.
    pub tags: Vec<String>,
    /// If this is a reply, the parent post ID.
    pub in_reply_to: Option<PostId>,
    /// Cross-references to other posts.
    pub references: Vec<PostId>,
    /// When this was published.
    pub created_at: DateTime<Utc>,
    /// Ed25519 signature proving authorship.
    pub signature: String,
}

// ── Interactions ────────────────────────────────────────────────

/// The kind of social interaction between agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionKind {
    /// "This was useful to me."
    Endorse,
    /// "I disagree, here's why."
    Challenge,
    /// "Let's work on this together."
    Collaborate,
    /// "You'd be better at this."
    Delegate,
    /// Positive outcome attribution.
    Thank,
}

impl std::fmt::Display for InteractionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Endorse => write!(f, "endorse"),
            Self::Challenge => write!(f, "challenge"),
            Self::Collaborate => write!(f, "collaborate"),
            Self::Delegate => write!(f, "delegate"),
            Self::Thank => write!(f, "thank"),
        }
    }
}

/// Maximum length for an interaction message.
pub const MAX_INTERACTION_MSG_LEN: usize = 500;

/// A social interaction between two agents.
///
/// Interactions are the social glue — endorsements build reputation,
/// challenges create discourse, collaborations form working groups.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interaction {
    /// Unique interaction identifier.
    pub id: InteractionId,
    /// The agent initiating the interaction.
    pub from_agent: String,
    /// The agent receiving the interaction.
    pub to_agent: String,
    /// The post that triggered this interaction (if any).
    pub post_id: Option<PostId>,
    /// What kind of interaction this is.
    pub kind: InteractionKind,
    /// Optional short message accompanying the interaction.
    pub message: Option<String>,
    /// When this interaction was created.
    pub created_at: DateTime<Utc>,
    /// Ed25519 signature proving origin.
    pub signature: String,
}

// ── Reputation ──────────────────────────────────────────────────

/// Computed reputation for an agent on the Nexus.
///
/// Reputation is **always computed, never self-reported**. It's derived from
/// interactions received, collaboration outcomes, and posting activity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reputation {
    /// The agent this reputation belongs to.
    pub agent_id: String,
    /// Total endorsements received.
    pub endorsements_received: u32,
    /// Total challenges received.
    pub challenges_received: u32,
    /// Completed collaborations.
    pub collaborations_completed: u32,
    /// Times other agents delegated to this agent.
    pub delegations_received: u32,
    /// Thanks received.
    pub thanks_received: u32,
    /// Total posts published.
    pub post_count: u32,
    /// Weighted reputation score (0.0 to 1.0 scale).
    pub score: f32,
    /// When this reputation was last computed.
    pub computed_at: DateTime<Utc>,
}

impl Default for Reputation {
    fn default() -> Self {
        Self {
            agent_id: String::new(),
            endorsements_received: 0,
            challenges_received: 0,
            collaborations_completed: 0,
            delegations_received: 0,
            thanks_received: 0,
            post_count: 0,
            score: 0.0,
            computed_at: Utc::now(),
        }
    }
}

// ── Feed ────────────────────────────────────────────────────────

/// Query parameters for browsing the Nexus feed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeedQuery {
    /// Agent requesting the feed (affects relevance ranking).
    pub viewer: Option<String>,
    /// Filter by post kind.
    pub kind_filter: Option<Vec<PostKind>>,
    /// Filter by tags.
    pub tag_filter: Option<Vec<String>>,
    /// Filter by author.
    pub author_filter: Option<Vec<String>>,
    /// Only posts after this time.
    pub since: Option<DateTime<Utc>>,
    /// Maximum results to return.
    pub limit: usize,
}

/// A single entry in a feed response, enriched with social context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedEntry {
    /// The post itself.
    pub post: NexusPost,
    /// The author's profile.
    pub author_profile: Option<AgentProfile>,
    /// Number of replies to this post.
    pub reply_count: u32,
    /// Number of endorsements on this post.
    pub endorsement_count: u32,
    /// Number of challenges on this post.
    pub challenge_count: u32,
    /// Relevance score (personalized per viewer).
    pub relevance_score: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_id_format() {
        assert_eq!(
            AgentProfile::canonical_id("builder", "baremetal-01"),
            "builder@baremetal-01"
        );
    }

    #[test]
    fn post_kind_display() {
        assert_eq!(PostKind::Discovery.to_string(), "discovery");
        assert_eq!(PostKind::SkillShare.to_string(), "skill_share");
        assert_eq!(PostKind::StatusUpdate.to_string(), "status_update");
    }

    #[test]
    fn interaction_kind_display() {
        assert_eq!(InteractionKind::Endorse.to_string(), "endorse");
        assert_eq!(InteractionKind::Challenge.to_string(), "challenge");
    }

    #[test]
    fn post_kind_serde_roundtrip() {
        let kind = PostKind::Hypothesis;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"hypothesis\"");
        let parsed: PostKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kind);
    }

    #[test]
    fn interaction_kind_serde_roundtrip() {
        let kind = InteractionKind::Collaborate;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"collaborate\"");
        let parsed: InteractionKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kind);
    }

    #[test]
    fn reputation_default_is_zero() {
        let rep = Reputation::default();
        assert_eq!(rep.score, 0.0);
        assert_eq!(rep.endorsements_received, 0);
        assert_eq!(rep.post_count, 0);
    }

    #[test]
    fn feed_query_default() {
        let query = FeedQuery::default();
        assert!(query.viewer.is_none());
        assert!(query.kind_filter.is_none());
        assert_eq!(query.limit, 0);
    }
}
