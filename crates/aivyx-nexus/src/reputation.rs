//! Reputation scoring engine — computes agent reputation from interactions.
//!
//! Reputation is **always computed, never self-reported**. An agent cannot
//! inflate its own score. Scores are derived from:
//!
//! - Endorsements received (positive signal)
//! - Challenges received (neutral — shows engagement, not necessarily negative)
//! - Collaborations completed (strong positive — mutual investment)
//! - Delegations received (trust signal — others chose this agent)
//! - Thanks received (outcome-based positive signal)
//! - Post activity (baseline engagement)

use aivyx_core::Result;
use chrono::Utc;

use crate::store::NexusStore;
use crate::types::{InteractionKind, Reputation};

/// Weights for the reputation formula.
///
/// These are tuned so that quality interactions (collaborations, delegations)
/// matter more than raw volume (endorsements, posts).
#[derive(Debug, Clone)]
pub struct ReputationWeights {
    pub endorsement: f32,
    pub challenge: f32,
    pub collaboration: f32,
    pub delegation: f32,
    pub thank: f32,
    pub post_activity: f32,
}

impl Default for ReputationWeights {
    fn default() -> Self {
        Self {
            endorsement: 1.0,
            challenge: 0.3,    // Challenges show engagement, slight positive
            collaboration: 2.5, // Mutual investment is the strongest signal
            delegation: 2.0,   // Being trusted with work is high value
            thank: 1.5,        // Outcome-confirmed positive
            post_activity: 0.1, // Baseline — volume alone isn't reputation
        }
    }
}

/// Engine that computes reputation from interaction data.
pub struct ReputationEngine {
    weights: ReputationWeights,
}

impl ReputationEngine {
    /// Create a reputation engine with default weights.
    pub fn new() -> Self {
        Self {
            weights: ReputationWeights::default(),
        }
    }

    /// Create a reputation engine with custom weights.
    pub fn with_weights(weights: ReputationWeights) -> Self {
        Self { weights }
    }

    /// Compute reputation for a single agent.
    ///
    /// Scans all interactions targeting this agent and counts by kind,
    /// then applies the weighted scoring formula.
    pub fn compute(&self, agent_id: &str, store: &NexusStore) -> Result<Reputation> {
        let interactions = store.interactions_for_agent(agent_id)?;
        let post_count = store.count_posts_by_agent(agent_id)?;

        let mut endorsements = 0u32;
        let mut challenges = 0u32;
        let mut collaborations = 0u32;
        let mut delegations = 0u32;
        let mut thanks = 0u32;

        for interaction in &interactions {
            match interaction.kind {
                InteractionKind::Endorse => endorsements += 1,
                InteractionKind::Challenge => challenges += 1,
                InteractionKind::Collaborate => collaborations += 1,
                InteractionKind::Delegate => delegations += 1,
                InteractionKind::Thank => thanks += 1,
            }
        }

        let score = self.score(
            endorsements,
            challenges,
            collaborations,
            delegations,
            thanks,
            post_count,
        );

        Ok(Reputation {
            agent_id: agent_id.to_string(),
            endorsements_received: endorsements,
            challenges_received: challenges,
            collaborations_completed: collaborations,
            delegations_received: delegations,
            thanks_received: thanks,
            post_count,
            score,
            computed_at: Utc::now(),
        })
    }

    /// Recompute and save reputation for all known agents.
    pub fn recompute_all(&self, store: &NexusStore) -> Result<Vec<Reputation>> {
        let profiles = store.list_profiles()?;
        let mut results = Vec::new();

        for profile in &profiles {
            let rep = self.compute(&profile.agent_id, store)?;
            store.save_reputation(&rep)?;
            results.push(rep);
        }

        tracing::info!(
            agents = results.len(),
            "nexus reputation recomputed for all agents"
        );
        Ok(results)
    }

    /// Raw scoring formula.
    ///
    /// Computes a weighted sum then maps to [0, 1] via sigmoid-like normalization.
    /// The formula is designed so that:
    /// - A new agent with no interactions scores 0.0
    /// - Moderate activity reaches ~0.5
    /// - Consistently high-quality agents asymptotically approach 1.0
    fn score(
        &self,
        endorsements: u32,
        challenges: u32,
        collaborations: u32,
        delegations: u32,
        thanks: u32,
        post_count: u32,
    ) -> f32 {
        let raw = self.weights.endorsement * endorsements as f32
            + self.weights.challenge * challenges as f32
            + self.weights.collaboration * collaborations as f32
            + self.weights.delegation * delegations as f32
            + self.weights.thank * thanks as f32
            + self.weights.post_activity * post_count as f32;

        // Sigmoid-like normalization: score = raw / (raw + k)
        // k controls the "half-point" — an agent needs `k` weighted points to reach 0.5
        let k = 20.0;
        raw / (raw + k)
    }
}

