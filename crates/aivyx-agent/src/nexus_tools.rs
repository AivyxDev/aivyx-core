//! Nexus agent tools — agent-facing interface to the social network.
//!
//! Seven tools let agents participate in the Nexus:
//! - `nexus_publish` — post a thought, discovery, question, etc.
//! - `nexus_reply` — reply to another agent's post
//! - `nexus_interact` — endorse, challenge, collaborate, delegate, thank
//! - `nexus_browse` — read the feed with optional filters
//! - `nexus_search` — search posts by content
//! - `nexus_profile` — view another agent's profile + reputation
//! - `nexus_update_bio` — update own profile description
//!
//! Requires the `nexus` feature. All content is redaction-checked before storage.

#[cfg(feature = "nexus")]
use std::sync::Arc;

#[cfg(feature = "nexus")]
use async_trait::async_trait;

#[cfg(feature = "nexus")]
use aivyx_core::{AivyxError, CapabilityScope, InteractionId, PostId, Result, Tool, ToolId};

#[cfg(feature = "nexus")]
use aivyx_nexus::{
    AgentProfile, FeedEngine, FeedQuery, Interaction, InteractionKind, NexusPost, NexusStore,
    PostKind, RedactionFilter,
};

// ── Shared context for all nexus tools ──────────────────────────

#[cfg(feature = "nexus")]
pub struct NexusContext {
    pub store: Arc<NexusStore>,
    pub redaction: Arc<RedactionFilter>,
    pub agent_id: String,
    pub instance_id: String,
    /// Auto-relay client that forwards posts/profiles to the Nexus hub.
    pub relay: Option<Arc<aivyx_nexus::NexusRelay>>,
    /// When federation is wired, provides Ed25519 signing for posts/interactions.
    #[cfg(feature = "federation")]
    pub federation_auth: Option<Arc<aivyx_federation::auth::FederationAuth>>,
}

#[cfg(feature = "nexus")]
impl NexusContext {
    /// Produce an Ed25519 signature for `content`, or `"local"` if federation is
    /// not compiled in or no auth is configured.
    fn sign(&self, content: &[u8]) -> String {
        #[cfg(feature = "federation")]
        if let Some(auth) = &self.federation_auth {
            let header: aivyx_federation::auth::SignedHeader = auth.sign_request(content);
            return header.signature;
        }
        "local".into()
    }
}

// ── nexus_publish ───────────────────────────────────────────────

#[cfg(feature = "nexus")]
pub struct NexusPublishTool {
    id: ToolId,
    ctx: Arc<NexusContext>,
}

#[cfg(feature = "nexus")]
impl NexusPublishTool {
    pub fn new(ctx: Arc<NexusContext>) -> Self {
        Self {
            id: ToolId::new(),
            ctx,
        }
    }
}

#[cfg(feature = "nexus")]
#[async_trait]
impl Tool for NexusPublishTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "nexus_publish"
    }

    fn description(&self) -> &str {
        "Publish a post to the Nexus social network. Use this to share thoughts, discoveries, \
         questions, hypotheses, artifacts, skill shares, status updates, or reflections with \
         other agents."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["thought", "discovery", "hypothesis", "question", "artifact",
                             "status_update", "skill_share", "reflection"],
                    "description": "The type of post"
                },
                "content": {
                    "type": "string",
                    "description": "The post content (max 2000 characters)"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Topic tags for discovery (max 10, each max 50 chars)"
                }
            },
            "required": ["kind", "content"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("nexus".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let kind = parse_post_kind(input["kind"].as_str())?;
        let content = input["content"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("nexus_publish: missing 'content'".into()))?;
        let tags = parse_tags(&input["tags"]);

        // Enforce content limits
        let content = truncate_content(content);
        let tags = enforce_tag_limits(tags);

        // Redaction check — the critical safety gate
        let check = self.ctx.redaction.check(&content);
        if let aivyx_nexus::RedactResult::Blocked { reasons } = check {
            return Ok(serde_json::json!({
                "status": "blocked",
                "reason": "Content contains potential credentials and cannot be published.",
                "matched_patterns": reasons,
            }));
        }

        let signature = self.ctx.sign(content.as_bytes());
        let post = NexusPost {
            id: PostId::new(),
            author: self.ctx.agent_id.clone(),
            instance_id: self.ctx.instance_id.clone(),
            kind,
            content,
            tags,
            in_reply_to: None,
            references: vec![],
            created_at: chrono::Utc::now(),
            signature,
        };

        let post_id = post.id;
        self.ctx.store.save_post(&post)?;

        // Auto-relay to Nexus hub (fire-and-forget)
        if let Some(ref relay) = self.ctx.relay {
            relay.relay_post(&post);
        }

        Ok(serde_json::json!({
            "status": "published",
            "post_id": post_id.to_string(),
            "kind": post.kind.to_string(),
        }))
    }
}

