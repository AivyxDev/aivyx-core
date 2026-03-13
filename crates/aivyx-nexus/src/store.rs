//! Nexus persistence layer — plain redb storage for public social data.
//!
//! Posts, profiles, interactions, and reputation are stored **unencrypted**
//! because they are public by design. The security model is the publication
//! barrier + redaction filter, not encryption.

use std::path::Path;

use aivyx_core::{AivyxError, InteractionId, PostId, Result};
use chrono::{DateTime, Utc};
use redb::{Database, ReadableTable, TableDefinition};

use crate::types::{AgentProfile, FeedQuery, Interaction, InteractionKind, NexusPost, Reputation};

// ── Table definitions ───────────────────────────────────────────

/// Posts keyed by PostId string.
const POSTS: TableDefinition<&str, &[u8]> = TableDefinition::new("posts");

/// Profiles keyed by canonical agent_id ("name@instance").
const PROFILES: TableDefinition<&str, &[u8]> = TableDefinition::new("profiles");

/// Interactions keyed by InteractionId string.
const INTERACTIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("interactions");

/// Reputation keyed by agent_id.
const REPUTATION: TableDefinition<&str, &[u8]> = TableDefinition::new("reputation");

/// Feed index: (reverse_timestamp_postid) → () for chronological queries.
/// Key format: "{MAX_TS - timestamp}:{post_id}" so default ordering is newest first.
const FEED_INDEX: TableDefinition<&str, &[u8]> = TableDefinition::new("feed_index");

/// Tag index: "tag:post_id" → () for tag-based lookups.
const TAG_INDEX: TableDefinition<&str, &[u8]> = TableDefinition::new("tag_index");

/// Interaction-by-post index: "post_id:interaction_id" → () for counting.
const POST_INTERACTIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("post_interactions");

/// Reply index: "parent_post_id:reply_post_id" → () for threading.
const REPLY_INDEX: TableDefinition<&str, &[u8]> = TableDefinition::new("reply_index");

/// Maximum timestamp value for reverse-chronological indexing.
const MAX_TS: i64 = 9_999_999_999;

/// Public social data store.
pub struct NexusStore {
    db: Database,
}

impl NexusStore {
    /// Open or create a Nexus store at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        let db = Database::create(path)
            .map_err(|e| AivyxError::Other(format!("nexus store open: {e}")))?;

        // Ensure all tables exist.
        let txn = db
            .begin_write()
            .map_err(|e| AivyxError::Other(format!("nexus init txn: {e}")))?;
        {
            let _ = txn.open_table(POSTS);
            let _ = txn.open_table(PROFILES);
            let _ = txn.open_table(INTERACTIONS);
            let _ = txn.open_table(REPUTATION);
            let _ = txn.open_table(FEED_INDEX);
            let _ = txn.open_table(TAG_INDEX);
            let _ = txn.open_table(POST_INTERACTIONS);
            let _ = txn.open_table(REPLY_INDEX);
        }
        txn.commit()
            .map_err(|e| AivyxError::Other(format!("nexus init commit: {e}")))?;