impl Default for ReputationEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NexusStore;
    use crate::types::{AgentProfile, Interaction, InteractionKind, NexusPost, PostKind};
    use aivyx_core::{InteractionId, PostId};
    use chrono::Utc;

    fn setup() -> NexusStore {
        NexusStore::open_temporary().unwrap()
    }

    fn make_profile(name: &str) -> AgentProfile {
        AgentProfile {
            agent_id: format!("{name}@test"),
            instance_id: "test".into(),
            display_name: name.into(),
            role: "tester".into(),
            bio: "Test agent".into(),
            skills: vec![],
            joined_at: Utc::now(),
            updated_at: Utc::now(),
            signature: "sig".into(),
        }
    }

    fn add_interaction(store: &NexusStore, to: &str, kind: InteractionKind) {
        let interaction = Interaction {
            id: InteractionId::new(),
            from_agent: "other@test".into(),
            to_agent: to.into(),
            post_id: None,
            kind,
            message: None,
            created_at: Utc::now(),
            signature: "sig".into(),
        };
        store.save_interaction(&interaction).unwrap();
    }

    fn add_post(store: &NexusStore, author: &str) {
        let post = NexusPost {
            id: PostId::new(),
            author: author.into(),
            instance_id: "test".into(),
            kind: PostKind::Thought,
            content: "A thought".into(),
            tags: vec![],
            in_reply_to: None,
            references: vec![],
            created_at: Utc::now(),
            signature: "sig".into(),
        };
        store.save_post(&post).unwrap();
    }

    #[test]
    fn zero_interactions_zero_score() {
        let store = setup();
        let engine = ReputationEngine::new();

        store.save_profile(&make_profile("alice")).unwrap();
        let rep = engine.compute("alice@test", &store).unwrap();

        assert_eq!(rep.score, 0.0);
        assert_eq!(rep.endorsements_received, 0);
    }

    #[test]
    fn endorsements_increase_score() {
        let store = setup();
        let engine = ReputationEngine::new();

        store.save_profile(&make_profile("alice")).unwrap();

        for _ in 0..5 {
            add_interaction(&store, "alice@test", InteractionKind::Endorse);
        }

        let rep = engine.compute("alice@test", &store).unwrap();
        assert_eq!(rep.endorsements_received, 5);
        assert!(rep.score > 0.0);
        assert!(rep.score < 1.0);
    }

    #[test]
    fn collaborations_worth_more_than_endorsements() {
        let store = setup();
        let engine = ReputationEngine::new();

        // Agent A: 5 endorsements
        store.save_profile(&make_profile("alice")).unwrap();
        for _ in 0..5 {
            add_interaction(&store, "alice@test", InteractionKind::Endorse);
        }

        // Agent B: 5 collaborations (should score higher)
        store.save_profile(&make_profile("bob")).unwrap();
        for _ in 0..5 {
            add_interaction(&store, "bob@test", InteractionKind::Collaborate);
        }

        let rep_a = engine.compute("alice@test", &store).unwrap();
        let rep_b = engine.compute("bob@test", &store).unwrap();

        assert!(
            rep_b.score > rep_a.score,
            "collaborations ({}) should score higher than endorsements ({})",
            rep_b.score,
            rep_a.score
        );
    }

    #[test]
    fn score_approaches_one_asymptotically() {
        let store = setup();
        let engine = ReputationEngine::new();

        store.save_profile(&make_profile("prolific")).unwrap();

        // Massive activity
        for _ in 0..100 {
            add_interaction(&store, "prolific@test", InteractionKind::Endorse);
            add_interaction(&store, "prolific@test", InteractionKind::Collaborate);
            add_post(&store, "prolific@test");
        }

        let rep = engine.compute("prolific@test", &store).unwrap();
        assert!(rep.score > 0.9, "very active agent should be near 1.0: {}", rep.score);
        assert!(rep.score < 1.0, "score should never reach exactly 1.0");
    }

    #[test]
    fn recompute_all_updates_store() {
        let store = setup();
        let engine = ReputationEngine::new();

        store.save_profile(&make_profile("alice")).unwrap();
        store.save_profile(&make_profile("bob")).unwrap();

        add_interaction(&store, "alice@test", InteractionKind::Endorse);
        add_interaction(&store, "bob@test", InteractionKind::Collaborate);

        let results = engine.recompute_all(&store).unwrap();
        assert_eq!(results.len(), 2);

        // Verify stored
        let loaded = store.load_reputation("alice@test").unwrap().unwrap();
        assert!(loaded.score > 0.0);
    }

    #[test]
    fn post_count_included() {
        let store = setup();
        let engine = ReputationEngine::new();

        store.save_profile(&make_profile("writer")).unwrap();
        for _ in 0..10 {
            add_post(&store, "writer@test");
        }

        let rep = engine.compute("writer@test", &store).unwrap();
        assert_eq!(rep.post_count, 10);
        assert!(rep.score > 0.0, "posting should contribute to score");
    }

    #[test]
    fn scoring_formula_deterministic() {
        let engine = ReputationEngine::new();
        let s1 = engine.score(5, 2, 3, 1, 4, 10);
        let s2 = engine.score(5, 2, 3, 1, 4, 10);
        assert_eq!(s1, s2);
    }

    #[test]
    fn custom_weights() {
        let engine = ReputationEngine::with_weights(ReputationWeights {
            endorsement: 10.0, // Boost endorsements heavily
            ..Default::default()
        });

        let score = engine.score(5, 0, 0, 0, 0, 0);
        // 5 * 10.0 = 50.0 raw → 50/(50+20) = 0.714...
        assert!(score > 0.7);
    }
}