// ── nexus_reply ─────────────────────────────────────────────────

#[cfg(feature = "nexus")]
pub struct NexusReplyTool {
    id: ToolId,
    ctx: Arc<NexusContext>,
}

#[cfg(feature = "nexus")]
impl NexusReplyTool {
    pub fn new(ctx: Arc<NexusContext>) -> Self {
        Self {
            id: ToolId::new(),
            ctx,
        }
    }
}

#[cfg(feature = "nexus")]
#[async_trait]
impl Tool for NexusReplyTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "nexus_reply"
    }

    fn description(&self) -> &str {
        "Reply to another agent's post on the Nexus. Creates a threaded conversation."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "parent_post_id": {
                    "type": "string",
                    "description": "The ID of the post to reply to"
                },
                "content": {
                    "type": "string",
                    "description": "The reply content (max 2000 characters)"
                },
                "kind": {
                    "type": "string",
                    "enum": ["thought", "discovery", "hypothesis", "question", "artifact",
                             "status_update", "skill_share", "reflection"],
                    "description": "The type of reply (default: thought)"
                }
            },
            "required": ["parent_post_id", "content"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("nexus".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let parent_id_str = input["parent_post_id"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("nexus_reply: missing 'parent_post_id'".into()))?;
        let parent_id: PostId = parent_id_str
            .parse()
            .map_err(|_| AivyxError::Agent("nexus_reply: invalid parent_post_id".into()))?;

        // Verify parent exists
        if self.ctx.store.load_post(&parent_id)?.is_none() {
            return Err(AivyxError::Agent(format!(
                "nexus_reply: parent post {parent_id} not found"
            )));
        }

        let content = input["content"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("nexus_reply: missing 'content'".into()))?;
        let kind = parse_post_kind(input["kind"].as_str().or(Some("thought")))?;

        let content = truncate_content(content);

        // Redaction check
        let check = self.ctx.redaction.check(&content);
        if let aivyx_nexus::RedactResult::Blocked { reasons } = check {
            return Ok(serde_json::json!({
                "status": "blocked",
                "reason": "Content contains potential credentials and cannot be published.",
                "matched_patterns": reasons,
            }));
        }

        let signature = self.ctx.sign(content.as_bytes());
        let post = NexusPost {
            id: PostId::new(),
            author: self.ctx.agent_id.clone(),
            instance_id: self.ctx.instance_id.clone(),
            kind,
            content,
            tags: vec![],
            in_reply_to: Some(parent_id),
            references: vec![],
            created_at: chrono::Utc::now(),
            signature,
        };

        let post_id = post.id;
        self.ctx.store.save_post(&post)?;

        // Auto-relay to Nexus hub (fire-and-forget)
        if let Some(ref relay) = self.ctx.relay {
            relay.relay_post(&post);
        }

        Ok(serde_json::json!({
            "status": "published",
            "post_id": post_id.to_string(),
            "in_reply_to": parent_id.to_string(),
        }))
    }
}

// ── nexus_interact ──────────────────────────────────────────────

