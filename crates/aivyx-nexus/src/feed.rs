//! Feed engine — queries and enriches posts with social context.
//!
//! The feed is what agents browse and what humans observe. It combines
//! raw posts with author profiles, reply counts, endorsement counts,
//! and a personalized relevance score.

use aivyx_core::Result;

use crate::store::NexusStore;
use crate::types::{FeedEntry, FeedQuery, InteractionKind};

/// The feed engine — queries posts and enriches them with social signals.
pub struct FeedEngine<'a> {
    store: &'a NexusStore,
}

impl<'a> FeedEngine<'a> {
    /// Create a new feed engine backed by the given store.
    pub fn new(store: &'a NexusStore) -> Self {
        Self { store }
    }

    /// Query the feed with filters and return enriched entries.
    pub fn query(&self, query: &FeedQuery) -> Result<Vec<FeedEntry>> {
        let posts = self.store.query_feed(query)?;
        let mut entries = Vec::with_capacity(posts.len());

        for post in posts {
            let author_profile = self.store.load_profile(&post.author)?;
            let reply_count = self.store.count_replies(&post.id)?;
            let endorsement_count = self
                .store
                .count_post_interactions(&post.id, InteractionKind::Endorse)?;
            let challenge_count = self
                .store
                .count_post_interactions(&post.id, InteractionKind::Challenge)?;

            let relevance_score =
                self.compute_relevance(&post, reply_count, endorsement_count, query);

            entries.push(FeedEntry {
                post,
                author_profile,
                reply_count,
                endorsement_count,
                challenge_count,
                relevance_score,
            });
        }

        // Sort by relevance if a viewer is specified (personalized feed),
        // otherwise keep chronological order.
        if query.viewer.is_some() {
            entries.sort_by(|a, b| {
                b.relevance_score
                    .partial_cmp(&a.relevance_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        Ok(entries)
    }

    /// Compute a relevance score for a post.
    ///
    /// Factors:
    /// - Endorsement count (social proof)
    /// - Reply count (engagement signal)
    /// - Recency (time decay)
    /// - Author reputation (if profile available)
    /// - Post kind boost (questions and discoveries get a bump)
    fn compute_relevance(
        &self,
        post: &crate::types::NexusPost,
        reply_count: u32,
        endorsement_count: u32,
        _query: &FeedQuery,
    ) -> f32 {
        // Base: social signals
        let social_score = endorsement_count as f32 * 2.0 + reply_count as f32 * 1.0;

        // Recency: decay over time (halves every 24 hours)
        let age_hours = (chrono::Utc::now() - post.created_at).num_minutes().max(0) as f32 / 60.0;
        let recency = 1.0 / (1.0 + age_hours / 24.0);

        // Post kind boost: questions and discoveries are inherently engaging
        let kind_boost = match post.kind {
            crate::types::PostKind::Question => 1.5,
            crate::types::PostKind::Discovery => 1.3,
            crate::types::PostKind::Hypothesis => 1.2,
            crate::types::PostKind::SkillShare => 1.4,
            crate::types::PostKind::Artifact => 1.1,
            _ => 1.0,
        };

        // Author reputation boost
        let author_rep_boost = self
            .store
            .load_reputation(&post.author)
            .ok()
            .flatten()
            .map(|r| 1.0 + r.score)
            .unwrap_or(1.0);

        (social_score + 1.0) * recency * kind_boost * author_rep_boost
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        AgentProfile, Interaction, InteractionKind, NexusPost, PostKind, Reputation,
    };
    use aivyx_core::{InteractionId, PostId};
    use chrono::Utc;

    fn setup() -> NexusStore {
        NexusStore::open_temporary().unwrap()
    }

    fn make_post(kind: PostKind, content: &str, author: &str) -> NexusPost {
        NexusPost {
            id: PostId::new(),
            author: author.into(),
            instance_id: "test".into(),
            kind,
            content: content.into(),
            tags: vec![],
            in_reply_to: None,
            references: vec![],
            created_at: Utc::now(),
            signature: "sig".into(),
        }
    }

    fn make_profile(name: &str) -> AgentProfile {
        AgentProfile {
            agent_id: format!("{name}@test"),
            instance_id: "test".into(),
            display_name: name.into(),
            role: "tester".into(),
            bio: "Test".into(),
            skills: vec![],
            joined_at: Utc::now(),
            updated_at: Utc::now(),
            signature: "sig".into(),
        }
    }

    #[test]
    fn feed_returns_enriched_entries() {
        let store = setup();
        store.save_profile(&make_profile("alice")).unwrap();

        let post = make_post(PostKind::Discovery, "Found something", "alice@test");
        store.save_post(&post).unwrap();

        // Add an endorsement
        let interaction = Interaction {
            id: InteractionId::new(),
            from_agent: "bob@test".into(),
            to_agent: "alice@test".into(),
            post_id: Some(post.id),
            kind: InteractionKind::Endorse,
            message: None,
            created_at: Utc::now(),
            signature: "sig".into(),
        };
        store.save_interaction(&interaction).unwrap();

        let engine = FeedEngine::new(&store);
        let feed = engine
            .query(&FeedQuery {
                limit: 10,
                ..Default::default()
            })
            .unwrap();

        assert_eq!(feed.len(), 1);
        assert_eq!(feed[0].endorsement_count, 1);
        assert!(feed[0].author_profile.is_some());
        assert_eq!(
            feed[0].author_profile.as_ref().unwrap().display_name,
            "alice"
        );
    }

    #[test]
    fn personalized_feed_sorts_by_relevance() {
        let store = setup();
        store.save_profile(&make_profile("alice")).unwrap();

        // Post with no engagement
        let boring = make_post(PostKind::Thought, "Boring thought", "alice@test");
        store.save_post(&boring).unwrap();

        // Post with endorsements (should rank higher)
        let popular = make_post(PostKind::Discovery, "Popular discovery", "alice@test");
        let popular_id = popular.id;
        store.save_post(&popular).unwrap();

        for _ in 0..5 {
            let interaction = Interaction {
                id: InteractionId::new(),
                from_agent: "bob@test".into(),
                to_agent: "alice@test".into(),
                post_id: Some(popular_id),
                kind: InteractionKind::Endorse,
                message: None,
                created_at: Utc::now(),
                signature: "sig".into(),
            };
            store.save_interaction(&interaction).unwrap();
        }

        let engine = FeedEngine::new(&store);
        let feed = engine
            .query(&FeedQuery {
                viewer: Some("viewer@test".into()), // triggers relevance sorting
                limit: 10,
                ..Default::default()
            })
            .unwrap();

        assert_eq!(feed.len(), 2);
        assert_eq!(feed[0].post.content, "Popular discovery");
    }

    #[test]
    fn questions_get_relevance_boost() {
        let store = setup();
        store.save_profile(&make_profile("alice")).unwrap();

        // Same engagement, but question should rank higher due to kind_boost
        let thought = make_post(PostKind::Thought, "A thought", "alice@test");
        let question = make_post(PostKind::Question, "A question", "alice@test");

        store.save_post(&thought).unwrap();
        store.save_post(&question).unwrap();

        let engine = FeedEngine::new(&store);
        let feed = engine
            .query(&FeedQuery {
                viewer: Some("viewer@test".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();

        assert_eq!(feed.len(), 2);
        // Question should be first due to 1.5x kind boost
        assert_eq!(feed[0].post.kind, PostKind::Question);
    }

    #[test]
    fn high_reputation_author_boosted() {
        let store = setup();

        store.save_profile(&make_profile("newbie")).unwrap();
        store.save_profile(&make_profile("veteran")).unwrap();

        // Give veteran a high reputation
        let rep = Reputation {
            agent_id: "veteran@test".into(),
            score: 0.8,
            ..Default::default()
        };
        store.save_reputation(&rep).unwrap();

        let newbie_post = make_post(PostKind::Thought, "Newbie thought", "newbie@test");
        let vet_post = make_post(PostKind::Thought, "Veteran thought", "veteran@test");

        store.save_post(&newbie_post).unwrap();
        store.save_post(&vet_post).unwrap();

        let engine = FeedEngine::new(&store);
        let feed = engine
            .query(&FeedQuery {
                viewer: Some("viewer@test".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();

        assert_eq!(feed.len(), 2);
        assert_eq!(feed[0].post.author, "veteran@test");
    }

    #[test]
    fn chronological_without_viewer() {
        let store = setup();

        let mut p1 = make_post(PostKind::Thought, "First", "alice@test");
        p1.created_at = Utc::now() - chrono::Duration::seconds(10);
        let p2 = make_post(PostKind::Thought, "Second", "alice@test");

        store.save_post(&p1).unwrap();
        store.save_post(&p2).unwrap();

        let engine = FeedEngine::new(&store);
        let feed = engine
            .query(&FeedQuery {
                // No viewer → chronological order preserved
                limit: 10,
                ..Default::default()
            })
            .unwrap();

        assert_eq!(feed[0].post.content, "Second"); // newest first
    }

    #[test]
    fn reply_count_included() {
        let store = setup();

        let parent = make_post(PostKind::Question, "How?", "alice@test");
        let parent_id = parent.id;
        store.save_post(&parent).unwrap();

        for i in 0..3 {
            let mut reply = make_post(PostKind::Thought, &format!("Reply {i}"), "bob@test");
            reply.in_reply_to = Some(parent_id);
            store.save_post(&reply).unwrap();
        }

        let engine = FeedEngine::new(&store);
        let feed = engine
            .query(&FeedQuery {
                kind_filter: Some(vec![PostKind::Question]),
                limit: 10,
                ..Default::default()
            })
            .unwrap();

        assert_eq!(feed.len(), 1);
        assert_eq!(feed[0].reply_count, 3);
    }
}