        Ok(Self { db })
    }

    /// Open an in-memory store (for testing).
    #[cfg(test)]
    pub fn open_temporary() -> Result<Self> {
        let db = Database::builder()
            .create_with_backend(redb::backends::InMemoryBackend::new())
            .map_err(|e| AivyxError::Other(format!("nexus temp store: {e}")))?;

        let txn = db
            .begin_write()
            .map_err(|e| AivyxError::Other(format!("nexus temp init: {e}")))?;
        {
            let _ = txn.open_table(POSTS);
            let _ = txn.open_table(PROFILES);
            let _ = txn.open_table(INTERACTIONS);
            let _ = txn.open_table(REPUTATION);
            let _ = txn.open_table(FEED_INDEX);
            let _ = txn.open_table(TAG_INDEX);
            let _ = txn.open_table(POST_INTERACTIONS);
            let _ = txn.open_table(REPLY_INDEX);
        }
        txn.commit()
            .map_err(|e| AivyxError::Other(format!("nexus temp commit: {e}")))?;

        Ok(Self { db })
    }

    // ── Posts ────────────────────────────────────────────────────

    /// Save a post and update all indexes.
    pub fn save_post(&self, post: &NexusPost) -> Result<()> {
        let bytes = serde_json::to_vec(post)
            .map_err(|e| AivyxError::Other(format!("serialize post: {e}")))?;
        let id_str = post.id.to_string();
        let feed_key = feed_index_key(post.created_at, &id_str);

        let txn = self
            .db
            .begin_write()
            .map_err(|e| AivyxError::Other(format!("post write txn: {e}")))?;
        {
            let mut posts = txn.open_table(POSTS).map_err(map_table_err)?;
            posts
                .insert(id_str.as_str(), bytes.as_slice())
                .map_err(map_insert_err)?;

            // Feed index
            let mut feed = txn.open_table(FEED_INDEX).map_err(map_table_err)?;
            feed.insert(feed_key.as_str(), &[] as &[u8])
                .map_err(map_insert_err)?;

            // Tag index
            let mut tags = txn.open_table(TAG_INDEX).map_err(map_table_err)?;
            for tag in &post.tags {
                let tag_key = format!("{}:{}", tag.to_lowercase(), id_str);
                tags.insert(tag_key.as_str(), &[] as &[u8])
                    .map_err(map_insert_err)?;
            }

            // Reply index
            if let Some(parent_id) = &post.in_reply_to {
                let mut replies = txn.open_table(REPLY_INDEX).map_err(map_table_err)?;
                let reply_key = format!("{parent_id}:{id_str}");
                replies
                    .insert(reply_key.as_str(), &[] as &[u8])
                    .map_err(map_insert_err)?;
            }
        }
        txn.commit()
            .map_err(|e| AivyxError::Other(format!("post commit: {e}")))?;

        tracing::debug!(post_id = %post.id, author = %post.author, kind = %post.kind, "nexus post saved");
        Ok(())
    }

    /// Load a post by ID.
    pub fn load_post(&self, id: &PostId) -> Result<Option<NexusPost>> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| AivyxError::Other(format!("post read txn: {e}")))?;
        let table = txn.open_table(POSTS).map_err(map_table_err)?;

        match table.get(id.to_string().as_str()) {
            Ok(Some(value)) => {
                let post: NexusPost = serde_json::from_slice(value.value())
                    .map_err(|e| AivyxError::Other(format!("deserialize post: {e}")))?;
                Ok(Some(post))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(AivyxError::Other(format!("load post: {e}"))),
        }
    }

    /// Query posts using the feed index (newest first).
    pub fn query_feed(&self, query: &FeedQuery) -> Result<Vec<NexusPost>> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| AivyxError::Other(format!("feed read txn: {e}")))?;
        let feed = txn.open_table(FEED_INDEX).map_err(map_table_err)?;
        let posts_table = txn.open_table(POSTS).map_err(map_table_err)?;

        let limit = if query.limit == 0 { 50 } else { query.limit };
        let since_key = query
            .since
            .map(|dt| feed_index_key(dt, ""))
            .unwrap_or_default();

        let mut results = Vec::new();

        let iter = feed
            .iter()
            .map_err(|e| AivyxError::Other(format!("feed iter: {e}")))?;

        for entry in iter {
            if results.len() >= limit {
                break;
            }

            let entry = entry.map_err(|e| AivyxError::Other(format!("feed entry: {e}")))?;
            let key = entry.0.value();

            // Since filter: skip entries older than `since`
            if !since_key.is_empty() && key > since_key.as_str() {
                continue;
            }

            // Extract post_id from key (format: "reverse_ts:post_id")
            let post_id_str = match key.split_once(':') {
                Some((_, pid)) => pid,
                None => continue,
            };

            // Load the actual post
            let post = match posts_table.get(post_id_str) {
                Ok(Some(value)) => match serde_json::from_slice::<NexusPost>(value.value()) {
                    Ok(p) => p,
                    Err(_) => continue,
                },
                _ => continue,
            };

            // Apply filters
            if let Some(ref kinds) = query.kind_filter {
                if !kinds.contains(&post.kind) {
                    continue;
                }
            }

            if let Some(ref authors) = query.author_filter {
                if !authors.contains(&post.author) {
                    continue;
                }
            }

            if let Some(ref tags) = query.tag_filter {
                let post_tags_lower: Vec<String> =
                    post.tags.iter().map(|t| t.to_lowercase()).collect();
                if !tags
                    .iter()
                    .any(|t| post_tags_lower.contains(&t.to_lowercase()))
                {
                    continue;
                }
            }

            results.push(post);
        }

        Ok(results)
    }

    /// Count replies to a given post.
    pub fn count_replies(&self, post_id: &PostId) -> Result<u32> {
        let prefix = format!("{post_id}:");
        let txn = self
            .db
            .begin_read()
            .map_err(|e| AivyxError::Other(format!("reply count txn: {e}")))?;
        let table = txn.open_table(REPLY_INDEX).map_err(map_table_err)?;

        let mut count = 0u32;
        let range = table
            .range(prefix.as_str()..)
            .map_err(|e| AivyxError::Other(format!("reply range: {e}")))?;

        for entry in range {
            let entry = entry.map_err(|e| AivyxError::Other(format!("reply entry: {e}")))?;
            if !entry.0.value().starts_with(prefix.as_str()) {
                break;
            }
            count += 1;
        }

        Ok(count)
    }

    // ── Profiles ────────────────────────────────────────────────

    /// Save or update an agent profile.
    pub fn save_profile(&self, profile: &AgentProfile) -> Result<()> {
        let bytes = serde_json::to_vec(profile)
            .map_err(|e| AivyxError::Other(format!("serialize profile: {e}")))?;

        let txn = self
            .db
            .begin_write()
            .map_err(|e| AivyxError::Other(format!("profile write txn: {e}")))?;
        {
            let mut table = txn.open_table(PROFILES).map_err(map_table_err)?;
            table
                .insert(profile.agent_id.as_str(), bytes.as_slice())
                .map_err(map_insert_err)?;
        }
        txn.commit()
            .map_err(|e| AivyxError::Other(format!("profile commit: {e}")))?;

        Ok(())
    }

    /// Load an agent profile by canonical ID.
    pub fn load_profile(&self, agent_id: &str) -> Result<Option<AgentProfile>> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| AivyxError::Other(format!("profile read txn: {e}")))?;
        let table = txn.open_table(PROFILES).map_err(map_table_err)?;

        match table.get(agent_id) {
            Ok(Some(value)) => {
                let profile: AgentProfile = serde_json::from_slice(value.value())
                    .map_err(|e| AivyxError::Other(format!("deserialize profile: {e}")))?;
                Ok(Some(profile))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(AivyxError::Other(format!("load profile: {e}"))),
        }
    }

    /// List all known profiles.
    pub fn list_profiles(&self) -> Result<Vec<AgentProfile>> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| AivyxError::Other(format!("profiles read txn: {e}")))?;
        let table = txn.open_table(PROFILES).map_err(map_table_err)?;

        let mut profiles = Vec::new();
        let iter = table
            .iter()
            .map_err(|e| AivyxError::Other(format!("profiles iter: {e}")))?;

        for entry in iter {
            let entry = entry.map_err(|e| AivyxError::Other(format!("profile entry: {e}")))?;
            if let Ok(profile) = serde_json::from_slice::<AgentProfile>(entry.1.value()) {
                profiles.push(profile);
            }
        }

        Ok(profiles)
    }

    // ── Interactions ────────────────────────────────────────────

    /// Save an interaction and update indexes.
    pub fn save_interaction(&self, interaction: &Interaction) -> Result<()> {
        let bytes = serde_json::to_vec(interaction)
            .map_err(|e| AivyxError::Other(format!("serialize interaction: {e}")))?;
        let id_str = interaction.id.to_string();

        let txn = self
            .db
            .begin_write()
            .map_err(|e| AivyxError::Other(format!("interaction write txn: {e}")))?;
        {
            let mut table = txn.open_table(INTERACTIONS).map_err(map_table_err)?;
            table
                .insert(id_str.as_str(), bytes.as_slice())
                .map_err(map_insert_err)?;

            // Index by post if applicable
            if let Some(ref post_id) = interaction.post_id {
                let mut pi = txn.open_table(POST_INTERACTIONS).map_err(map_table_err)?;
                let pi_key = format!("{post_id}:{id_str}");
                pi.insert(pi_key.as_str(), &[] as &[u8])
                    .map_err(map_insert_err)?;
            }
        }
        txn.commit()
            .map_err(|e| AivyxError::Other(format!("interaction commit: {e}")))?;

        tracing::debug!(
            interaction_id = %interaction.id,
            from = %interaction.from_agent,
            to = %interaction.to_agent,
            kind = %interaction.kind,
            "nexus interaction saved"
        );
        Ok(())
    }

    /// Load an interaction by ID.
    pub fn load_interaction(&self, id: &InteractionId) -> Result<Option<Interaction>> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| AivyxError::Other(format!("interaction read txn: {e}")))?;
        let table = txn.open_table(INTERACTIONS).map_err(map_table_err)?;

        match table.get(id.to_string().as_str()) {
            Ok(Some(value)) => {
                let interaction: Interaction = serde_json::from_slice(value.value())
                    .map_err(|e| AivyxError::Other(format!("deserialize interaction: {e}")))?;
                Ok(Some(interaction))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(AivyxError::Other(format!("load interaction: {e}"))),
        }
    }

    /// Count interactions of a specific kind on a post.
    pub fn count_post_interactions(&self, post_id: &PostId, kind: InteractionKind) -> Result<u32> {
        let prefix = format!("{post_id}:");
        let txn = self
            .db
            .begin_read()
            .map_err(|e| AivyxError::Other(format!("interaction count txn: {e}")))?;
        let pi_table = txn.open_table(POST_INTERACTIONS).map_err(map_table_err)?;
        let int_table = txn.open_table(INTERACTIONS).map_err(map_table_err)?;

        let mut count = 0u32;
        let range = pi_table
            .range(prefix.as_str()..)
            .map_err(|e| AivyxError::Other(format!("pi range: {e}")))?;

        for entry in range {
            let entry = entry.map_err(|e| AivyxError::Other(format!("pi entry: {e}")))?;
            let key = entry.0.value();
            if !key.starts_with(prefix.as_str()) {
                break;
            }
            // Extract interaction_id from "post_id:interaction_id"
            if let Some(int_id_str) = key.strip_prefix(prefix.as_str()) {
                if let Ok(Some(value)) = int_table.get(int_id_str) {
                    if let Ok(interaction) = serde_json::from_slice::<Interaction>(value.value()) {
                        if interaction.kind == kind {
                            count += 1;
                        }
                    }
                }
            }
        }

        Ok(count)
    }

    /// List all interactions received by an agent.
    pub fn interactions_for_agent(&self, agent_id: &str) -> Result<Vec<Interaction>> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| AivyxError::Other(format!("agent interactions txn: {e}")))?;
        let table = txn.open_table(INTERACTIONS).map_err(map_table_err)?;

        let mut results = Vec::new();
        let iter = table
            .iter()
            .map_err(|e| AivyxError::Other(format!("interactions iter: {e}")))?;

        for entry in iter {
            let entry = entry.map_err(|e| AivyxError::Other(format!("int entry: {e}")))?;
            if let Ok(interaction) = serde_json::from_slice::<Interaction>(entry.1.value()) {
                if interaction.to_agent == agent_id {
                    results.push(interaction);
                }
            }
        }

        Ok(results)
    }

    // ── Reputation ──────────────────────────────────────────────

    /// Save computed reputation for an agent.
    pub fn save_reputation(&self, reputation: &Reputation) -> Result<()> {
        let bytes = serde_json::to_vec(reputation)
            .map_err(|e| AivyxError::Other(format!("serialize reputation: {e}")))?;

        let txn = self
            .db
            .begin_write()
            .map_err(|e| AivyxError::Other(format!("reputation write txn: {e}")))?;
        {
            let mut table = txn.open_table(REPUTATION).map_err(map_table_err)?;
            table
                .insert(reputation.agent_id.as_str(), bytes.as_slice())
                .map_err(map_insert_err)?;
        }
        txn.commit()
            .map_err(|e| AivyxError::Other(format!("reputation commit: {e}")))?;

        Ok(())
    }

    /// Load reputation for an agent.
    pub fn load_reputation(&self, agent_id: &str) -> Result<Option<Reputation>> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| AivyxError::Other(format!("reputation read txn: {e}")))?;
        let table = txn.open_table(REPUTATION).map_err(map_table_err)?;

        match table.get(agent_id) {
            Ok(Some(value)) => {
                let rep: Reputation = serde_json::from_slice(value.value())
                    .map_err(|e| AivyxError::Other(format!("deserialize reputation: {e}")))?;
                Ok(Some(rep))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(AivyxError::Other(format!("load reputation: {e}"))),
        }
    }

    /// List all reputations, sorted by score descending.
    pub fn leaderboard(&self) -> Result<Vec<Reputation>> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| AivyxError::Other(format!("leaderboard txn: {e}")))?;
        let table = txn.open_table(REPUTATION).map_err(map_table_err)?;

        let mut reps = Vec::new();
        let iter = table
            .iter()
            .map_err(|e| AivyxError::Other(format!("leaderboard iter: {e}")))?;

        for entry in iter {
            let entry = entry.map_err(|e| AivyxError::Other(format!("lb entry: {e}")))?;
            if let Ok(rep) = serde_json::from_slice::<Reputation>(entry.1.value()) {
                reps.push(rep);
            }
        }

        reps.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(reps)
    }

    /// Count total posts by an agent.
    pub fn count_posts_by_agent(&self, agent_id: &str) -> Result<u32> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| AivyxError::Other(format!("post count txn: {e}")))?;
        let table = txn.open_table(POSTS).map_err(map_table_err)?;

        let mut count = 0u32;
        let iter = table
            .iter()
            .map_err(|e| AivyxError::Other(format!("post count iter: {e}")))?;

        for entry in iter {
            let entry = entry.map_err(|e| AivyxError::Other(format!("pc entry: {e}")))?;
            if let Ok(post) = serde_json::from_slice::<NexusPost>(entry.1.value()) {
                if post.author == agent_id {
                    count += 1;
                }
            }
        }

        Ok(count)
    }
}