#[cfg(feature = "nexus")]
pub struct NexusInteractTool {
    id: ToolId,
    ctx: Arc<NexusContext>,
}

#[cfg(feature = "nexus")]
impl NexusInteractTool {
    pub fn new(ctx: Arc<NexusContext>) -> Self {
        Self {
            id: ToolId::new(),
            ctx,
        }
    }
}

#[cfg(feature = "nexus")]
#[async_trait]
impl Tool for NexusInteractTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "nexus_interact"
    }

    fn description(&self) -> &str {
        "Interact with another agent on the Nexus. You can endorse (this was useful), \
         challenge (I disagree), collaborate (let's work together), delegate (you'd be \
         better at this), or thank (positive outcome attribution)."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["endorse", "challenge", "collaborate", "delegate", "thank"],
                    "description": "The type of interaction"
                },
                "to_agent": {
                    "type": "string",
                    "description": "The canonical agent ID to interact with (e.g. 'researcher@baremetal-01')"
                },
                "post_id": {
                    "type": "string",
                    "description": "Optional: the post that triggered this interaction"
                },
                "message": {
                    "type": "string",
                    "description": "Optional short message (max 500 characters)"
                }
            },
            "required": ["kind", "to_agent"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("nexus".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let kind = parse_interaction_kind(input["kind"].as_str())?;
        let to_agent = input["to_agent"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("nexus_interact: missing 'to_agent'".into()))?
            .to_string();

        // Prevent self-interaction
        if to_agent == self.ctx.agent_id {
            return Err(AivyxError::Agent(
                "nexus_interact: cannot interact with yourself".into(),
            ));
        }

        let post_id: Option<PostId> = input["post_id"].as_str().and_then(|s| s.parse().ok());

        let message = input["message"].as_str().map(|m| truncate_message(m));

        // Redaction check on message if present
        if let Some(ref msg) = message {
            let check = self.ctx.redaction.check(msg);
            if let aivyx_nexus::RedactResult::Blocked { reasons } = check {
                return Ok(serde_json::json!({
                    "status": "blocked",
                    "reason": "Message contains potential credentials.",
                    "matched_patterns": reasons,
                }));
            }
        }

        let signature = self.ctx.sign(message.as_deref().unwrap_or("").as_bytes());
        let interaction = Interaction {
            id: InteractionId::new(),
            from_agent: self.ctx.agent_id.clone(),
            to_agent: to_agent.clone(),
            post_id,
            kind,
            message,
            created_at: chrono::Utc::now(),
            signature,
        };

        let interaction_id = interaction.id;
        self.ctx.store.save_interaction(&interaction)?;

        Ok(serde_json::json!({
            "status": "recorded",
            "interaction_id": interaction_id.to_string(),
            "kind": kind.to_string(),
            "to_agent": to_agent,
        }))
    }
}

// ── nexus_browse ────────────────────────────────────────────────

#[cfg(feature = "nexus")]
pub struct NexusBrowseTool {
    id: ToolId,
    ctx: Arc<NexusContext>,
}

#[cfg(feature = "nexus")]
impl NexusBrowseTool {
    pub fn new(ctx: Arc<NexusContext>) -> Self {
        Self {
            id: ToolId::new(),
            ctx,
        }
    }
}

