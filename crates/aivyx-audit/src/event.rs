use aivyx_core::{
    AgentId, AutonomyTier, CapabilityId, MemoryId, Principal, SessionId, TaskId, TaskStatus,
    ToolId, TripleId,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Every auditable action in the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AuditEvent {
    /// The aivyx data directory was initialized.
    SystemInit { timestamp: DateTime<Utc> },
    /// A new capability was granted to a principal.
    CapabilityGranted {
        capability_id: CapabilityId,
        granted_to: Principal,
        granted_by: Principal,
        scope_summary: String,
    },
    /// An existing capability was revoked.
    CapabilityRevoked {
        capability_id: CapabilityId,
        revoked_by: Principal,
    },
    /// An agent successfully executed a tool action.
    ToolExecuted {
        tool_id: ToolId,
        agent_id: AgentId,
        action: String,
        result_summary: String,
    },
    /// A tool execution was denied by the capability system.
    ToolDenied {
        tool_id: ToolId,
        agent_id: AgentId,
        action: String,
        reason: String,
    },
    /// A configuration value was modified.
    ConfigChanged {
        key: String,
        old_value_hash: String,
        new_value_hash: String,
        changed_by: Principal,
    },
    /// A new agent was created with a specific autonomy tier.
    AgentCreated {
        agent_id: AgentId,
        autonomy_tier: AutonomyTier,
    },
    /// An agent was destroyed/removed.
    AgentDestroyed { agent_id: AgentId },
    /// The master encryption key was rotated.
    MasterKeyRotated { timestamp: DateTime<Utc> },
    /// The audit log was verified for integrity.
    AuditVerified { entries_checked: u64, valid: bool },
    /// An agent turn started.
    AgentTurnStarted {
        agent_id: AgentId,
        session_id: SessionId,
    },
    /// An agent turn completed.
    AgentTurnCompleted {
        agent_id: AgentId,
        session_id: SessionId,
        tool_calls_made: u32,
        tokens_used: u64,
    },
    /// An LLM request was sent.
    LlmRequestSent {
        agent_id: AgentId,
        provider: String,
        model: String,
    },
    /// An LLM response was received.
    LlmResponseReceived {
        agent_id: AgentId,
        provider: String,
        input_tokens: u32,
        output_tokens: u32,
        stop_reason: String,
    },
    /// Conversation history was compressed to fit the context window.
    ConversationCompressed {
        agent_id: AgentId,
        /// Number of messages before compression.
        messages_before: usize,
        /// Number of messages after compression.
        messages_after: usize,
    },
    /// Post-turn memory extraction completed (background LLM call).
    MemoryExtractionCompleted {
        agent_id: AgentId,
        /// Number of items extracted (facts + preferences + triples).
        items_extracted: usize,
        input_tokens: u32,
        output_tokens: u32,
    },
    /// A task was delegated from one team member to another.
    TeamDelegation {
        from: String,
        to: String,
        task: String,
    },
    /// A message was sent between team members.
    TeamMessage { from: String, to: String },
    /// A memory was stored by an agent.
    MemoryStored {
        memory_id: MemoryId,
        agent_id: AgentId,
        kind: String,
    },
    /// Memories were retrieved for an agent turn.
    MemoryRetrieved {
        agent_id: AgentId,
        query_summary: String,
        results_count: usize,
    },
    /// A memory was deleted.
    MemoryDeleted {
        memory_id: MemoryId,
        agent_id: AgentId,
    },
    /// A knowledge triple was stored.
    TripleStored {
        triple_id: TripleId,
        agent_id: AgentId,
        subject: String,
        predicate: String,
    },
    /// An HTTP request was received by the server.
    HttpRequestReceived {
        method: String,
        path: String,
        remote_addr: String,
    },
    /// An HTTP authentication attempt failed.
    HttpAuthFailed { remote_addr: String, reason: String },
    /// An MCP server was connected and tools were discovered.
    McpServerConnected {
        server_name: String,
        tool_count: usize,
        timestamp: DateTime<Utc>,
    },
    /// An MCP server was disconnected.
    McpServerDisconnected {
        server_name: String,
        reason: String,
        timestamp: DateTime<Utc>,
    },
    /// A tool result was served from cache instead of executing.
    ToolCacheHit {
        tool_name: String,
        query_hash: String,
    },
    /// A new task/mission was created.
    TaskCreated {
        task_id: TaskId,
        agent_name: String,
        goal: String,
    },
    /// A task step completed (success or failure).
    TaskStepCompleted {
        task_id: TaskId,
        step_index: usize,
        step_description: String,
        success: bool,
    },
    /// A task completed successfully.
    TaskCompleted {
        task_id: TaskId,
        status: TaskStatus,
        steps_completed: usize,
        steps_total: usize,
    },
    /// A task failed with an error.
    TaskFailed {
        task_id: TaskId,
        step_index: usize,
        error: String,
        steps_completed: usize,
        steps_total: usize,
    },
    /// A task was resumed from checkpoint.
    TaskResumed {
        task_id: TaskId,
        resumed_from_step: usize,
    },
    /// An approval gate was reached during task execution.
    TaskApprovalRequested {
        task_id: TaskId,
        step_index: usize,
        context: String,
    },
    /// An approval gate was resolved (approved, rejected, or timed out).
    TaskApprovalResolved {
        task_id: TaskId,
        step_index: usize,
        approved: bool,
        /// How the approval was resolved: "user", "timeout_auto", "timeout_reject".
        method: String,
    },
    /// A project was registered.
    ProjectRegistered {
        /// The project's slug name.
        project_name: String,
        /// The absolute filesystem path to the project root.
        project_path: String,
    },
    /// A project was removed from the registry.
    ProjectRemoved {
        /// The project's slug name.
        project_name: String,
    },
    /// A scheduled background task fired.
    ScheduleFired {
        /// Name of the schedule entry.
        schedule_name: String,
        /// Agent profile that was invoked.
        agent_name: String,
        /// Timestamp of the fire.
        timestamp: DateTime<Utc>,
    },
    /// A scheduled background task completed.
    ScheduleCompleted {
        /// Name of the schedule entry.
        schedule_name: String,
        /// Whether the run succeeded.
        success: bool,
        /// One-line summary of the result (truncated to ~200 chars).
        result_summary: String,
    },
    /// A heartbeat tick fired.
    HeartbeatFired {
        /// Agent profile used for the heartbeat.
        agent_name: String,
        /// Number of context sections gathered (0 = skipped).
        context_sections: usize,
        /// Timestamp of the fire.
        timestamp: DateTime<Utc>,
    },
    /// A heartbeat tick completed.
    HeartbeatCompleted {
        /// Agent profile used for the heartbeat.
        agent_name: String,
        /// Whether the agent decided to act.
        acted: bool,
        /// Number of actions taken (notifications stored, messages sent, etc.).
        actions_taken: usize,
        /// Brief summary of the heartbeat outcome.
        summary: String,
    },
    /// A heartbeat tick was skipped (no context to review).
    HeartbeatSkipped {
        /// Reason the tick was skipped.
        reason: String,
    },
    /// A notification was stored for the user.
    NotificationStored {
        /// The notification ID.
        notification_id: String,
        /// Source schedule or trigger.
        source: String,
    },
    /// Pending notifications were surfaced to the user.
    NotificationsDrained {
        /// Number of notifications surfaced.
        count: usize,
    },
    /// A notification was rated by the user (feedback loop).
    NotificationRated {
        /// The notification ID that was rated.
        notification_id: String,
        /// The rating value ("useful", "partial", "useless").
        rating: String,
    },
    /// The audit log was pruned (old entries removed).
    LogPruned {
        /// Number of entries that were removed.
        entries_removed: usize,
        /// Timestamp of the oldest remaining entry after pruning.
        oldest_remaining: String,
    },
    /// The user profile was updated.
    ProfileUpdated {
        /// Profile revision number after the update.
        revision: u64,
        /// Which top-level fields were changed.
        fields_changed: Vec<String>,
        /// How the update was triggered: `"extraction"`, `"correction"`, or
        /// `"manual"`.
        source: String,
    },
    /// A specialist job was spawned asynchronously by the lead agent.
    JobSpawned {
        /// Name of the team that owns the job.
        team_name: String,
        /// Name of the specialist agent running the job.
        agent_name: String,
        /// Brief summary of the delegated task.
        task_summary: String,
    },
    /// An asynchronous specialist job completed.
    JobCompleted {
        /// Name of the team that owns the job.
        team_name: String,
        /// Name of the specialist agent that ran the job.
        agent_name: String,
        /// Whether the job succeeded.
        success: bool,
    },
    /// An agent modified its own profile configuration.
    SelfProfileModified {
        /// Name of the agent that modified itself.
        agent_name: String,
        /// Which profile fields were changed.
        fields_changed: Vec<String>,
    },
    /// A plugin (MCP tool pack) was installed.
    PluginInstalled {
        /// Name of the installed plugin.
        plugin_name: String,
        /// Installation source (e.g., local path or registry URL).
        source: String,
    },
    /// A plugin was removed.
    PluginRemoved {
        /// Name of the removed plugin.
        plugin_name: String,
    },
    /// A secret was stored or updated in the encrypted store.
    SecretStored {
        /// The key name of the secret.
        key_name: String,
        /// Who triggered the change (e.g., "api" or "cli").
        changed_by: String,
    },
    /// A secret was deleted from the encrypted store.
    SecretDeleted {
        /// The key name of the secret.
        key_name: String,
        /// Who triggered the deletion.
        changed_by: String,
    },
    /// A new agent profile was created.
    AgentProfileCreated {
        /// Name of the created agent.
        agent_name: String,
    },
    /// An agent profile was updated.
    AgentProfileUpdated {
        /// Name of the updated agent.
        agent_name: String,
        /// Which top-level fields were changed.
        fields_changed: Vec<String>,
    },
    /// An agent profile was deleted.
    AgentProfileDeleted {
        /// Name of the deleted agent.
        agent_name: String,
    },
    /// The server bearer token was rotated.
    BearerTokenRotated {
        /// When the rotation occurred.
        timestamp: DateTime<Utc>,
    },
    /// Audio was transcribed to text via speech-to-text.
    AudioTranscribed {
        /// Model used for transcription (e.g., "whisper-1").
        model: String,
        /// Duration of the audio in seconds.
        duration_secs: f64,
    },
    /// An inbound message was received from a communication channel.
    ChannelMessageReceived {
        /// Name of the channel configuration.
        channel_name: String,
        /// Platform identifier (e.g., "telegram", "email").
        platform: String,
        /// Platform-specific user identifier.
        user_id: String,
    },
    /// A response was sent back through a communication channel.
    ChannelMessageSent {
        /// Name of the channel configuration.
        channel_name: String,
        /// Platform identifier.
        platform: String,
        /// Platform-specific user identifier.
        user_id: String,
    },
    /// A communication channel was started.
    ChannelStarted {
        /// Name of the channel configuration.
        channel_name: String,
        /// Platform identifier.
        platform: String,
    },
    /// A communication channel was stopped.
    ChannelStopped {
        /// Name of the channel configuration.
        channel_name: String,
        /// Platform identifier.
        platform: String,
        /// Why the channel stopped (e.g., "shutdown", "error: ...").
        reason: String,
    },
    /// A SKILL.md skill was loaded into an agent's context.
    SkillLoaded {
        /// Name of the skill (from SKILL.md frontmatter).
        skill_name: String,
        /// Name of the agent that activated the skill.
        agent_name: String,
        /// Filesystem path to the skill directory.
        source_path: String,
    },
    /// An endpoint rate limit was exceeded.
    RateLimitExceeded {
        /// Client IP address.
        remote_addr: String,
        /// Which rate limit tier was exceeded (e.g., "llm", "search", "task").
        tier: String,
        /// The request path that triggered the limit.
        path: String,
    },
    /// A WebSocket message exceeded the configured size limit.
    WebSocketFrameTooLarge {
        /// Size of the rejected message in bytes.
        size_bytes: usize,
        /// Configured maximum size in bytes.
        max_bytes: usize,
    },
    /// A team session was saved to persistent storage.
    TeamSessionSaved {
        /// Name of the team.
        team_name: String,
        /// Session identifier.
        session_id: String,
    },
    /// A team session was resumed from persistent storage.
    TeamSessionResumed {
        /// Name of the team.
        team_name: String,
        /// Session identifier.
        session_id: String,
    },

    // --- Phase 5: Enterprise & Scale events ---
    /// A new tenant was created.
    TenantCreated {
        /// Tenant identifier.
        tenant_id: String,
        /// Human-readable tenant name.
        name: String,
    },
    /// A tenant was suspended (API keys disabled, requests rejected).
    TenantSuspended {
        /// Tenant identifier.
        tenant_id: String,
        /// Why the tenant was suspended.
        reason: String,
    },
    /// A tenant was soft-deleted.
    TenantDeleted {
        /// Tenant identifier.
        tenant_id: String,
    },
    /// A tenant API key was created.
    ApiKeyCreated {
        /// Tenant this key belongs to.
        tenant_id: String,
        /// Unique key identifier (not the secret).
        key_id: String,
        /// Permitted scopes for this key.
        scopes: Vec<String>,
    },
    /// A tenant API key was revoked.
    ApiKeyRevoked {
        /// Tenant this key belongs to.
        tenant_id: String,
        /// Unique key identifier.
        key_id: String,
    },
    /// A tenant resource quota was exceeded.
    QuotaExceeded {
        /// Tenant identifier.
        tenant_id: String,
        /// Which quota was hit (e.g., "sessions_per_day", "storage_mb").
        quota_type: String,
        /// The configured limit.
        limit: u64,
        /// The current usage at the time of violation.
        current: u64,
    },
    /// A cost budget threshold was exceeded.
    BudgetExceeded {
        /// Tenant identifier (empty for global budgets).
        tenant_id: String,
        /// Budget scope (e.g., "agent_daily", "tenant_monthly").
        budget_type: String,
        /// Spending limit in USD.
        limit_usd: f64,
        /// Amount spent in USD.
        spent_usd: f64,
    },
    /// A cost alert was fired (e.g., at 80% threshold).
    CostAlertFired {
        /// Tenant identifier.
        tenant_id: String,
        /// Percentage threshold that was crossed.
        threshold_pct: f64,
        /// Amount spent in USD.
        spent_usd: f64,
        /// Budget limit in USD.
        limit_usd: f64,
    },
    /// A multi-stage workflow was created.
    WorkflowCreated {
        /// Workflow identifier.
        workflow_id: String,
        /// Human-readable workflow name.
        name: String,
        /// Number of stages in the workflow.
        stages: usize,
    },
    /// A workflow completed (all stages finished or a terminal failure).
    WorkflowCompleted {
        /// Workflow identifier.
        workflow_id: String,
        /// Terminal status (e.g., "completed", "failed", "cancelled").
        status: String,
    },
    /// An inbound webhook was received and dispatched.
    WebhookReceived {
        /// Name of the trigger configuration that matched.
        trigger_name: String,
        /// Client IP that sent the webhook.
        source_ip: String,
    },
    /// An encrypted backup completed successfully.
    BackupCompleted {
        /// Size of the backup archive in bytes.
        size_bytes: u64,
        /// Where the backup was stored (e.g., S3 bucket/key).
        destination: String,
    },
    /// An encrypted backup failed.
    BackupFailed {
        /// Why the backup failed.
        reason: String,
    },
    /// An SSO (OIDC/SAML) login succeeded.
    SsoLoginSucceeded {
        /// Authenticated user identifier.
        user_id: String,
        /// Identity provider name (e.g., "okta", "azure-ad").
        provider: String,
    },
    /// An SSO login attempt failed.
    SsoLoginFailed {
        /// Why the login failed.
        reason: String,
        /// Identity provider name.
        provider: String,
    },
    /// A capability audit report was generated.
    CapabilityAuditGenerated {
        /// Number of warning findings.
        warnings_count: usize,
        /// Number of agents scanned.
        agents_scanned: usize,
    },
    /// A security alert was triggered (e.g., tool abuse detection).
    SecurityAlert {
        /// Type of alert (e.g., "HighFrequency", "RepeatedDenials", "ScopeEscalation").
        alert_type: String,
        /// Agent that triggered the alert.
        agent_id: aivyx_core::AgentId,
        /// Human-readable details.
        details: String,
    },
    /// A bearer token was generated via passphrase exchange.
    TokenGenerated {
        /// Method used to generate the token (e.g., "passphrase_exchange").
        method: String,
    },
    /// A document was ingested into the memory system (RAG).
    DocumentIngested {
        /// Name of the source document file.
        document_name: String,
        /// Number of text chunks stored.
        chunks: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivyx_core::{AgentId, MemoryId, TripleId};

    fn roundtrip(event: &AuditEvent) -> AuditEvent {
        let json = serde_json::to_string(event).unwrap();
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn memory_stored_serde_roundtrip() {
        let event = AuditEvent::MemoryStored {
            memory_id: MemoryId::new(),
            agent_id: AgentId::new(),
            kind: "Fact".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::MemoryStored { kind, .. } = restored {
            assert_eq!(kind, "Fact");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn memory_retrieved_serde_roundtrip() {
        let event = AuditEvent::MemoryRetrieved {
            agent_id: AgentId::new(),
            query_summary: "what is rust".into(),
            results_count: 3,
        };
        let restored = roundtrip(&event);
        if let AuditEvent::MemoryRetrieved {
            query_summary,
            results_count,
            ..
        } = restored
        {
            assert_eq!(query_summary, "what is rust");
            assert_eq!(results_count, 3);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn memory_deleted_serde_roundtrip() {
        let event = AuditEvent::MemoryDeleted {
            memory_id: MemoryId::new(),
            agent_id: AgentId::new(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"MemoryDeleted\""));
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(restored, AuditEvent::MemoryDeleted { .. }));
    }

    #[test]
    fn http_request_received_serde_roundtrip() {
        let event = AuditEvent::HttpRequestReceived {
            method: "POST".into(),
            path: "/chat".into(),
            remote_addr: "127.0.0.1:8080".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::HttpRequestReceived { method, path, .. } = restored {
            assert_eq!(method, "POST");
            assert_eq!(path, "/chat");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn http_auth_failed_serde_roundtrip() {
        let event = AuditEvent::HttpAuthFailed {
            remote_addr: "10.0.0.1:1234".into(),
            reason: "missing bearer token".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"HttpAuthFailed\""));
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        if let AuditEvent::HttpAuthFailed { reason, .. } = restored {
            assert_eq!(reason, "missing bearer token");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn mcp_server_connected_serde_roundtrip() {
        let event = AuditEvent::McpServerConnected {
            server_name: "test-server".into(),
            tool_count: 5,
            timestamp: Utc::now(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::McpServerConnected {
            server_name,
            tool_count,
            ..
        } = restored
        {
            assert_eq!(server_name, "test-server");
            assert_eq!(tool_count, 5);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn mcp_server_disconnected_serde_roundtrip() {
        let event = AuditEvent::McpServerDisconnected {
            server_name: "test-server".into(),
            reason: "process exited".into(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"McpServerDisconnected\""));
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(restored, AuditEvent::McpServerDisconnected { .. }));
    }

    #[test]
    fn tool_cache_hit_serde_roundtrip() {
        let event = AuditEvent::ToolCacheHit {
            tool_name: "web_search".into(),
            query_hash: "abc123".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::ToolCacheHit {
            tool_name,
            query_hash,
        } = restored
        {
            assert_eq!(tool_name, "web_search");
            assert_eq!(query_hash, "abc123");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn task_created_serde_roundtrip() {
        let event = AuditEvent::TaskCreated {
            task_id: TaskId::new(),
            agent_name: "researcher".into(),
            goal: "Research Rust async".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::TaskCreated { goal, .. } = restored {
            assert_eq!(goal, "Research Rust async");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn task_step_completed_serde_roundtrip() {
        let event = AuditEvent::TaskStepCompleted {
            task_id: TaskId::new(),
            step_index: 2,
            step_description: "Search for tokio docs".into(),
            success: true,
        };
        let restored = roundtrip(&event);
        if let AuditEvent::TaskStepCompleted {
            step_index,
            success,
            ..
        } = restored
        {
            assert_eq!(step_index, 2);
            assert!(success);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn task_completed_serde_roundtrip() {
        let event = AuditEvent::TaskCompleted {
            task_id: TaskId::new(),
            status: TaskStatus::Completed,
            steps_completed: 5,
            steps_total: 5,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"TaskCompleted\""));
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(restored, AuditEvent::TaskCompleted { .. }));
    }

    #[test]
    fn task_failed_serde_roundtrip() {
        let event = AuditEvent::TaskFailed {
            task_id: TaskId::new(),
            step_index: 2,
            error: "API rate limit exceeded".into(),
            steps_completed: 2,
            steps_total: 5,
        };
        let restored = roundtrip(&event);
        if let AuditEvent::TaskFailed {
            step_index,
            error,
            steps_completed,
            steps_total,
            ..
        } = restored
        {
            assert_eq!(step_index, 2);
            assert_eq!(error, "API rate limit exceeded");
            assert_eq!(steps_completed, 2);
            assert_eq!(steps_total, 5);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn task_resumed_serde_roundtrip() {
        let event = AuditEvent::TaskResumed {
            task_id: TaskId::new(),
            resumed_from_step: 3,
        };
        let restored = roundtrip(&event);
        if let AuditEvent::TaskResumed {
            resumed_from_step, ..
        } = restored
        {
            assert_eq!(resumed_from_step, 3);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn profile_updated_serde_roundtrip() {
        let event = AuditEvent::ProfileUpdated {
            revision: 3,
            fields_changed: vec!["name".into(), "tech_stack".into()],
            source: "extraction".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::ProfileUpdated {
            revision,
            fields_changed,
            source,
        } = restored
        {
            assert_eq!(revision, 3);
            assert_eq!(fields_changed, vec!["name", "tech_stack"]);
            assert_eq!(source, "extraction");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn project_registered_serde_roundtrip() {
        let event = AuditEvent::ProjectRegistered {
            project_name: "my-app".into(),
            project_path: "/home/user/projects/my-app".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::ProjectRegistered {
            project_name,
            project_path,
        } = restored
        {
            assert_eq!(project_name, "my-app");
            assert_eq!(project_path, "/home/user/projects/my-app");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn project_removed_serde_roundtrip() {
        let event = AuditEvent::ProjectRemoved {
            project_name: "old-project".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"ProjectRemoved\""));
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        if let AuditEvent::ProjectRemoved { project_name } = restored {
            assert_eq!(project_name, "old-project");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn schedule_fired_serde_roundtrip() {
        let event = AuditEvent::ScheduleFired {
            schedule_name: "morning-digest".into(),
            agent_name: "assistant".into(),
            timestamp: Utc::now(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::ScheduleFired {
            schedule_name,
            agent_name,
            ..
        } = restored
        {
            assert_eq!(schedule_name, "morning-digest");
            assert_eq!(agent_name, "assistant");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn schedule_completed_serde_roundtrip() {
        let event = AuditEvent::ScheduleCompleted {
            schedule_name: "morning-digest".into(),
            success: true,
            result_summary: "CI pipeline green, 3 PRs merged".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::ScheduleCompleted {
            schedule_name,
            success,
            result_summary,
        } = restored
        {
            assert_eq!(schedule_name, "morning-digest");
            assert!(success);
            assert!(result_summary.contains("CI pipeline"));
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn notification_stored_serde_roundtrip() {
        let event = AuditEvent::NotificationStored {
            notification_id: "some-uuid".into(),
            source: "morning-digest".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"NotificationStored\""));
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        if let AuditEvent::NotificationStored { source, .. } = restored {
            assert_eq!(source, "morning-digest");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn notifications_drained_serde_roundtrip() {
        let event = AuditEvent::NotificationsDrained { count: 5 };
        let restored = roundtrip(&event);
        if let AuditEvent::NotificationsDrained { count } = restored {
            assert_eq!(count, 5);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn log_pruned_serde_roundtrip() {
        let event = AuditEvent::LogPruned {
            entries_removed: 150,
            oldest_remaining: "2026-02-01T00:00:00Z".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::LogPruned {
            entries_removed,
            oldest_remaining,
        } = restored
        {
            assert_eq!(entries_removed, 150);
            assert_eq!(oldest_remaining, "2026-02-01T00:00:00Z");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn triple_stored_serde_roundtrip() {
        let event = AuditEvent::TripleStored {
            triple_id: TripleId::new(),
            agent_id: AgentId::new(),
            subject: "Rust".into(),
            predicate: "is_a".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::TripleStored {
            subject, predicate, ..
        } = restored
        {
            assert_eq!(subject, "Rust");
            assert_eq!(predicate, "is_a");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn job_spawned_serde_roundtrip() {
        let event = AuditEvent::JobSpawned {
            team_name: "analysis-team".into(),
            agent_name: "researcher".into(),
            task_summary: "Analyze market data".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::JobSpawned {
            team_name,
            agent_name,
            task_summary,
        } = restored
        {
            assert_eq!(team_name, "analysis-team");
            assert_eq!(agent_name, "researcher");
            assert_eq!(task_summary, "Analyze market data");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn job_completed_serde_roundtrip() {
        let event = AuditEvent::JobCompleted {
            team_name: "analysis-team".into(),
            agent_name: "coder".into(),
            success: true,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"JobCompleted\""));
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        if let AuditEvent::JobCompleted {
            agent_name,
            success,
            ..
        } = restored
        {
            assert_eq!(agent_name, "coder");
            assert!(success);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn self_profile_modified_serde_roundtrip() {
        let event = AuditEvent::SelfProfileModified {
            agent_name: "assistant".into(),
            fields_changed: vec!["soul".into(), "max_tokens".into()],
        };
        let restored = roundtrip(&event);
        if let AuditEvent::SelfProfileModified {
            agent_name,
            fields_changed,
        } = restored
        {
            assert_eq!(agent_name, "assistant");
            assert_eq!(fields_changed, vec!["soul", "max_tokens"]);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn plugin_installed_serde_roundtrip() {
        let event = AuditEvent::PluginInstalled {
            plugin_name: "code-review".into(),
            source: "/usr/local/bin/code-review-mcp".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::PluginInstalled {
            plugin_name,
            source,
        } = restored
        {
            assert_eq!(plugin_name, "code-review");
            assert_eq!(source, "/usr/local/bin/code-review-mcp");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn plugin_removed_serde_roundtrip() {
        let event = AuditEvent::PluginRemoved {
            plugin_name: "old-plugin".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"PluginRemoved\""));
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        if let AuditEvent::PluginRemoved { plugin_name } = restored {
            assert_eq!(plugin_name, "old-plugin");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn secret_stored_serde_roundtrip() {
        let event = AuditEvent::SecretStored {
            key_name: "claude-api-key".into(),
            changed_by: "api".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::SecretStored {
            key_name,
            changed_by,
        } = restored
        {
            assert_eq!(key_name, "claude-api-key");
            assert_eq!(changed_by, "api");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn secret_deleted_serde_roundtrip() {
        let event = AuditEvent::SecretDeleted {
            key_name: "old-key".into(),
            changed_by: "cli".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"SecretDeleted\""));
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(restored, AuditEvent::SecretDeleted { .. }));
    }

    #[test]
    fn agent_profile_created_serde_roundtrip() {
        let event = AuditEvent::AgentProfileCreated {
            agent_name: "researcher".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::AgentProfileCreated { agent_name } = restored {
            assert_eq!(agent_name, "researcher");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn agent_profile_updated_serde_roundtrip() {
        let event = AuditEvent::AgentProfileUpdated {
            agent_name: "coder".into(),
            fields_changed: vec!["role".into(), "soul".into()],
        };
        let restored = roundtrip(&event);
        if let AuditEvent::AgentProfileUpdated {
            agent_name,
            fields_changed,
        } = restored
        {
            assert_eq!(agent_name, "coder");
            assert_eq!(fields_changed, vec!["role", "soul"]);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn agent_profile_deleted_serde_roundtrip() {
        let event = AuditEvent::AgentProfileDeleted {
            agent_name: "old-agent".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"AgentProfileDeleted\""));
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(restored, AuditEvent::AgentProfileDeleted { .. }));
    }

    #[test]
    fn bearer_token_rotated_serde_roundtrip() {
        let event = AuditEvent::BearerTokenRotated {
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"BearerTokenRotated\""));
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(restored, AuditEvent::BearerTokenRotated { .. }));
    }

    #[test]
    fn audio_transcribed_serde_roundtrip() {
        let event = AuditEvent::AudioTranscribed {
            model: "whisper-1".into(),
            duration_secs: 12.5,
        };
        let restored = roundtrip(&event);
        if let AuditEvent::AudioTranscribed {
            model,
            duration_secs,
        } = restored
        {
            assert_eq!(model, "whisper-1");
            assert!((duration_secs - 12.5).abs() < f64::EPSILON);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn channel_message_received_serde_roundtrip() {
        let event = AuditEvent::ChannelMessageReceived {
            channel_name: "tg-personal".into(),
            platform: "telegram".into(),
            user_id: "123456".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::ChannelMessageReceived {
            channel_name,
            platform,
            user_id,
        } = restored
        {
            assert_eq!(channel_name, "tg-personal");
            assert_eq!(platform, "telegram");
            assert_eq!(user_id, "123456");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn channel_message_sent_serde_roundtrip() {
        let event = AuditEvent::ChannelMessageSent {
            channel_name: "tg-personal".into(),
            platform: "telegram".into(),
            user_id: "123456".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"ChannelMessageSent\""));
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(restored, AuditEvent::ChannelMessageSent { .. }));
    }

    #[test]
    fn channel_started_serde_roundtrip() {
        let event = AuditEvent::ChannelStarted {
            channel_name: "email-work".into(),
            platform: "email".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::ChannelStarted {
            channel_name,
            platform,
        } = restored
        {
            assert_eq!(channel_name, "email-work");
            assert_eq!(platform, "email");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn skill_loaded_serde_roundtrip() {
        let event = AuditEvent::SkillLoaded {
            skill_name: "webapp-testing".into(),
            agent_name: "assistant".into(),
            source_path: "/home/user/.aivyx/skills/webapp-testing".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::SkillLoaded {
            skill_name,
            agent_name,
            source_path,
        } = restored
        {
            assert_eq!(skill_name, "webapp-testing");
            assert_eq!(agent_name, "assistant");
            assert_eq!(source_path, "/home/user/.aivyx/skills/webapp-testing");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn channel_stopped_serde_roundtrip() {
        let event = AuditEvent::ChannelStopped {
            channel_name: "tg-personal".into(),
            platform: "telegram".into(),
            reason: "shutdown".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::ChannelStopped {
            channel_name,
            reason,
            ..
        } = restored
        {
            assert_eq!(channel_name, "tg-personal");
            assert_eq!(reason, "shutdown");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn rate_limit_exceeded_serde_roundtrip() {
        let event = AuditEvent::RateLimitExceeded {
            remote_addr: "127.0.0.1".into(),
            tier: "llm".into(),
            path: "/chat/stream".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::RateLimitExceeded {
            remote_addr,
            tier,
            path,
        } = restored
        {
            assert_eq!(remote_addr, "127.0.0.1");
            assert_eq!(tier, "llm");
            assert_eq!(path, "/chat/stream");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn team_session_saved_serde_roundtrip() {
        let event = AuditEvent::TeamSessionSaved {
            team_name: "dev-team".into(),
            session_id: "sess-abc-123".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::TeamSessionSaved {
            team_name,
            session_id,
        } = restored
        {
            assert_eq!(team_name, "dev-team");
            assert_eq!(session_id, "sess-abc-123");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn team_session_resumed_serde_roundtrip() {
        let event = AuditEvent::TeamSessionResumed {
            team_name: "dev-team".into(),
            session_id: "sess-abc-123".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"TeamSessionResumed\""));
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        if let AuditEvent::TeamSessionResumed {
            team_name,
            session_id,
        } = restored
        {
            assert_eq!(team_name, "dev-team");
            assert_eq!(session_id, "sess-abc-123");
        } else {
            panic!("wrong variant");
        }
    }

    // --- Phase 5 audit event tests ---

    #[test]
    fn tenant_created_serde_roundtrip() {
        let event = AuditEvent::TenantCreated {
            tenant_id: "t-abc-123".into(),
            name: "Acme Corp".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::TenantCreated { tenant_id, name } = restored {
            assert_eq!(tenant_id, "t-abc-123");
            assert_eq!(name, "Acme Corp");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn tenant_suspended_serde_roundtrip() {
        let event = AuditEvent::TenantSuspended {
            tenant_id: "t-abc-123".into(),
            reason: "payment overdue".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::TenantSuspended { reason, .. } = restored {
            assert_eq!(reason, "payment overdue");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn tenant_deleted_serde_roundtrip() {
        let event = AuditEvent::TenantDeleted {
            tenant_id: "t-abc-123".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"TenantDeleted\""));
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(restored, AuditEvent::TenantDeleted { .. }));
    }

    #[test]
    fn api_key_created_serde_roundtrip() {
        let event = AuditEvent::ApiKeyCreated {
            tenant_id: "t-abc".into(),
            key_id: "k-xyz".into(),
            scopes: vec!["Chat".into(), "Memory".into()],
        };
        let restored = roundtrip(&event);
        if let AuditEvent::ApiKeyCreated { key_id, scopes, .. } = restored {
            assert_eq!(key_id, "k-xyz");
            assert_eq!(scopes, vec!["Chat", "Memory"]);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn api_key_revoked_serde_roundtrip() {
        let event = AuditEvent::ApiKeyRevoked {
            tenant_id: "t-abc".into(),
            key_id: "k-xyz".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"ApiKeyRevoked\""));
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(restored, AuditEvent::ApiKeyRevoked { .. }));
    }

    #[test]
    fn quota_exceeded_serde_roundtrip() {
        let event = AuditEvent::QuotaExceeded {
            tenant_id: "t-abc".into(),
            quota_type: "sessions_per_day".into(),
            limit: 100,
            current: 101,
        };
        let restored = roundtrip(&event);
        if let AuditEvent::QuotaExceeded {
            quota_type,
            limit,
            current,
            ..
        } = restored
        {
            assert_eq!(quota_type, "sessions_per_day");
            assert_eq!(limit, 100);
            assert_eq!(current, 101);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn budget_exceeded_serde_roundtrip() {
        let event = AuditEvent::BudgetExceeded {
            tenant_id: "t-abc".into(),
            budget_type: "tenant_daily".into(),
            limit_usd: 10.0,
            spent_usd: 10.5,
        };
        let restored = roundtrip(&event);
        if let AuditEvent::BudgetExceeded {
            budget_type,
            limit_usd,
            spent_usd,
            ..
        } = restored
        {
            assert_eq!(budget_type, "tenant_daily");
            assert!((limit_usd - 10.0).abs() < f64::EPSILON);
            assert!((spent_usd - 10.5).abs() < f64::EPSILON);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn cost_alert_fired_serde_roundtrip() {
        let event = AuditEvent::CostAlertFired {
            tenant_id: "t-abc".into(),
            threshold_pct: 80.0,
            spent_usd: 8.0,
            limit_usd: 10.0,
        };
        let restored = roundtrip(&event);
        if let AuditEvent::CostAlertFired {
            threshold_pct,
            spent_usd,
            ..
        } = restored
        {
            assert!((threshold_pct - 80.0).abs() < f64::EPSILON);
            assert!((spent_usd - 8.0).abs() < f64::EPSILON);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn workflow_created_serde_roundtrip() {
        let event = AuditEvent::WorkflowCreated {
            workflow_id: "wf-123".into(),
            name: "deploy-pipeline".into(),
            stages: 4,
        };
        let restored = roundtrip(&event);
        if let AuditEvent::WorkflowCreated { name, stages, .. } = restored {
            assert_eq!(name, "deploy-pipeline");
            assert_eq!(stages, 4);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn workflow_completed_serde_roundtrip() {
        let event = AuditEvent::WorkflowCompleted {
            workflow_id: "wf-123".into(),
            status: "completed".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"WorkflowCompleted\""));
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        if let AuditEvent::WorkflowCompleted { status, .. } = restored {
            assert_eq!(status, "completed");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn webhook_received_serde_roundtrip() {
        let event = AuditEvent::WebhookReceived {
            trigger_name: "github-push".into(),
            source_ip: "192.168.1.1".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::WebhookReceived {
            trigger_name,
            source_ip,
        } = restored
        {
            assert_eq!(trigger_name, "github-push");
            assert_eq!(source_ip, "192.168.1.1");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn backup_completed_serde_roundtrip() {
        let event = AuditEvent::BackupCompleted {
            size_bytes: 1_048_576,
            destination: "s3://aivyx-backups/2026-03-07.tar.gz.enc".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::BackupCompleted {
            size_bytes,
            destination,
        } = restored
        {
            assert_eq!(size_bytes, 1_048_576);
            assert!(destination.contains("s3://"));
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn backup_failed_serde_roundtrip() {
        let event = AuditEvent::BackupFailed {
            reason: "S3 connection timeout".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"BackupFailed\""));
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        if let AuditEvent::BackupFailed { reason } = restored {
            assert_eq!(reason, "S3 connection timeout");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn sso_login_succeeded_serde_roundtrip() {
        let event = AuditEvent::SsoLoginSucceeded {
            user_id: "alice@example.com".into(),
            provider: "okta".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::SsoLoginSucceeded { user_id, provider } = restored {
            assert_eq!(user_id, "alice@example.com");
            assert_eq!(provider, "okta");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn sso_login_failed_serde_roundtrip() {
        let event = AuditEvent::SsoLoginFailed {
            reason: "invalid signature".into(),
            provider: "azure-ad".into(),
        };
        let restored = roundtrip(&event);
        if let AuditEvent::SsoLoginFailed { reason, provider } = restored {
            assert_eq!(reason, "invalid signature");
            assert_eq!(provider, "azure-ad");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn capability_audit_generated_serde_roundtrip() {
        let event = AuditEvent::CapabilityAuditGenerated {
            warnings_count: 7,
            agents_scanned: 12,
        };
        let restored = roundtrip(&event);
        if let AuditEvent::CapabilityAuditGenerated {
            warnings_count,
            agents_scanned,
        } = restored
        {
            assert_eq!(warnings_count, 7);
            assert_eq!(agents_scanned, 12);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn security_alert_serde_roundtrip() {
        let event = AuditEvent::SecurityAlert {
            alert_type: "HighFrequency".into(),
            agent_id: AgentId::new(),
            details: "Agent made 500 tool calls in 60s".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"SecurityAlert\""));
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        if let AuditEvent::SecurityAlert {
            alert_type,
            details,
            ..
        } = restored
        {
            assert_eq!(alert_type, "HighFrequency");
            assert_eq!(details, "Agent made 500 tool calls in 60s");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn ws_frame_too_large_serde_roundtrip() {
        let event = AuditEvent::WebSocketFrameTooLarge {
            size_bytes: 2_000_000,
            max_bytes: 1_048_576,
        };
        let restored = roundtrip(&event);
        if let AuditEvent::WebSocketFrameTooLarge {
            size_bytes,
            max_bytes,
        } = restored
        {
            assert_eq!(size_bytes, 2_000_000);
            assert_eq!(max_bytes, 1_048_576);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn conversation_compressed_serde_roundtrip() {
        let event = AuditEvent::ConversationCompressed {
            agent_id: AgentId::new(),
            messages_before: 42,
            messages_after: 17,
        };
        let restored = roundtrip(&event);
        if let AuditEvent::ConversationCompressed {
            messages_before,
            messages_after,
            ..
        } = restored
        {
            assert_eq!(messages_before, 42);
            assert_eq!(messages_after, 17);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn memory_extraction_completed_serde_roundtrip() {
        let event = AuditEvent::MemoryExtractionCompleted {
            agent_id: AgentId::new(),
            items_extracted: 5,
            input_tokens: 800,
            output_tokens: 120,
        };
        let restored = roundtrip(&event);
        if let AuditEvent::MemoryExtractionCompleted {
            items_extracted,
            input_tokens,
            output_tokens,
            ..
        } = restored
        {
            assert_eq!(items_extracted, 5);
            assert_eq!(input_tokens, 800);
            assert_eq!(output_tokens, 120);
        } else {
            panic!("wrong variant");
        }
    }
}