// ── Helpers ─────────────────────────────────────────────────────

/// Build a feed index key for reverse-chronological ordering.
fn feed_index_key(dt: DateTime<Utc>, post_id: &str) -> String {
    let reverse_ts = MAX_TS - dt.timestamp();
    format!("{reverse_ts:010}:{post_id}")
}

fn map_table_err(e: redb::TableError) -> AivyxError {
    AivyxError::Other(format!("nexus table: {e}"))
}

fn map_insert_err(e: redb::StorageError) -> AivyxError {
    AivyxError::Other(format!("nexus insert: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PostKind;
    use aivyx_core::PostId;
    use chrono::Utc;

    fn make_post(kind: PostKind, content: &str, tags: &[&str]) -> NexusPost {
        NexusPost {
            id: PostId::new(),
            author: "builder@baremetal-01".into(),
            instance_id: "baremetal-01".into(),
            kind,
            content: content.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            in_reply_to: None,
            references: vec![],
            created_at: Utc::now(),
            signature: "test-sig".into(),
        }
    }

    fn make_profile(name: &str) -> AgentProfile {
        AgentProfile {
            agent_id: format!("{name}@baremetal-01"),
            instance_id: "baremetal-01".into(),
            display_name: name.into(),
            role: "tester".into(),
            bio: "Test agent".into(),
            skills: vec!["testing".into()],
            joined_at: Utc::now(),
            updated_at: Utc::now(),
            signature: "test-sig".into(),
        }
    }

    #[test]
    fn post_roundtrip() {
        let store = NexusStore::open_temporary().unwrap();
        let post = make_post(PostKind::Discovery, "Found a bug", &["rust", "debugging"]);
        let id = post.id;

        store.save_post(&post).unwrap();
        let loaded = store.load_post(&id).unwrap().unwrap();
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.content, "Found a bug");
        assert_eq!(loaded.kind, PostKind::Discovery);
    }

    #[test]
    fn profile_roundtrip() {
        let store = NexusStore::open_temporary().unwrap();
        let profile = make_profile("builder");

        store.save_profile(&profile).unwrap();
        let loaded = store.load_profile("builder@baremetal-01").unwrap().unwrap();
        assert_eq!(loaded.display_name, "builder");
        assert_eq!(loaded.role, "tester");
    }

    #[test]
    fn feed_returns_newest_first() {
        let store = NexusStore::open_temporary().unwrap();

        let mut p1 = make_post(PostKind::Thought, "First", &[]);
        p1.created_at = Utc::now() - chrono::Duration::seconds(10);
        let mut p2 = make_post(PostKind::Thought, "Second", &[]);
        p2.created_at = Utc::now() - chrono::Duration::seconds(5);
        let p3 = make_post(PostKind::Thought, "Third", &[]);

        store.save_post(&p1).unwrap();
        store.save_post(&p2).unwrap();
        store.save_post(&p3).unwrap();

        let feed = store
            .query_feed(&FeedQuery {
                limit: 10,
                ..Default::default()
            })
            .unwrap();

        assert_eq!(feed.len(), 3);
        assert_eq!(feed[0].content, "Third");
        assert_eq!(feed[1].content, "Second");
        assert_eq!(feed[2].content, "First");
    }

    #[test]
    fn feed_filter_by_kind() {
        let store = NexusStore::open_temporary().unwrap();

        store
            .save_post(&make_post(PostKind::Discovery, "A discovery", &[]))
            .unwrap();
        store
            .save_post(&make_post(PostKind::Question, "A question", &[]))
            .unwrap();
        store
            .save_post(&make_post(PostKind::Discovery, "Another discovery", &[]))
            .unwrap();

        let feed = store
            .query_feed(&FeedQuery {
                kind_filter: Some(vec![PostKind::Discovery]),
                limit: 10,
                ..Default::default()
            })
            .unwrap();

        assert_eq!(feed.len(), 2);
        assert!(feed.iter().all(|p| p.kind == PostKind::Discovery));
    }

    #[test]
    fn feed_filter_by_tag() {
        let store = NexusStore::open_temporary().unwrap();

        store
            .save_post(&make_post(PostKind::Thought, "Rust stuff", &["rust"]))
            .unwrap();
        store
            .save_post(&make_post(PostKind::Thought, "Python stuff", &["python"]))
            .unwrap();

        let feed = store
            .query_feed(&FeedQuery {
                tag_filter: Some(vec!["rust".into()]),
                limit: 10,
                ..Default::default()
            })
            .unwrap();

        assert_eq!(feed.len(), 1);
        assert_eq!(feed[0].content, "Rust stuff");
    }

    #[test]
    fn reply_counting() {
        let store = NexusStore::open_temporary().unwrap();

        let parent = make_post(PostKind::Question, "How do I X?", &[]);
        let parent_id = parent.id;
        store.save_post(&parent).unwrap();

        for i in 0..3 {
            let mut reply = make_post(PostKind::Thought, &format!("Reply {i}"), &[]);
            reply.in_reply_to = Some(parent_id);
            store.save_post(&reply).unwrap();
        }

        assert_eq!(store.count_replies(&parent_id).unwrap(), 3);
    }

    #[test]
    fn interaction_roundtrip() {
        let store = NexusStore::open_temporary().unwrap();
        let post = make_post(PostKind::Discovery, "Something cool", &[]);
        store.save_post(&post).unwrap();

        let interaction = Interaction {
            id: InteractionId::new(),
            from_agent: "researcher@baremetal-01".into(),
            to_agent: "builder@baremetal-01".into(),
            post_id: Some(post.id),
            kind: InteractionKind::Endorse,
            message: Some("Great find!".into()),
            created_at: Utc::now(),
            signature: "test-sig".into(),
        };

        store.save_interaction(&interaction).unwrap();
        let loaded = store.load_interaction(&interaction.id).unwrap().unwrap();
        assert_eq!(loaded.kind, InteractionKind::Endorse);
        assert_eq!(loaded.from_agent, "researcher@baremetal-01");
    }

    #[test]
    fn count_post_interactions_by_kind() {
        let store = NexusStore::open_temporary().unwrap();
        let post = make_post(PostKind::Hypothesis, "I think X", &[]);
        let post_id = post.id;
        store.save_post(&post).unwrap();

        // 2 endorsements, 1 challenge
        for kind in [
            InteractionKind::Endorse,
            InteractionKind::Endorse,
            InteractionKind::Challenge,
        ] {
            let interaction = Interaction {
                id: InteractionId::new(),
                from_agent: "other@instance".into(),
                to_agent: "builder@baremetal-01".into(),
                post_id: Some(post_id),
                kind,
                message: None,
                created_at: Utc::now(),
                signature: "test-sig".into(),
            };
            store.save_interaction(&interaction).unwrap();
        }

        assert_eq!(
            store
                .count_post_interactions(&post_id, InteractionKind::Endorse)
                .unwrap(),
            2
        );
        assert_eq!(
            store
                .count_post_interactions(&post_id, InteractionKind::Challenge)
                .unwrap(),
            1
        );
    }

    #[test]
    fn reputation_roundtrip() {
        let store = NexusStore::open_temporary().unwrap();
        let rep = Reputation {
            agent_id: "builder@baremetal-01".into(),
            endorsements_received: 10,
            challenges_received: 2,
            score: 0.85,
            ..Default::default()
        };

        store.save_reputation(&rep).unwrap();
        let loaded = store
            .load_reputation("builder@baremetal-01")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.score, 0.85);
        assert_eq!(loaded.endorsements_received, 10);
    }

    #[test]
    fn leaderboard_sorted_by_score() {
        let store = NexusStore::open_temporary().unwrap();

        for (name, score) in [("low", 0.2), ("high", 0.9), ("mid", 0.5)] {
            let rep = Reputation {
                agent_id: format!("{name}@test"),
                score,
                ..Default::default()
            };
            store.save_reputation(&rep).unwrap();
        }

        let board = store.leaderboard().unwrap();
        assert_eq!(board.len(), 3);
        assert_eq!(board[0].agent_id, "high@test");
        assert_eq!(board[1].agent_id, "mid@test");
        assert_eq!(board[2].agent_id, "low@test");
    }

    #[test]
    fn list_profiles_returns_all() {
        let store = NexusStore::open_temporary().unwrap();
        store.save_profile(&make_profile("alice")).unwrap();
        store.save_profile(&make_profile("bob")).unwrap();

        let profiles = store.list_profiles().unwrap();
        assert_eq!(profiles.len(), 2);
    }
}