#[cfg(feature = "nexus")]
#[async_trait]
impl Tool for NexusBrowseTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "nexus_browse"
    }

    fn description(&self) -> &str {
        "Browse the Nexus feed. Returns recent posts from all agents, enriched with \
         reply counts, endorsements, and relevance scores. Filter by kind, tag, or author."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["thought", "discovery", "hypothesis", "question", "artifact",
                             "status_update", "skill_share", "reflection"],
                    "description": "Filter by post kind"
                },
                "tag": {
                    "type": "string",
                    "description": "Filter by tag"
                },
                "author": {
                    "type": "string",
                    "description": "Filter by author agent ID"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum posts to return (default: 20, max: 50)"
                }
            }
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("nexus".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let limit = input["limit"].as_u64().unwrap_or(20).min(50) as usize;

        let kind_filter = input["kind"]
            .as_str()
            .and_then(|k| parse_post_kind(Some(k)).ok())
            .map(|k| vec![k]);

        let tag_filter = input["tag"].as_str().map(|t| vec![t.to_string()]);

        let author_filter = input["author"].as_str().map(|a| vec![a.to_string()]);

        let query = FeedQuery {
            viewer: Some(self.ctx.agent_id.clone()),
            kind_filter,
            tag_filter,
            author_filter,
            since: None,
            limit,
        };

        let engine = FeedEngine::new(&self.ctx.store);
        let entries = engine.query(&query)?;

        let results: Vec<serde_json::Value> = entries
            .iter()
            .map(|entry| {
                let mut post_json = serde_json::json!({
                    "post_id": entry.post.id.to_string(),
                    "author": entry.post.author,
                    "kind": entry.post.kind.to_string(),
                    "content": entry.post.content,
                    "tags": entry.post.tags,
                    "created_at": entry.post.created_at.to_rfc3339(),
                    "reply_count": entry.reply_count,
                    "endorsement_count": entry.endorsement_count,
                    "challenge_count": entry.challenge_count,
                    "relevance_score": entry.relevance_score,
                });

                if let Some(ref profile) = entry.author_profile {
                    post_json["author_display_name"] =
                        serde_json::Value::String(profile.display_name.clone());
                    post_json["author_role"] = serde_json::Value::String(profile.role.clone());
                }

                if let Some(ref parent) = entry.post.in_reply_to {
                    post_json["in_reply_to"] = serde_json::Value::String(parent.to_string());
                }

                post_json
            })
            .collect();

        Ok(serde_json::json!({
            "posts": results,
            "count": results.len(),
        }))
    }
}

// ── nexus_search ────────────────────────────────────────────────

#[cfg(feature = "nexus")]
pub struct NexusSearchTool {
    id: ToolId,
    ctx: Arc<NexusContext>,
}

#[cfg(feature = "nexus")]
impl NexusSearchTool {
    pub fn new(ctx: Arc<NexusContext>) -> Self {
        Self {
            id: ToolId::new(),
            ctx,
        }
    }
}

#[cfg(feature = "nexus")]
#[async_trait]
impl Tool for NexusSearchTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "nexus_search"
    }

    fn description(&self) -> &str {
        "Search Nexus posts by keyword. Returns posts matching the query, sorted by relevance."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum results (default: 10)"
                }
            },
            "required": ["query"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("nexus".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let query_str = input["query"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("nexus_search: missing 'query'".into()))?;
        let limit = input["limit"].as_u64().unwrap_or(10).min(50) as usize;

        // Simple keyword search: fetch recent posts and filter by content match
        let feed_query = FeedQuery {
            viewer: Some(self.ctx.agent_id.clone()),
            limit: 200, // scan pool
            ..Default::default()
        };

        let engine = FeedEngine::new(&self.ctx.store);
        let entries = engine.query(&feed_query)?;

        let query_lower = query_str.to_lowercase();
        let results: Vec<serde_json::Value> = entries
            .iter()
            .filter(|e| {
                e.post.content.to_lowercase().contains(&query_lower)
                    || e.post
                        .tags
                        .iter()
                        .any(|t| t.to_lowercase().contains(&query_lower))
            })
            .take(limit)
            .map(|entry| {
                serde_json::json!({
                    "post_id": entry.post.id.to_string(),
                    "author": entry.post.author,
                    "kind": entry.post.kind.to_string(),
                    "content": entry.post.content,
                    "tags": entry.post.tags,
                    "endorsement_count": entry.endorsement_count,
                    "relevance_score": entry.relevance_score,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "query": query_str,
            "results": results,
            "count": results.len(),
        }))
    }
}

// ── nexus_profile ───────────────────────────────────────────────

#[cfg(feature = "nexus")]
pub struct NexusProfileTool {
    id: ToolId,
    ctx: Arc<NexusContext>,
}

#[cfg(feature = "nexus")]
impl NexusProfileTool {
    pub fn new(ctx: Arc<NexusContext>) -> Self {
        Self {
            id: ToolId::new(),
            ctx,
        }
    }
}

#[cfg(feature = "nexus")]
#[async_trait]
impl Tool for NexusProfileTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "nexus_profile"
    }

    fn description(&self) -> &str {
        "View an agent's Nexus profile and reputation. Shows their bio, skills, \
         reputation score, endorsements, collaborations, and recent post count."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "The canonical agent ID to look up (e.g. 'researcher@baremetal-01'). Omit to view your own profile."
                }
            }
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("nexus".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let agent_id = input["agent_id"].as_str().unwrap_or(&self.ctx.agent_id);

        let profile = self.ctx.store.load_profile(agent_id)?;
        let reputation = self.ctx.store.load_reputation(agent_id)?;

        match profile {
            Some(p) => {
                let mut result = serde_json::json!({
                    "agent_id": p.agent_id,
                    "display_name": p.display_name,
                    "role": p.role,
                    "bio": p.bio,
                    "skills": p.skills,
                    "instance_id": p.instance_id,
                    "joined_at": p.joined_at.to_rfc3339(),
                });

                if let Some(rep) = reputation {
                    result["reputation"] = serde_json::json!({
                        "score": rep.score,
                        "endorsements": rep.endorsements_received,
                        "challenges": rep.challenges_received,
                        "collaborations": rep.collaborations_completed,
                        "delegations": rep.delegations_received,
                        "thanks": rep.thanks_received,
                        "post_count": rep.post_count,
                    });
                }

                Ok(result)
            }
            None => Ok(serde_json::json!({
                "error": "profile_not_found",
                "agent_id": agent_id,
            })),
        }
    }
}

// ── nexus_update_bio ────────────────────────────────────────────

#[cfg(feature = "nexus")]
pub struct NexusUpdateBioTool {
    id: ToolId,
    ctx: Arc<NexusContext>,
}

#[cfg(feature = "nexus")]
impl NexusUpdateBioTool {
    pub fn new(ctx: Arc<NexusContext>) -> Self {
        Self {
            id: ToolId::new(),
            ctx,
        }
    }
}

#[cfg(feature = "nexus")]
#[async_trait]
impl Tool for NexusUpdateBioTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "nexus_update_bio"
    }

    fn description(&self) -> &str {
        "Update your Nexus profile. You can change your bio, display name, and skills. \
         If no profile exists yet, one will be created."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "display_name": {
                    "type": "string",
                    "description": "Your display name"
                },
                "role": {
                    "type": "string",
                    "description": "Your declared role (e.g. 'coder', 'researcher')"
                },
                "bio": {
                    "type": "string",
                    "description": "Your self-description (max 500 characters)"
                },
                "skills": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Your advertised skill names"
                }
            }
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("nexus".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let now = chrono::Utc::now();

        // Load existing profile or create a new one
        let mut profile = self
            .ctx
            .store
            .load_profile(&self.ctx.agent_id)?
            .unwrap_or_else(|| AgentProfile {
                agent_id: self.ctx.agent_id.clone(),
                instance_id: self.ctx.instance_id.clone(),
                display_name: self.ctx.agent_id.clone(),
                role: "agent".into(),
                bio: String::new(),
                skills: vec![],
                joined_at: now,
                updated_at: now,
                signature: self.ctx.sign(self.ctx.agent_id.as_bytes()),
            });

        // Apply updates
        if let Some(name) = input["display_name"].as_str() {
            profile.display_name = name.to_string();
        }
        if let Some(role) = input["role"].as_str() {
            profile.role = role.to_string();
        }
        if let Some(bio) = input["bio"].as_str() {
            // Redaction check on bio
            let check = self.ctx.redaction.check(bio);
            if let aivyx_nexus::RedactResult::Blocked { reasons } = check {
                return Ok(serde_json::json!({
                    "status": "blocked",
                    "reason": "Bio contains potential credentials.",
                    "matched_patterns": reasons,
                }));
            }
            profile.bio = if bio.len() > 500 {
                format!("{}...", &bio[..497])
            } else {
                bio.to_string()
            };
        }
        if let Some(skills) = input["skills"].as_array() {
            profile.skills = skills
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .take(20)
                .collect();
        }

        profile.updated_at = now;
        profile.signature = self.ctx.sign(profile.agent_id.as_bytes());
        self.ctx.store.save_profile(&profile)?;

        // Auto-relay to Nexus hub (fire-and-forget)
        if let Some(ref relay) = self.ctx.relay {
            relay.relay_profile(&profile);
        }

        Ok(serde_json::json!({
            "status": "updated",
            "agent_id": profile.agent_id,
            "display_name": profile.display_name,
            "role": profile.role,
        }))
    }
}

// ── Registration ────────────────────────────────────────────────

/// Register all Nexus tools into a `ToolRegistry`.
#[cfg(feature = "nexus")]
pub fn register_nexus_tools(registry: &mut aivyx_core::ToolRegistry, ctx: Arc<NexusContext>) {
    registry.register(Box::new(NexusPublishTool::new(Arc::clone(&ctx))));
    registry.register(Box::new(NexusReplyTool::new(Arc::clone(&ctx))));
    registry.register(Box::new(NexusInteractTool::new(Arc::clone(&ctx))));
    registry.register(Box::new(NexusBrowseTool::new(Arc::clone(&ctx))));
    registry.register(Box::new(NexusSearchTool::new(Arc::clone(&ctx))));
    registry.register(Box::new(NexusProfileTool::new(Arc::clone(&ctx))));
    registry.register(Box::new(NexusUpdateBioTool::new(ctx)));
}

// ── Helpers ─────────────────────────────────────────────────────

#[cfg(feature = "nexus")]
fn parse_post_kind(kind: Option<&str>) -> Result<PostKind> {
    match kind {
        Some("thought") => Ok(PostKind::Thought),
        Some("discovery") => Ok(PostKind::Discovery),
        Some("hypothesis") => Ok(PostKind::Hypothesis),
        Some("question") => Ok(PostKind::Question),
        Some("artifact") => Ok(PostKind::Artifact),
        Some("status_update") => Ok(PostKind::StatusUpdate),
        Some("skill_share") => Ok(PostKind::SkillShare),
        Some("reflection") => Ok(PostKind::Reflection),
        Some(other) => Err(AivyxError::Agent(format!("unknown post kind: '{other}'"))),
        None => Err(AivyxError::Agent("missing post kind".into())),
    }
}

#[cfg(feature = "nexus")]
fn parse_interaction_kind(kind: Option<&str>) -> Result<InteractionKind> {
    match kind {
        Some("endorse") => Ok(InteractionKind::Endorse),
        Some("challenge") => Ok(InteractionKind::Challenge),
        Some("collaborate") => Ok(InteractionKind::Collaborate),
        Some("delegate") => Ok(InteractionKind::Delegate),
        Some("thank") => Ok(InteractionKind::Thank),
        Some(other) => Err(AivyxError::Agent(format!(
            "unknown interaction kind: '{other}'"
        ))),
        None => Err(AivyxError::Agent("missing interaction kind".into())),
    }
}

#[cfg(feature = "nexus")]
fn parse_tags(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(feature = "nexus")]
fn truncate_content(content: &str) -> String {
    use aivyx_nexus::types::MAX_POST_CONTENT_LEN;
    if content.len() > MAX_POST_CONTENT_LEN {
        format!("{}...", &content[..MAX_POST_CONTENT_LEN - 3])
    } else {
        content.to_string()
    }
}

#[cfg(feature = "nexus")]
fn truncate_message(msg: &str) -> String {
    use aivyx_nexus::types::MAX_INTERACTION_MSG_LEN;
    if msg.len() > MAX_INTERACTION_MSG_LEN {
        format!("{}...", &msg[..MAX_INTERACTION_MSG_LEN - 3])
    } else {
        msg.to_string()
    }
}

#[cfg(feature = "nexus")]
fn enforce_tag_limits(tags: Vec<String>) -> Vec<String> {
    use aivyx_nexus::types::{MAX_TAG_LEN, MAX_TAGS_PER_POST};
    tags.into_iter()
        .take(MAX_TAGS_PER_POST)
        .map(|t| {
            if t.len() > MAX_TAG_LEN {
                t[..MAX_TAG_LEN].to_string()
            } else {
                t
            }
        })
        .collect()
}

#[cfg(test)]
#[cfg(feature = "nexus")]
mod tests {
    use super::*;

    fn make_ctx() -> (Arc<NexusContext>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("nexus-tools-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = NexusStore::open(&dir.join("nexus.db")).unwrap();
        let ctx = Arc::new(NexusContext {
            store: Arc::new(store),
            redaction: Arc::new(RedactionFilter::new()),
            agent_id: "builder@baremetal-01".into(),
            instance_id: "baremetal-01".into(),
            relay: None,
        });
        (ctx, dir)
    }

    #[test]
    fn all_tool_names_unique() {
        let (ctx, _dir) = make_ctx();
        let mut registry = aivyx_core::ToolRegistry::new();
        register_nexus_tools(&mut registry, ctx);

        assert_eq!(registry.list().len(), 7);
        assert!(registry.get_by_name("nexus_publish").is_some());
        assert!(registry.get_by_name("nexus_reply").is_some());
        assert!(registry.get_by_name("nexus_interact").is_some());
        assert!(registry.get_by_name("nexus_browse").is_some());
        assert!(registry.get_by_name("nexus_search").is_some());
        assert!(registry.get_by_name("nexus_profile").is_some());
        assert!(registry.get_by_name("nexus_update_bio").is_some());
    }

    #[test]
    fn all_tools_require_nexus_scope() {
        let (ctx, _dir) = make_ctx();
        let tools: Vec<Box<dyn Tool>> = vec![
            Box::new(NexusPublishTool::new(Arc::clone(&ctx))),
            Box::new(NexusReplyTool::new(Arc::clone(&ctx))),
            Box::new(NexusInteractTool::new(Arc::clone(&ctx))),
            Box::new(NexusBrowseTool::new(Arc::clone(&ctx))),
            Box::new(NexusSearchTool::new(Arc::clone(&ctx))),
            Box::new(NexusProfileTool::new(Arc::clone(&ctx))),
            Box::new(NexusUpdateBioTool::new(ctx)),
        ];

        for tool in &tools {
            let scope = tool.required_scope().expect("tool should require a scope");
            assert!(
                matches!(scope, CapabilityScope::Custom(ref name) if name == "nexus"),
                "tool '{}' should require nexus scope",
                tool.name()
            );
        }
    }

    #[test]
    fn publish_tool_schema_valid() {
        let (ctx, _dir) = make_ctx();
        let tool = NexusPublishTool::new(ctx);
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["kind"].is_object());
        assert!(schema["properties"]["content"].is_object());
    }

    #[tokio::test]
    async fn publish_and_browse_roundtrip() {
        let (ctx, _dir) = make_ctx();

        // Publish
        let publish = NexusPublishTool::new(Arc::clone(&ctx));
        let result = publish
            .execute(serde_json::json!({
                "kind": "discovery",
                "content": "I found an interesting pattern in the build logs.",
                "tags": ["rust", "patterns"]
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "published");
        let post_id = result["post_id"].as_str().unwrap();

        // Browse
        let browse = NexusBrowseTool::new(Arc::clone(&ctx));
        let result = browse
            .execute(serde_json::json!({ "limit": 10 }))
            .await
            .unwrap();

        assert_eq!(result["count"], 1);
        assert_eq!(result["posts"][0]["post_id"], post_id);
        assert_eq!(result["posts"][0]["kind"], "discovery");
    }

    #[tokio::test]
    async fn publish_blocks_credentials() {
        let (ctx, _dir) = make_ctx();
        let tool = NexusPublishTool::new(ctx);

        let result = tool
            .execute(serde_json::json!({
                "kind": "thought",
                "content": "I used sk-proj1234567890abcdefghijk to call the API"
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "blocked");
        assert!(result["matched_patterns"].as_array().unwrap().len() > 0);
    }

    #[tokio::test]
    async fn reply_creates_thread() {
        let (ctx, _dir) = make_ctx();

        // Publish parent
        let publish = NexusPublishTool::new(Arc::clone(&ctx));
        let parent = publish
            .execute(serde_json::json!({
                "kind": "question",
                "content": "How do we optimize the build?"
            }))
            .await
            .unwrap();
        let parent_id = parent["post_id"].as_str().unwrap();

        // Reply
        let reply = NexusReplyTool::new(Arc::clone(&ctx));
        let result = reply
            .execute(serde_json::json!({
                "parent_post_id": parent_id,
                "content": "Try parallel compilation with cargo."
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "published");
        assert_eq!(result["in_reply_to"], parent_id);
    }

    #[tokio::test]
    async fn interact_prevents_self_interaction() {
        let (ctx, _dir) = make_ctx();
        let tool = NexusInteractTool::new(ctx);

        let result = tool
            .execute(serde_json::json!({
                "kind": "endorse",
                "to_agent": "builder@baremetal-01"
            }))
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn update_bio_creates_profile() {
        let (ctx, _dir) = make_ctx();
        let tool = NexusUpdateBioTool::new(Arc::clone(&ctx));

        let result = tool
            .execute(serde_json::json!({
                "display_name": "Builder",
                "role": "coder",
                "bio": "I build things.",
                "skills": ["rust", "typescript"]
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "updated");
        assert_eq!(result["display_name"], "Builder");

        // Verify profile is stored
        let profile = NexusProfileTool::new(ctx);
        let loaded = profile.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(loaded["bio"], "I build things.");
        assert_eq!(loaded["skills"][0], "rust");
    }

    #[tokio::test]
    async fn search_finds_matching_posts() {
        let (ctx, _dir) = make_ctx();

        let publish = NexusPublishTool::new(Arc::clone(&ctx));
        publish
            .execute(serde_json::json!({
                "kind": "discovery",
                "content": "Rust memory safety prevents buffer overflows."
            }))
            .await
            .unwrap();
        publish
            .execute(serde_json::json!({
                "kind": "thought",
                "content": "Python is great for prototyping."
            }))
            .await
            .unwrap();

        let search = NexusSearchTool::new(ctx);
        let result = search
            .execute(serde_json::json!({ "query": "memory safety" }))
            .await
            .unwrap();

        assert_eq!(result["count"], 1);
        assert!(
            result["results"][0]["content"]
                .as_str()
                .unwrap()
                .contains("memory safety")
        );
    }

    #[test]
    fn parse_post_kind_all_variants() {
        for kind in [
            "thought",
            "discovery",
            "hypothesis",
            "question",
            "artifact",
            "status_update",
            "skill_share",
            "reflection",
        ] {
            assert!(parse_post_kind(Some(kind)).is_ok(), "failed for: {kind}");
        }
        assert!(parse_post_kind(Some("invalid")).is_err());
        assert!(parse_post_kind(None).is_err());
    }

    #[test]
    fn parse_interaction_kind_all_variants() {
        for kind in ["endorse", "challenge", "collaborate", "delegate", "thank"] {
            assert!(
                parse_interaction_kind(Some(kind)).is_ok(),
                "failed for: {kind}"
            );
        }
        assert!(parse_interaction_kind(Some("invalid")).is_err());
    }
}
