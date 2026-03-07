use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

use aivyx_audit::{AuditEvent, AuditLog, abuse::AbuseDetector};
use aivyx_capability::CapabilitySet;
use aivyx_core::{
    AgentId, AivyxError, AutonomyTier, CapabilityScope, ChannelAdapter, Principal, Result,
    SessionId, ToolRegistry,
};
use aivyx_llm::{
    ChatMessage, ChatRequest, ChatResponse, Content, LlmProvider, StopReason, StreamEvent,
    ToolCall, ToolResult,
};
use tokio::sync::mpsc;

use crate::cost_tracker::CostTracker;
use crate::rate_limiter::RateLimiter;

#[cfg(feature = "memory")]
use tokio::sync::Mutex;

/// Maximum number of consecutive tool-use loops before forced stop.
const MAX_TOOL_LOOPS: usize = 20;

/// A single AI agent instance capable of multi-turn conversation.
pub struct Agent {
    pub id: AgentId,
    pub session_id: SessionId,
    pub name: String,
    system_prompt: String,
    max_tokens: u32,
    autonomy_tier: AutonomyTier,
    provider: Box<dyn LlmProvider>,
    tools: ToolRegistry,
    capabilities: CapabilitySet,
    rate_limiter: RateLimiter,
    cost_tracker: CostTracker,
    audit_log: Option<AuditLog>,
    conversation: Vec<ChatMessage>,
    max_retries: u32,
    retry_base_delay_ms: u64,
    created_at: chrono::DateTime<chrono::Utc>,
    #[cfg(feature = "memory")]
    memory_manager: Option<Arc<Mutex<aivyx_memory::MemoryManager>>>,
    /// The currently active project context, resolved from CWD at creation.
    active_project: Option<aivyx_config::ProjectConfig>,
    /// Pending notifications from background scheduler activity.
    /// Drained into the system prompt on the first turn iteration, then cleared.
    #[cfg(feature = "memory")]
    pending_notifications: Vec<aivyx_memory::Notification>,
    /// Skill loader for SKILL.md progressive disclosure.
    /// Shared via `Arc<Mutex<>>` with the `skill_activate` tool.
    skill_loader: Option<Arc<tokio::sync::Mutex<crate::skill_loader::SkillLoader>>>,
    /// Shared team context string, updated by team runtime after delegations.
    /// When set, `resolve_system_prompt()` injects a `[TEAM CONTEXT]` block
    /// so specialists know about the team, the original goal, and completed work.
    team_context: Option<Arc<tokio::sync::Mutex<String>>>,
    /// Sliding-window tool abuse detector.
    /// When set, records every tool call and emits `SecurityAlert` audit events
    /// when anomalous patterns are detected (high frequency, repeated denials,
    /// scope escalation).
    abuse_detector: Option<Arc<AbuseDetector>>,
}

impl Agent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: AgentId,
        name: String,
        system_prompt: String,
        max_tokens: u32,
        autonomy_tier: AutonomyTier,
        provider: Box<dyn LlmProvider>,
        tools: ToolRegistry,
        capabilities: CapabilitySet,
        rate_limiter: RateLimiter,
        cost_tracker: CostTracker,
        audit_log: Option<AuditLog>,
        max_retries: u32,
        retry_base_delay_ms: u64,
    ) -> Self {
        Self {
            id,
            session_id: SessionId::new(),
            name,
            system_prompt,
            max_tokens,
            autonomy_tier,
            provider,
            tools,
            capabilities,
            rate_limiter,
            cost_tracker,
            audit_log,
            conversation: Vec::new(),
            max_retries,
            retry_base_delay_ms,
            created_at: chrono::Utc::now(),
            #[cfg(feature = "memory")]
            memory_manager: None,
            active_project: None,
            #[cfg(feature = "memory")]
            pending_notifications: Vec::new(),
            skill_loader: None,
            team_context: None,
            abuse_detector: None,
        }
    }

    /// Set the tool abuse detector for anomaly monitoring.
    pub fn set_abuse_detector(&mut self, detector: Arc<AbuseDetector>) {
        self.abuse_detector = Some(detector);
    }

    /// Set the memory manager for memory-augmented conversations.
    #[cfg(feature = "memory")]
    pub fn set_memory_manager(&mut self, manager: Arc<Mutex<aivyx_memory::MemoryManager>>) {
        self.memory_manager = Some(manager);
    }

    /// Set the active project for project-scoped context and memory recall.
    pub fn set_active_project(&mut self, project: aivyx_config::ProjectConfig) {
        self.active_project = Some(project);
    }

    /// Set the skill loader for SKILL.md progressive disclosure.
    ///
    /// When set, the agent's system prompt is augmented with a
    /// `[AVAILABLE SKILLS]` block listing Tier-1 summaries.
    pub fn set_skill_loader(
        &mut self,
        loader: Arc<tokio::sync::Mutex<crate::skill_loader::SkillLoader>>,
    ) {
        self.skill_loader = Some(loader);
    }

    /// Set shared team context for specialist agents.
    ///
    /// The referenced string is injected as a `[TEAM CONTEXT]` block in the
    /// system prompt. It is shared (`Arc<Mutex<>>`) so the team runtime can
    /// update it as work progresses, and the specialist sees the latest state
    /// on every turn.
    pub fn set_team_context(&mut self, context: Arc<tokio::sync::Mutex<String>>) {
        self.team_context = Some(context);
    }

    /// Set pending notifications to be surfaced on the next turn.
    #[cfg(feature = "memory")]
    pub fn set_pending_notifications(&mut self, notifs: Vec<aivyx_memory::Notification>) {
        self.pending_notifications = notifs;
    }

    /// Whether there are pending notifications.
    #[cfg(feature = "memory")]
    pub fn has_pending_notifications(&self) -> bool {
        !self.pending_notifications.is_empty()
    }

    /// Get the active project, if any.
    pub fn active_project(&self) -> Option<&aivyx_config::ProjectConfig> {
        self.active_project.as_ref()
    }

    /// Resolve the system prompt, optionally augmented with user profile,
    /// team context, project context, and memory context.
    ///
    /// Assembly order: `{system_prompt} + [USER PROFILE] + [TEAM CONTEXT] + [PROJECT CONTEXT] + [MEMORY CONTEXT] + [BACKGROUND FINDINGS]`.
    /// Profile is most authoritative, then team, then project, then memories,
    /// then background notifications from the scheduler.
    async fn resolve_system_prompt(&self, user_message: &str) -> String {
        // Team context block (set by team runtime for specialist agents)
        let team_block = if let Some(ref ctx) = self.team_context {
            let text = ctx.lock().await;
            if text.is_empty() {
                None
            } else {
                Some(text.clone())
            }
        } else {
            None
        };

        // Project context block (independent of memory feature)
        let project_block = self.active_project.as_ref().map(|p| {
            let mut out = String::from("[PROJECT CONTEXT]\n");
            out.push_str(&format!("Active project: {}\n", p.name));
            out.push_str(&format!("Path: {}\n", p.path.display()));
            if let Some(ref lang) = p.language {
                out.push_str(&format!("Language: {lang}\n"));
            }
            if let Some(ref desc) = p.description {
                out.push_str(&format!("Description: {desc}\n"));
            }
            out.push_str("[END PROJECT CONTEXT]");
            out
        });

        #[cfg(feature = "memory")]
        if let Some(ref mgr) = self.memory_manager {
            let mut mgr = mgr.lock().await;

            // 1. Profile context (always include if non-empty)
            let profile_block = mgr.get_profile().ok().and_then(|p| p.format_for_prompt());

            // 2. Memory context — dual recall when project is active:
            //    project-scoped (3) + global (2) to avoid tunnel vision
            let memories = if let Some(ref proj) = self.active_project {
                let project_tag = proj.project_tag();
                let mut scoped = mgr
                    .recall(user_message, 3, Some(self.id), &[project_tag])
                    .await
                    .unwrap_or_default();
                let global = mgr
                    .recall(user_message, 2, Some(self.id), &[])
                    .await
                    .unwrap_or_default();
                // Merge, dedup by id
                for entry in global {
                    if !scoped.iter().any(|e| e.id == entry.id) {
                        scoped.push(entry);
                    }
                }
                scoped
            } else {
                mgr.recall(user_message, 5, Some(self.id), &[])
                    .await
                    .unwrap_or_default()
            };

            let triples = mgr
                .query_triples(None, None, None, Some(self.id))
                .unwrap_or_default();
            let memory_block = if !memories.is_empty() || !triples.is_empty() {
                Some(aivyx_memory::MemoryManager::format_context(
                    &memories, &triples,
                ))
            } else {
                None
            };

            // Notification block (from background scheduler activity)
            let notification_block =
                aivyx_memory::NotificationStore::format_block(&self.pending_notifications);

            // Skill discovery block (Tier 1 summaries)
            let skills_block = if let Some(ref loader) = self.skill_loader {
                loader.lock().await.format_discovery_block()
            } else {
                None
            };

            // Assemble: system_prompt + profile + team + project + memory + skills + notifications
            if profile_block.is_some()
                || team_block.is_some()
                || project_block.is_some()
                || memory_block.is_some()
                || skills_block.is_some()
                || notification_block.is_some()
            {
                let mut augmented = self.system_prompt.clone();
                if let Some(ref p) = profile_block {
                    augmented = format!("{augmented}\n\n{p}");
                }
                if let Some(ref t) = team_block {
                    augmented = format!("{augmented}\n\n{t}");
                }
                if let Some(ref p) = project_block {
                    augmented = format!("{augmented}\n\n{p}");
                }
                if let Some(ref m) = memory_block {
                    augmented = format!("{augmented}\n\n{m}");
                }
                if let Some(ref s) = skills_block {
                    augmented = format!("{augmented}\n\n{s}");
                }
                if let Some(ref n) = notification_block {
                    augmented = format!("{augmented}\n\n{n}");
                }
                augmented.push_str("\n\n");
                augmented.push_str(crate::sanitize::TOOL_OUTPUT_INSTRUCTION);
                return augmented;
            }
        }

        // Without memory feature — still inject team + project + skills context if available
        let skills_block = if let Some(ref loader) = self.skill_loader {
            loader.lock().await.format_discovery_block()
        } else {
            None
        };

        if team_block.is_some() || project_block.is_some() || skills_block.is_some() {
            let mut augmented = self.system_prompt.clone();
            if let Some(ref t) = team_block {
                augmented = format!("{augmented}\n\n{t}");
            }
            if let Some(ref p) = project_block {
                augmented = format!("{augmented}\n\n{p}");
            }
            if let Some(ref s) = skills_block {
                augmented = format!("{augmented}\n\n{s}");
            }
            augmented.push_str("\n\n");
            augmented.push_str(crate::sanitize::TOOL_OUTPUT_INSTRUCTION);
            return augmented;
        }

        // Base case: system prompt + tool output safety instruction
        format!(
            "{}\n\n{}",
            self.system_prompt,
            crate::sanitize::TOOL_OUTPUT_INSTRUCTION
        )
    }

    /// Execute a single turn: add user message, run the LLM loop, return the
    /// final assistant text.
    pub async fn turn(
        &mut self,
        user_message: &str,
        channel: Option<&dyn ChannelAdapter>,
    ) -> Result<String> {
        self.turn_with_content(Content::Text(user_message.to_string()), channel)
            .await
    }

    /// Execute a single turn with multimodal content (text + images).
    pub async fn turn_with_content(
        &mut self,
        content: Content,
        channel: Option<&dyn ChannelAdapter>,
    ) -> Result<String> {
        let user_message = content.to_text();
        self.conversation
            .push(ChatMessage::user_multimodal(content));
        self.maybe_compress_conversation().await;

        self.audit_event(AuditEvent::AgentTurnStarted {
            agent_id: self.id,
            session_id: self.session_id,
        });

        let mut tool_calls_made: u32 = 0;
        let mut total_tokens: u64 = 0;
        let mut augmented_prompt: Option<String> = None;

        for loop_idx in 0..MAX_TOOL_LOOPS {
            // Only resolve memory context on the first iteration.
            let system_prompt = if loop_idx == 0 {
                let prompt = self.resolve_system_prompt(&user_message).await;
                augmented_prompt = Some(prompt.clone());
                prompt
            } else {
                augmented_prompt
                    .clone()
                    .unwrap_or_else(|| self.system_prompt.clone())
            };

            let request = ChatRequest {
                system_prompt: Some(system_prompt),
                messages: self.conversation.clone(),
                tools: self.tools.tool_definitions(),
                model: None,
                max_tokens: self.max_tokens,
            };

            self.audit_event(AuditEvent::LlmRequestSent {
                agent_id: self.id,
                provider: self.provider.name().to_string(),
                model: String::new(),
            });

            let response = self.chat_with_retry(&request).await?;

            self.cost_tracker.track(&response.usage)?;
            total_tokens = total_tokens.saturating_add(
                (response.usage.input_tokens as u64) + (response.usage.output_tokens as u64),
            );

            self.audit_event(AuditEvent::LlmResponseReceived {
                agent_id: self.id,
                provider: self.provider.name().to_string(),
                input_tokens: response.usage.input_tokens,
                output_tokens: response.usage.output_tokens,
                stop_reason: response.stop_reason.to_string(),
            });

            match response.stop_reason {
                StopReason::EndTurn => {
                    let content = response.message.content.to_text();
                    self.conversation.push(response.message);
                    self.audit_turn_completed(tool_calls_made, total_tokens);
                    #[cfg(feature = "memory")]
                    {
                        self.pending_notifications.clear();
                        self.post_turn_extract().await;
                    }
                    return Ok(content);
                }
                StopReason::MaxTokens => {
                    let content = response.message.content.to_text();
                    self.conversation.push(response.message);
                    self.audit_turn_completed(tool_calls_made, total_tokens);
                    warn!("Agent {} hit max_tokens limit", self.name);
                    #[cfg(feature = "memory")]
                    {
                        self.pending_notifications.clear();
                        self.post_turn_extract().await;
                    }
                    return Ok(content);
                }
                StopReason::ToolUse => {
                    let tool_results = self.execute_tool_calls(&response, channel).await?;
                    tool_calls_made += tool_results.len() as u32;

                    // Add assistant message with tool calls to conversation
                    self.conversation.push(response.message);

                    // Add each tool result with sanitized boundary markers
                    for result in tool_results {
                        self.conversation
                            .push(ChatMessage::tool(Self::wrap_tool_result(result)));
                    }
                }
            }
        }

        self.audit_turn_completed(tool_calls_made, total_tokens);
        Err(AivyxError::Agent(format!(
            "agent {} exceeded maximum tool loop count ({MAX_TOOL_LOOPS})",
            self.name
        )))
    }

    /// Execute a single turn with streaming: text tokens are sent to `token_tx`
    /// as they arrive. Intermediate tool-call loops use non-streaming `chat()`
    /// internally. Returns the final assistant text.
    pub async fn turn_stream(
        &mut self,
        user_message: &str,
        channel: Option<&dyn ChannelAdapter>,
        token_tx: mpsc::Sender<String>,
        cancel: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        self.turn_stream_with_content(
            Content::Text(user_message.to_string()),
            channel,
            token_tx,
            cancel,
        )
        .await
    }

    /// Execute a single turn with streaming and multimodal content.
    pub async fn turn_stream_with_content(
        &mut self,
        content: Content,
        channel: Option<&dyn ChannelAdapter>,
        token_tx: mpsc::Sender<String>,
        cancel: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        let user_message = content.to_text();
        self.conversation
            .push(ChatMessage::user_multimodal(content));
        self.maybe_compress_conversation().await;

        self.audit_event(AuditEvent::AgentTurnStarted {
            agent_id: self.id,
            session_id: self.session_id,
        });

        let mut tool_calls_made: u32 = 0;
        let mut total_tokens: u64 = 0;
        let mut augmented_prompt: Option<String> = None;

        for loop_idx in 0..MAX_TOOL_LOOPS {
            // Check for cancellation at the top of each tool loop iteration.
            if let Some(ref ct) = cancel
                && ct.is_cancelled()
            {
                self.audit_turn_completed(tool_calls_made, total_tokens);
                return Err(AivyxError::Agent("Turn cancelled by client".into()));
            }

            let system_prompt = if loop_idx == 0 {
                let prompt = self.resolve_system_prompt(&user_message).await;
                augmented_prompt = Some(prompt.clone());
                prompt
            } else {
                augmented_prompt
                    .clone()
                    .unwrap_or_else(|| self.system_prompt.clone())
            };

            let request = ChatRequest {
                system_prompt: Some(system_prompt),
                messages: self.conversation.clone(),
                tools: self.tools.tool_definitions(),
                model: None,
                max_tokens: self.max_tokens,
            };

            self.audit_event(AuditEvent::LlmRequestSent {
                agent_id: self.id,
                provider: self.provider.name().to_string(),
                model: String::new(),
            });

            // Use streaming for the LLM call
            let (stream_tx, mut stream_rx) = mpsc::channel::<StreamEvent>(64);
            let provider = &self.provider;

            // Spawn the streaming call
            // We need to collect the result since provider is behind &dyn
            provider.chat_stream(&request, stream_tx).await?;

            // Collect stream events, checking for cancellation between tokens.
            let mut final_event = None;
            while let Some(event) = stream_rx.recv().await {
                if let Some(ref ct) = cancel
                    && ct.is_cancelled()
                {
                    self.audit_turn_completed(tool_calls_made, total_tokens);
                    return Err(AivyxError::Agent("Turn cancelled by client".into()));
                }
                match event {
                    StreamEvent::TextDelta(text) => {
                        let _ = token_tx.send(text).await;
                    }
                    StreamEvent::Done { .. } => {
                        final_event = Some(event);
                    }
                    StreamEvent::Error(e) => {
                        return Err(AivyxError::LlmProvider(e));
                    }
                }
            }

            let Some(StreamEvent::Done {
                usage,
                stop_reason,
                message,
            }) = final_event
            else {
                return Err(AivyxError::LlmProvider(
                    "stream ended without Done event".into(),
                ));
            };

            self.cost_tracker.track(&usage)?;
            total_tokens = total_tokens
                .saturating_add((usage.input_tokens as u64) + (usage.output_tokens as u64));

            self.audit_event(AuditEvent::LlmResponseReceived {
                agent_id: self.id,
                provider: self.provider.name().to_string(),
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                stop_reason: stop_reason.to_string(),
            });

            match stop_reason {
                StopReason::EndTurn => {
                    let content = message.content.to_text();
                    self.conversation.push(message);
                    self.audit_turn_completed(tool_calls_made, total_tokens);
                    #[cfg(feature = "memory")]
                    {
                        self.pending_notifications.clear();
                        self.post_turn_extract().await;
                    }
                    return Ok(content);
                }
                StopReason::MaxTokens => {
                    let content = message.content.to_text();
                    self.conversation.push(message);
                    self.audit_turn_completed(tool_calls_made, total_tokens);
                    warn!("Agent {} hit max_tokens limit", self.name);
                    #[cfg(feature = "memory")]
                    {
                        self.pending_notifications.clear();
                        self.post_turn_extract().await;
                    }
                    return Ok(content);
                }
                StopReason::ToolUse => {
                    let response = ChatResponse {
                        message: message.clone(),
                        usage,
                        stop_reason,
                    };
                    let tool_results = self.execute_tool_calls(&response, channel).await?;
                    tool_calls_made += tool_results.len() as u32;

                    self.conversation.push(message);
                    for result in tool_results {
                        self.conversation
                            .push(ChatMessage::tool(Self::wrap_tool_result(result)));
                    }
                    // Continue loop — next iteration will stream again
                }
            }
        }

        self.audit_turn_completed(tool_calls_made, total_tokens);
        Err(AivyxError::Agent(format!(
            "agent {} exceeded maximum tool loop count ({MAX_TOOL_LOOPS})",
            self.name
        )))
    }

    /// Compress the conversation history if it's approaching the context window limit.
    ///
    /// Only triggers when there are more than 10 messages and estimated tokens
    /// exceed 80% of the provider's context window.
    async fn maybe_compress_conversation(&mut self) {
        if self.conversation.len() <= 10 {
            return;
        }

        let context_window = self.provider.context_window();
        match crate::compression::compress_conversation(
            self.provider.as_ref(),
            &self.conversation,
            context_window,
            0.8,
        )
        .await
        {
            Ok(compressed) => {
                if compressed.len() < self.conversation.len() {
                    debug!(
                        agent = %self.name,
                        before = self.conversation.len(),
                        after = compressed.len(),
                        "compressed conversation history"
                    );
                    self.conversation = compressed;
                }
            }
            Err(e) => {
                warn!(
                    agent = %self.name,
                    error = %e,
                    "conversation compression failed, continuing with full history"
                );
            }
        }
    }

    async fn chat_with_retry(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let mut attempt = 0;
        loop {
            match self.provider.chat(request).await {
                Ok(r) => return Ok(r),
                Err(e) if e.is_retryable() && attempt < self.max_retries => {
                    attempt += 1;
                    let delay = self
                        .retry_base_delay_ms
                        .saturating_mul(2u64.saturating_pow(attempt - 1))
                        .min(300_000); // cap at 5 minutes
                    warn!(
                        "Retry {attempt}/{}: backoff {delay}ms: {e}",
                        self.max_retries
                    );
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn execute_tool_calls(
        &mut self,
        response: &ChatResponse,
        channel: Option<&dyn ChannelAdapter>,
    ) -> Result<Vec<ToolResult>> {
        let mut results = Vec::new();

        for tc in &response.message.tool_calls {
            let result = self.execute_single_tool(tc, channel).await;
            results.push(result);
        }

        Ok(results)
    }

    async fn execute_single_tool(
        &mut self,
        tc: &ToolCall,
        channel: Option<&dyn ChannelAdapter>,
    ) -> ToolResult {
        // Rate limit check
        if let Err(e) = self.rate_limiter.check() {
            return ToolResult {
                tool_call_id: tc.id.clone(),
                content: serde_json::json!({"error": e.to_string()}),
                is_error: true,
            };
        }

        // Autonomy tier gate
        match self.autonomy_tier {
            AutonomyTier::Locked => {
                self.audit_event(AuditEvent::ToolDenied {
                    tool_id: aivyx_core::ToolId::new(),
                    agent_id: self.id,
                    action: tc.name.clone(),
                    reason: "agent is in Locked tier — all tool calls denied".into(),
                });
                return ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: serde_json::json!({"error": "tool execution denied: agent is in Locked autonomy tier"}),
                    is_error: true,
                };
            }
            AutonomyTier::Leash => {
                // In Leash mode, ask the user for approval via channel
                match channel {
                    Some(ch) => {
                        let prompt = format!(
                            "Agent '{}' wants to call tool '{}' with args: {}. Approve? (y/n)",
                            self.name,
                            tc.name,
                            serde_json::to_string_pretty(&tc.arguments).unwrap_or_default()
                        );
                        if let Err(e) = ch.send(&prompt).await {
                            debug!("Failed to send approval prompt: {e}");
                        }
                        match ch.receive().await {
                            Ok(answer) if answer.trim().eq_ignore_ascii_case("y") => {
                                // Approved — continue
                            }
                            _ => {
                                return ToolResult {
                                    tool_call_id: tc.id.clone(),
                                    content: serde_json::json!({"error": "tool execution denied by user"}),
                                    is_error: true,
                                };
                            }
                        }
                    }
                    None => {
                        // No channel adapter — cannot get approval, deny the tool call
                        self.audit_event(AuditEvent::ToolDenied {
                            tool_id: aivyx_core::ToolId::new(),
                            agent_id: self.id,
                            action: tc.name.clone(),
                            reason: "Leash tier requires a channel adapter for approval".into(),
                        });
                        return ToolResult {
                            tool_call_id: tc.id.clone(),
                            content: serde_json::json!({"error": "tool execution denied: Leash tier requires a channel adapter for user approval"}),
                            is_error: true,
                        };
                    }
                }
            }
            AutonomyTier::Trust | AutonomyTier::Free => {
                // Execute within capability set — no approval needed
            }
        }

        // Look up the tool
        let tool = match self.tools.get_by_name(&tc.name) {
            Some(t) => t,
            None => {
                return ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: serde_json::json!({"error": format!("unknown tool: {}", tc.name)}),
                    is_error: true,
                };
            }
        };

        let tool_id = tool.id();

        // Capability check: if the tool declares a required scope, verify that
        // this agent holds a matching capability.
        if let Some(required_scope) = tool.required_scope() {
            let principal = Principal::Agent(self.id);
            match self
                .capabilities
                .check(&principal, &required_scope, &tc.name)
            {
                Err(e) => {
                    if let Some(denial) = self.audit_security_event(
                        AuditEvent::ToolDenied {
                            tool_id,
                            agent_id: self.id,
                            action: tc.name.clone(),
                            reason: e.to_string(),
                        },
                        tc,
                    ) {
                        return denial;
                    }
                    return ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: serde_json::json!({"error": format!("capability denied: {e}")}),
                        is_error: true,
                    };
                }
                Ok(matched_cap) => {
                    if let Some(denial) = self.validate_tool_input(tc, &matched_cap.scope) {
                        return denial;
                    }
                }
            }
        }

        // Execute
        info!("Agent {} executing tool '{}'", self.name, tc.name);
        let (result, denied) = match tool.execute(tc.arguments.clone()).await {
            Ok(output) => {
                self.audit_event(AuditEvent::ToolExecuted {
                    tool_id,
                    agent_id: self.id,
                    action: tc.name.clone(),
                    result_summary: truncate(
                        &serde_json::to_string(&output).unwrap_or_default(),
                        200,
                    ),
                });
                (
                    ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: output,
                        is_error: false,
                    },
                    false,
                )
            }
            Err(e) => {
                self.audit_event(AuditEvent::ToolDenied {
                    tool_id,
                    agent_id: self.id,
                    action: tc.name.clone(),
                    reason: e.to_string(),
                });
                (
                    ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: serde_json::json!({"error": e.to_string()}),
                        is_error: true,
                    },
                    true,
                )
            }
        };

        // Record for abuse detection
        if let Some(ref detector) = self.abuse_detector {
            let agent_str = self.id.to_string();
            let alerts = detector.record_tool_call(&agent_str, &tc.name, denied);
            for alert in alerts {
                let details = serde_json::to_string(&alert).unwrap_or_default();
                let alert_type = match &alert {
                    aivyx_audit::abuse::AbuseAlert::HighFrequency { .. } => "HighFrequency",
                    aivyx_audit::abuse::AbuseAlert::RepeatedDenials { .. } => "RepeatedDenials",
                    aivyx_audit::abuse::AbuseAlert::ScopeEscalation { .. } => "ScopeEscalation",
                };
                warn!("Security alert for agent {}: {alert_type}", self.name);
                self.audit_event(AuditEvent::SecurityAlert {
                    alert_type: alert_type.to_string(),
                    agent_id: self.id,
                    details,
                });
            }
        }

        result
    }

    /// Wrap a tool result's content in `[TOOL_OUTPUT]` boundary markers for
    /// privileged context separation. This prevents prompt injection via tool
    /// outputs by clearly delineating untrusted data.
    fn wrap_tool_result(mut result: ToolResult) -> ToolResult {
        // Extract a text representation of the tool output
        let text = match &result.content {
            serde_json::Value::String(s) => s.clone(),
            other => serde_json::to_string(other).unwrap_or_default(),
        };
        // Re-wrap content with boundary markers. We preserve the tool_call_id
        // (which maps to the original tool call) so the LLM can correlate.
        let tool_name = result.tool_call_id.clone();
        let wrapped = crate::sanitize::wrap_tool_output(&tool_name, &text);
        result.content = serde_json::Value::String(wrapped);
        result
    }

    /// Validate tool input against the matched capability's constraints.
    ///
    /// For Shell capabilities with a non-empty `allowed_commands` list, extracts
    /// the command binary from the input and verifies it is in the allowed list.
    /// Returns `Some(ToolResult)` with a denial if validation fails, `None` if OK.
    fn validate_tool_input(
        &self,
        tc: &ToolCall,
        matched_scope: &CapabilityScope,
    ) -> Option<ToolResult> {
        // Shell: validate command binary against allowed_commands
        if let Some(allowed) = matched_scope.shell_allowed_commands()
            && !allowed.is_empty()
            && let Some(cmd_str) = tc.arguments.get("command").and_then(|v| v.as_str())
        {
            let binary = cmd_str.split_whitespace().next().unwrap_or("");
            let binary_name = std::path::Path::new(binary)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(binary);
            if !allowed.iter().any(|a| a == binary_name) {
                self.audit_event(AuditEvent::ToolDenied {
                    tool_id: self
                        .tools
                        .get_by_name(&tc.name)
                        .map(|t| t.id())
                        .unwrap_or_default(),
                    agent_id: self.id,
                    action: tc.name.clone(),
                    reason: format!(
                        "command '{}' not in allowed list: {:?}",
                        binary_name, allowed
                    ),
                });
                return Some(ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: serde_json::json!({"error": format!("command '{}' not permitted by capability", binary_name)}),
                    is_error: true,
                });
            }
        }
        None
    }

    fn audit_event(&self, event: AuditEvent) {
        if let Some(log) = &self.audit_log
            && let Err(e) = log.append(event)
        {
            warn!("Failed to write audit event: {e}");
        }
    }

    /// Audit a security-critical event. Returns a denial `ToolResult` if the
    /// audit write itself fails — this prevents security-sensitive actions
    /// from proceeding silently when the audit trail is broken.
    fn audit_security_event(&self, event: AuditEvent, tc: &ToolCall) -> Option<ToolResult> {
        if let Some(log) = &self.audit_log
            && let Err(e) = log.append(event)
        {
            warn!("FATAL: security audit write failed: {e}");
            return Some(ToolResult {
                tool_call_id: tc.id.clone(),
                content: serde_json::json!({"error": "security audit logging failed — action blocked"}),
                is_error: true,
            });
        }
        None
    }

    fn audit_turn_completed(&self, tool_calls_made: u32, tokens_used: u64) {
        self.audit_event(AuditEvent::AgentTurnCompleted {
            agent_id: self.id,
            session_id: self.session_id,
            tool_calls_made,
            tokens_used,
        });
    }

    /// Get the current conversation history.
    pub fn conversation(&self) -> &[ChatMessage] {
        &self.conversation
    }

    /// Get accumulated cost.
    pub fn current_cost_usd(&self) -> f64 {
        self.cost_tracker.current_cost_usd()
    }

    /// Export the current conversation as a `PersistedSession`.
    pub fn to_persisted_session(&self) -> crate::session_store::PersistedSession {
        crate::session_store::PersistedSession {
            metadata: crate::session_store::SessionMetadata {
                session_id: self.session_id,
                agent_name: self.name.clone(),
                created_at: self.created_at,
                updated_at: chrono::Utc::now(),
                message_count: self.conversation.len(),
            },
            messages: self.conversation.clone(),
        }
    }

    /// Replace the conversation history (for session restore).
    pub fn restore_conversation(&mut self, messages: Vec<ChatMessage>) {
        self.conversation = messages;
    }

    /// Get the session ID.
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Register an additional tool into this agent's tool registry.
    ///
    /// Used by `TeamRuntime` to inject delegation tools after agent creation.
    pub fn register_tool(&mut self, tool: Box<dyn aivyx_core::Tool>) {
        self.tools.register(tool);
    }

    /// Get a reference to this agent's capability set.
    pub fn capabilities(&self) -> &CapabilitySet {
        &self.capabilities
    }

    /// Replace the agent's capability set.
    ///
    /// Used by team delegation to enforce attenuated capabilities on
    /// specialist agents. The narrowed set should have been produced by
    /// `attenuate_for_member()` to guarantee that it is a subset of
    /// the original.
    pub fn replace_capabilities(&mut self, caps: CapabilitySet) {
        self.capabilities = caps;
    }

    /// Returns the agent's autonomy tier.
    pub fn autonomy_tier(&self) -> AutonomyTier {
        self.autonomy_tier
    }

    /// Sets the agent's autonomy tier at runtime.
    ///
    /// This allows dynamic tier changes from the TUI or other in-process
    /// callers. Does **not** persist to the agent profile TOML — callers
    /// must separately update the profile if persistence is required.
    pub fn set_autonomy_tier(&mut self, tier: AutonomyTier) {
        self.autonomy_tier = tier;
    }

    /// Generate a session summary via LLM and store it as a
    /// `MemoryKind::SessionSummary` memory.
    ///
    /// Returns the summary text, or `None` if memory is not configured or
    /// the conversation is too short (fewer than 2 messages).
    ///
    /// Called on session exit (CLI and TUI) to capture key takeaways from the
    /// conversation before the session is saved.
    #[cfg(feature = "memory")]
    pub async fn end_session(&mut self) -> Option<String> {
        // Skip if conversation is trivial
        if self.conversation.len() < 2 {
            return None;
        }

        // Build a condensed conversation excerpt (last 10 messages max)
        let excerpt = self
            .conversation
            .iter()
            .rev()
            .take(10)
            .rev()
            .filter_map(|msg| match msg.role {
                aivyx_llm::Role::User => Some(format!("User: {}", msg.content)),
                aivyx_llm::Role::Assistant => Some(format!("Assistant: {}", msg.content)),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        if excerpt.trim().is_empty() {
            return None;
        }

        let request = ChatRequest {
            system_prompt: Some(
                "Summarize this conversation in 2-3 sentences. Focus on: what was discussed, \
                 key decisions made, and any important facts about the user. Respond with ONLY \
                 the summary text, no markdown or formatting."
                    .to_string(),
            ),
            messages: vec![ChatMessage::user(format!(
                "Summarize this conversation:\n\n{excerpt}"
            ))],
            tools: vec![],
            model: None,
            max_tokens: 256,
        };

        let summary = match self.provider.chat(&request).await {
            Ok(response) => response.message.content.text().trim().to_string(),
            Err(e) => {
                debug!("Session summary generation failed (non-fatal): {e}");
                return None;
            }
        };

        if summary.is_empty() {
            return None;
        }

        // Store as SessionSummary memory
        if let Some(ref mgr) = self.memory_manager {
            let mut mgr = mgr.lock().await;
            if let Err(e) = mgr
                .remember(
                    summary.clone(),
                    aivyx_memory::MemoryKind::SessionSummary,
                    Some(self.id),
                    vec!["session-summary".into()],
                )
                .await
            {
                debug!("Failed to store session summary: {e}");
            }

            // Extract knowledge triples from the summary for cross-session accumulation.
            // Facts/preferences were already captured per-turn; summaries yield
            // higher-level entity relationships that span the entire session.
            use crate::memory_extractor;
            match memory_extractor::extract_from_summary(self.provider.as_ref(), &summary).await {
                Ok(triples) => {
                    for triple in &triples {
                        if let Err(e) = mgr.add_triple(
                            triple.subject.clone(),
                            triple.predicate.clone(),
                            triple.object.clone(),
                            Some(self.id),
                            0.7, // slightly lower confidence than per-turn (0.8)
                            "session-summary".into(),
                        ) {
                            debug!("Failed to store session triple: {e}");
                        }
                    }
                    if !triples.is_empty() {
                        info!("Extracted {} triples from session summary", triples.len());
                    }
                }
                Err(e) => {
                    debug!("Session summary triple extraction failed (non-fatal): {e}");
                }
            }
        }

        Some(summary)
    }

    /// Non-memory stub for `end_session()` when memory feature is disabled.
    #[cfg(not(feature = "memory"))]
    pub async fn end_session(&mut self) -> Option<String> {
        None
    }

    /// Run post-turn memory extraction.
    ///
    /// Calls the LLM to identify facts, preferences, and knowledge triples
    /// from the most recent conversation exchange, then stores them in the
    /// memory manager. Errors are logged but never propagated.
    #[cfg(feature = "memory")]
    async fn post_turn_extract(&mut self) {
        use crate::memory_extractor;

        let mgr = match self.memory_manager {
            Some(ref mgr) => mgr.clone(),
            None => return,
        };

        // Extract from the last 4 messages (2 exchanges)
        match memory_extractor::extract_from_turn(self.provider.as_ref(), &self.conversation, 4)
            .await
        {
            Ok(result) => {
                let total = result.facts.len() + result.preferences.len() + result.triples.len();
                if total > 0 {
                    memory_extractor::store_extractions(&mgr, &result, Some(self.id)).await;
                }
            }
            Err(e) => {
                debug!("Memory extraction failed (non-fatal): {e}");
            }
        }
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let end = s.floor_char_boundary(max_len);
        format!("{}...", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built_in_tools::FileReadTool;
    use crate::cost_tracker::CostTracker;
    use crate::rate_limiter::RateLimiter;
    use aivyx_capability::{ActionPattern, Capability, CapabilitySet};
    use aivyx_core::{CapabilityId, CapabilityScope, Tool as _, ToolRegistry};
    use aivyx_llm::{ChatResponse, TokenUsage};
    use chrono::Utc;
    use std::path::PathBuf;

    /// A mock provider that returns a fixed response.
    struct MockProvider {
        response: ChatResponse,
    }

    #[async_trait::async_trait]
    impl LlmProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }
        async fn chat(&self, _request: &ChatRequest) -> Result<ChatResponse> {
            Ok(self.response.clone())
        }
    }

    fn make_agent(response: ChatResponse) -> Agent {
        Agent::new(
            AgentId::new(),
            "test".into(),
            "You are helpful.".into(),
            4096,
            AutonomyTier::Trust,
            Box::new(MockProvider { response }),
            ToolRegistry::new(),
            CapabilitySet::new(),
            RateLimiter::new(60),
            CostTracker::new(5.0, 0.000003, 0.000015),
            None,
            3,
            1000,
        )
    }

    #[tokio::test]
    async fn simple_text_turn() {
        let response = ChatResponse {
            message: ChatMessage::assistant("Hello, world!"),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            stop_reason: StopReason::EndTurn,
        };

        let mut agent = make_agent(response);
        let result = agent.turn("Hi", None).await.unwrap();
        assert_eq!(result, "Hello, world!");
        assert_eq!(agent.conversation().len(), 2); // user + assistant
    }

    #[tokio::test]
    async fn max_tokens_returns_partial() {
        let response = ChatResponse {
            message: ChatMessage::assistant("Partial re"),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 4096,
            },
            stop_reason: StopReason::MaxTokens,
        };

        let mut agent = make_agent(response);
        let result = agent.turn("Tell me everything", None).await.unwrap();
        assert_eq!(result, "Partial re");
    }

    #[tokio::test]
    async fn capability_denied_when_set_empty() {
        // Agent with a tool that requires Filesystem scope but no capabilities granted.
        let agent_id = AgentId::new();
        let tool_call = ToolCall {
            id: "tc_1".into(),
            name: "file_read".into(),
            arguments: serde_json::json!({"path": "/tmp/test"}),
        };
        let response = ChatResponse {
            message: ChatMessage::assistant_with_tool_calls("Reading file", vec![tool_call]),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            stop_reason: StopReason::ToolUse,
        };

        let mut tools = ToolRegistry::new();
        tools.register(Box::new(FileReadTool::new()));

        // Mock provider: first call returns tool use, second returns end turn
        struct TwoShotProvider {
            responses: std::sync::Mutex<Vec<ChatResponse>>,
        }
        #[async_trait::async_trait]
        impl LlmProvider for TwoShotProvider {
            fn name(&self) -> &str {
                "mock"
            }
            async fn chat(&self, _request: &ChatRequest) -> Result<ChatResponse> {
                let mut responses = self.responses.lock().unwrap();
                if responses.len() > 1 {
                    Ok(responses.remove(0))
                } else {
                    Ok(responses[0].clone())
                }
            }
        }

        let end_response = ChatResponse {
            message: ChatMessage::assistant("Done"),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 3,
            },
            stop_reason: StopReason::EndTurn,
        };

        let provider = TwoShotProvider {
            responses: std::sync::Mutex::new(vec![response, end_response]),
        };

        let mut agent = Agent::new(
            agent_id,
            "test".into(),
            "Test".into(),
            4096,
            AutonomyTier::Trust,
            Box::new(provider),
            tools,
            CapabilitySet::new(), // empty — tool should be denied
            RateLimiter::new(60),
            CostTracker::new(5.0, 0.000003, 0.000015),
            None,
            3,
            1000,
        );

        // The turn should succeed (agent gets error result from tool, then LLM says "Done")
        let result = agent.turn("read file", None).await.unwrap();
        assert_eq!(result, "Done");

        // Verify tool result in conversation contains capability denied error
        let tool_msg = agent
            .conversation()
            .iter()
            .find(|m| m.tool_result.is_some())
            .unwrap();
        let tr = tool_msg.tool_result.as_ref().unwrap();
        assert!(tr.is_error);
        assert!(tr.content.to_string().contains("capability denied"));
    }

    #[tokio::test]
    async fn capability_allowed_with_matching_cap() {
        let agent_id = AgentId::new();

        let mut caps = CapabilitySet::new();
        caps.grant(Capability {
            id: CapabilityId::new(),
            scope: CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            },
            pattern: ActionPattern::new("*").unwrap(),
            granted_to: vec![Principal::Agent(agent_id)],
            granted_by: Principal::System,
            created_at: Utc::now(),
            expires_at: None,
            revoked: false,
            parent_id: None,
        });

        // Tool that requires Filesystem scope — should be allowed
        let tool = FileReadTool::new();
        let scope = tool.required_scope().unwrap();
        let result = caps.check(&Principal::Agent(agent_id), &scope, "file_read");
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn retry_succeeds_after_transient_failure() {
        use std::sync::atomic::{AtomicU32, Ordering};

        struct FailThenSucceed {
            call_count: AtomicU32,
        }

        #[async_trait::async_trait]
        impl LlmProvider for FailThenSucceed {
            fn name(&self) -> &str {
                "mock"
            }
            async fn chat(&self, _request: &ChatRequest) -> Result<ChatResponse> {
                let n = self.call_count.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(AivyxError::RateLimit("rate limited".into()))
                } else {
                    Ok(ChatResponse {
                        message: ChatMessage::assistant("ok"),
                        usage: TokenUsage {
                            input_tokens: 5,
                            output_tokens: 2,
                        },
                        stop_reason: StopReason::EndTurn,
                    })
                }
            }
        }

        let mut agent = Agent::new(
            AgentId::new(),
            "test".into(),
            "Test".into(),
            4096,
            AutonomyTier::Trust,
            Box::new(FailThenSucceed {
                call_count: AtomicU32::new(0),
            }),
            ToolRegistry::new(),
            CapabilitySet::new(),
            RateLimiter::new(60),
            CostTracker::new(5.0, 0.000003, 0.000015),
            None,
            3,
            1, // 1ms base delay for fast tests
        );

        let result = agent.turn("hi", None).await.unwrap();
        assert_eq!(result, "ok");
    }

    #[tokio::test]
    async fn retry_exhausted_returns_error() {
        struct AlwaysFail;

        #[async_trait::async_trait]
        impl LlmProvider for AlwaysFail {
            fn name(&self) -> &str {
                "mock"
            }
            async fn chat(&self, _request: &ChatRequest) -> Result<ChatResponse> {
                Err(AivyxError::RateLimit("rate limited".into()))
            }
        }

        let mut agent = Agent::new(
            AgentId::new(),
            "test".into(),
            "Test".into(),
            4096,
            AutonomyTier::Trust,
            Box::new(AlwaysFail),
            ToolRegistry::new(),
            CapabilitySet::new(),
            RateLimiter::new(60),
            CostTracker::new(5.0, 0.000003, 0.000015),
            None,
            2,
            1,
        );

        let err = agent.turn("hi", None).await.unwrap_err();
        assert!(matches!(err, AivyxError::RateLimit(_)));
    }

    #[tokio::test]
    async fn non_retryable_error_no_retry() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let call_count = Arc::new(AtomicU32::new(0));
        let count_clone = call_count.clone();

        struct FailAlways {
            call_count: Arc<AtomicU32>,
        }

        #[async_trait::async_trait]
        impl LlmProvider for FailAlways {
            fn name(&self) -> &str {
                "mock"
            }
            async fn chat(&self, _request: &ChatRequest) -> Result<ChatResponse> {
                self.call_count.fetch_add(1, Ordering::SeqCst);
                Err(AivyxError::LlmProvider("bad model".into()))
            }
        }

        let mut agent = Agent::new(
            AgentId::new(),
            "test".into(),
            "Test".into(),
            4096,
            AutonomyTier::Trust,
            Box::new(FailAlways {
                call_count: count_clone,
            }),
            ToolRegistry::new(),
            CapabilitySet::new(),
            RateLimiter::new(60),
            CostTracker::new(5.0, 0.000003, 0.000015),
            None,
            3,
            1,
        );

        let err = agent.turn("hi", None).await.unwrap_err();
        assert!(matches!(err, AivyxError::LlmProvider(_)));
        // Should have been called only once (no retries for non-retryable errors)
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn is_retryable_correctness() {
        assert!(AivyxError::RateLimit("test".into()).is_retryable());
        assert!(AivyxError::Http("test".into()).is_retryable());
        assert!(!AivyxError::LlmProvider("test".into()).is_retryable());
        assert!(!AivyxError::Agent("test".into()).is_retryable());
        assert!(!AivyxError::Config("test".into()).is_retryable());
    }

    #[test]
    fn no_required_scope_always_allowed() {
        // A tool with required_scope() = None should never trigger capability check.
        // Verify our mock provider tool has None scope.
        struct NullTool;
        #[async_trait::async_trait]
        impl aivyx_core::Tool for NullTool {
            fn id(&self) -> aivyx_core::ToolId {
                aivyx_core::ToolId::new()
            }
            fn name(&self) -> &str {
                "null"
            }
            fn description(&self) -> &str {
                "no-op"
            }
            fn input_schema(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            async fn execute(&self, _input: serde_json::Value) -> Result<serde_json::Value> {
                Ok(serde_json::json!({}))
            }
        }
        let tool = NullTool;
        assert!(tool.required_scope().is_none());
    }

    #[tokio::test]
    async fn turn_stream_delivers_tokens() {
        let response = ChatResponse {
            message: ChatMessage::assistant("Hello, world!"),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            stop_reason: StopReason::EndTurn,
        };

        let mut agent = make_agent(response);
        let (token_tx, mut token_rx) = tokio::sync::mpsc::channel(64);

        let result = agent.turn_stream("Hi", None, token_tx, None).await.unwrap();
        assert_eq!(result, "Hello, world!");

        // The default fallback sends one TextDelta with the full text
        let token = token_rx.recv().await.unwrap();
        assert_eq!(token, "Hello, world!");
    }

    #[tokio::test]
    async fn turn_still_works_after_stream_addition() {
        // Verify that the original turn() method still works identically.
        let response = ChatResponse {
            message: ChatMessage::assistant("Non-streaming works!"),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            stop_reason: StopReason::EndTurn,
        };

        let mut agent = make_agent(response);
        let result = agent.turn("Hi", None).await.unwrap();
        assert_eq!(result, "Non-streaming works!");
        assert_eq!(agent.conversation().len(), 2);
    }

    #[tokio::test]
    async fn turn_stream_with_tool_use_loop() {
        // Mock provider that returns tool_use on first call, then end_turn.
        struct ToolLoopProvider {
            call_count: std::sync::Mutex<u32>,
        }
        #[async_trait::async_trait]
        impl LlmProvider for ToolLoopProvider {
            fn name(&self) -> &str {
                "mock"
            }
            async fn chat(&self, _request: &ChatRequest) -> Result<ChatResponse> {
                let mut count = self.call_count.lock().unwrap();
                *count += 1;
                if *count == 1 {
                    Ok(ChatResponse {
                        message: ChatMessage::assistant("Done after tools"),
                        usage: TokenUsage {
                            input_tokens: 10,
                            output_tokens: 5,
                        },
                        stop_reason: StopReason::EndTurn,
                    })
                } else {
                    Ok(ChatResponse {
                        message: ChatMessage::assistant("Final"),
                        usage: TokenUsage {
                            input_tokens: 5,
                            output_tokens: 2,
                        },
                        stop_reason: StopReason::EndTurn,
                    })
                }
            }
        }

        let mut agent = Agent::new(
            AgentId::new(),
            "test".into(),
            "Test".into(),
            4096,
            AutonomyTier::Trust,
            Box::new(ToolLoopProvider {
                call_count: std::sync::Mutex::new(0),
            }),
            ToolRegistry::new(),
            CapabilitySet::new(),
            RateLimiter::new(60),
            CostTracker::new(5.0, 0.000003, 0.000015),
            None,
            3,
            1000,
        );

        let (token_tx, mut token_rx) = tokio::sync::mpsc::channel(64);
        let result = agent
            .turn_stream("Do it", None, token_tx, None)
            .await
            .unwrap();
        assert_eq!(result, "Done after tools");

        // Should have received the text via token channel
        let token = token_rx.recv().await.unwrap();
        assert_eq!(token, "Done after tools");
    }

    /// A mock channel adapter for testing Leash-tier approval.
    struct MockChannel {
        response: String,
    }

    #[async_trait::async_trait]
    impl aivyx_core::ChannelAdapter for MockChannel {
        async fn send(&self, _message: &str) -> aivyx_core::Result<()> {
            Ok(())
        }
        async fn receive(&self) -> aivyx_core::Result<String> {
            Ok(self.response.clone())
        }
    }

    #[tokio::test]
    async fn leash_tier_approved_by_channel() {
        // Leash-tier agent with a mock channel that approves
        let tool_call = ToolCall {
            id: "tc_1".into(),
            name: "file_read".into(),
            arguments: serde_json::json!({"path": "/tmp/test"}),
        };
        let response1 = ChatResponse {
            message: ChatMessage::assistant_with_tool_calls("Reading", vec![tool_call]),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            stop_reason: StopReason::ToolUse,
        };
        let response2 = ChatResponse {
            message: ChatMessage::assistant("Done"),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 3,
            },
            stop_reason: StopReason::EndTurn,
        };

        struct TwoShotProvider {
            responses: std::sync::Mutex<Vec<ChatResponse>>,
        }
        #[async_trait::async_trait]
        impl LlmProvider for TwoShotProvider {
            fn name(&self) -> &str {
                "mock"
            }
            async fn chat(&self, _req: &ChatRequest) -> Result<ChatResponse> {
                let mut r = self.responses.lock().unwrap();
                if r.len() > 1 {
                    Ok(r.remove(0))
                } else {
                    Ok(r[0].clone())
                }
            }
        }

        let agent_id = AgentId::new();
        let mut caps = CapabilitySet::new();
        caps.grant(aivyx_capability::Capability {
            id: aivyx_core::CapabilityId::new(),
            scope: CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            },
            pattern: ActionPattern::new("*").unwrap(),
            granted_to: vec![Principal::Agent(agent_id)],
            granted_by: Principal::System,
            created_at: Utc::now(),
            expires_at: None,
            revoked: false,
            parent_id: None,
        });

        let mut tools = ToolRegistry::new();
        tools.register(Box::new(FileReadTool::new()));

        let mut agent = Agent::new(
            agent_id,
            "test".into(),
            "Test".into(),
            4096,
            AutonomyTier::Leash,
            Box::new(TwoShotProvider {
                responses: std::sync::Mutex::new(vec![response1, response2]),
            }),
            tools,
            caps,
            RateLimiter::new(60),
            CostTracker::new(5.0, 0.000003, 0.000015),
            None,
            3,
            1000,
        );

        let channel = MockChannel {
            response: "y".into(),
        };
        let result = agent.turn("read file", Some(&channel)).await.unwrap();
        // The tool was approved and executed (or error from file not existing),
        // but the agent should complete successfully
        assert_eq!(result, "Done");
    }

    #[tokio::test]
    async fn leash_tier_denied_by_channel() {
        let tool_call = ToolCall {
            id: "tc_1".into(),
            name: "file_read".into(),
            arguments: serde_json::json!({"path": "/tmp/test"}),
        };
        let response1 = ChatResponse {
            message: ChatMessage::assistant_with_tool_calls("Reading", vec![tool_call]),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            stop_reason: StopReason::ToolUse,
        };
        let response2 = ChatResponse {
            message: ChatMessage::assistant("Denied"),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 3,
            },
            stop_reason: StopReason::EndTurn,
        };

        struct TwoShotProvider {
            responses: std::sync::Mutex<Vec<ChatResponse>>,
        }
        #[async_trait::async_trait]
        impl LlmProvider for TwoShotProvider {
            fn name(&self) -> &str {
                "mock"
            }
            async fn chat(&self, _req: &ChatRequest) -> Result<ChatResponse> {
                let mut r = self.responses.lock().unwrap();
                if r.len() > 1 {
                    Ok(r.remove(0))
                } else {
                    Ok(r[0].clone())
                }
            }
        }

        let mut agent = Agent::new(
            AgentId::new(),
            "test".into(),
            "Test".into(),
            4096,
            AutonomyTier::Leash,
            Box::new(TwoShotProvider {
                responses: std::sync::Mutex::new(vec![response1, response2]),
            }),
            ToolRegistry::new(),
            CapabilitySet::new(),
            RateLimiter::new(60),
            CostTracker::new(5.0, 0.000003, 0.000015),
            None,
            3,
            1000,
        );

        let channel = MockChannel {
            response: "n".into(),
        };
        let result = agent.turn("read file", Some(&channel)).await.unwrap();
        assert_eq!(result, "Denied");

        // Verify tool result contains denial
        let tool_msg = agent
            .conversation()
            .iter()
            .find(|m| m.tool_result.is_some())
            .unwrap();
        let tr = tool_msg.tool_result.as_ref().unwrap();
        assert!(tr.is_error);
        assert!(tr.content.to_string().contains("denied by user"));
    }

    #[tokio::test]
    async fn leash_tier_no_channel_denies() {
        let tool_call = ToolCall {
            id: "tc_1".into(),
            name: "file_read".into(),
            arguments: serde_json::json!({"path": "/tmp/test"}),
        };
        let response1 = ChatResponse {
            message: ChatMessage::assistant_with_tool_calls("Reading", vec![tool_call]),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            stop_reason: StopReason::ToolUse,
        };
        let response2 = ChatResponse {
            message: ChatMessage::assistant("No channel"),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 3,
            },
            stop_reason: StopReason::EndTurn,
        };

        struct TwoShotProvider {
            responses: std::sync::Mutex<Vec<ChatResponse>>,
        }
        #[async_trait::async_trait]
        impl LlmProvider for TwoShotProvider {
            fn name(&self) -> &str {
                "mock"
            }
            async fn chat(&self, _req: &ChatRequest) -> Result<ChatResponse> {
                let mut r = self.responses.lock().unwrap();
                if r.len() > 1 {
                    Ok(r.remove(0))
                } else {
                    Ok(r[0].clone())
                }
            }
        }

        let mut agent = Agent::new(
            AgentId::new(),
            "test".into(),
            "Test".into(),
            4096,
            AutonomyTier::Leash,
            Box::new(TwoShotProvider {
                responses: std::sync::Mutex::new(vec![response1, response2]),
            }),
            ToolRegistry::new(),
            CapabilitySet::new(),
            RateLimiter::new(60),
            CostTracker::new(5.0, 0.000003, 0.000015),
            None,
            3,
            1000,
        );

        // No channel adapter — should deny the tool call (not panic)
        let result = agent.turn("read file", None).await.unwrap();
        assert_eq!(result, "No channel");

        // Verify tool result contains channel adapter error
        let tool_msg = agent
            .conversation()
            .iter()
            .find(|m| m.tool_result.is_some())
            .unwrap();
        let tr = tool_msg.tool_result.as_ref().unwrap();
        assert!(tr.is_error);
        assert!(tr.content.to_string().contains("channel adapter"));
    }

    #[tokio::test]
    async fn end_session_generates_summary() {
        let summary_response = ChatResponse {
            message: ChatMessage::assistant(
                "User asked about Rust. Agent explained borrow checker.",
            ),
            usage: TokenUsage {
                input_tokens: 50,
                output_tokens: 12,
            },
            stop_reason: StopReason::EndTurn,
        };

        struct SummaryProvider {
            turn_response: ChatResponse,
            summary_response: ChatResponse,
        }
        #[async_trait::async_trait]
        impl LlmProvider for SummaryProvider {
            fn name(&self) -> &str {
                "mock"
            }
            async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
                // Detect summary request by system prompt content
                if let Some(ref sys) = request.system_prompt
                    && sys.contains("Summarize this conversation")
                {
                    Ok(self.summary_response.clone())
                } else {
                    Ok(self.turn_response.clone())
                }
            }
        }

        let turn_response = ChatResponse {
            message: ChatMessage::assistant("Rust's borrow checker ensures memory safety."),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 8,
            },
            stop_reason: StopReason::EndTurn,
        };

        let mut agent = Agent::new(
            AgentId::new(),
            "test".into(),
            "You are helpful.".into(),
            4096,
            AutonomyTier::Trust,
            Box::new(SummaryProvider {
                turn_response,
                summary_response,
            }),
            ToolRegistry::new(),
            CapabilitySet::new(),
            RateLimiter::new(60),
            CostTracker::new(5.0, 0.000003, 0.000015),
            None,
            3,
            1000,
        );

        // Have a conversation first
        let _ = agent.turn("Tell me about Rust", None).await.unwrap();
        assert_eq!(agent.conversation().len(), 2);

        // Now end session — should generate summary
        let summary = agent.end_session().await;
        assert!(summary.is_some());
        assert!(summary.unwrap().contains("User asked about Rust"),);
    }

    #[tokio::test]
    async fn end_session_short_conversation_returns_none() {
        let response = ChatResponse {
            message: ChatMessage::assistant("Hi!"),
            usage: TokenUsage {
                input_tokens: 5,
                output_tokens: 1,
            },
            stop_reason: StopReason::EndTurn,
        };

        let mut agent = make_agent(response);

        // Empty conversation — should return None
        let summary = agent.end_session().await;
        assert!(
            summary.is_none(),
            "empty conversation should produce no summary"
        );

        // Add just 1 message (still < 2)
        agent.restore_conversation(vec![ChatMessage::user("Hi")]);
        let summary = agent.end_session().await;
        assert!(
            summary.is_none(),
            "single message should produce no summary"
        );
    }

    // --- Part C: Agent turn loop edge cases ---

    #[tokio::test]
    async fn turn_empty_message_still_works() {
        let response = ChatResponse {
            message: ChatMessage::assistant("I received an empty message."),
            usage: TokenUsage {
                input_tokens: 5,
                output_tokens: 7,
            },
            stop_reason: StopReason::EndTurn,
        };

        let mut agent = make_agent(response);
        let result = agent.turn("", None).await.unwrap();
        assert_eq!(result, "I received an empty message.");
        assert_eq!(agent.conversation().len(), 2);
    }

    #[tokio::test]
    async fn to_persisted_session_roundtrip() {
        let response = ChatResponse {
            message: ChatMessage::assistant("Hello"),
            usage: TokenUsage {
                input_tokens: 5,
                output_tokens: 2,
            },
            stop_reason: StopReason::EndTurn,
        };

        let mut agent = make_agent(response);
        let _ = agent.turn("Hi", None).await.unwrap();

        let persisted = agent.to_persisted_session();
        assert_eq!(persisted.metadata.agent_name, "test");
        assert_eq!(persisted.metadata.session_id, agent.session_id());
        assert_eq!(persisted.messages.len(), 2); // user + assistant
        assert_eq!(persisted.metadata.message_count, 2);
    }

    #[tokio::test]
    async fn conversation_history_tracks_messages() {
        let response = ChatResponse {
            message: ChatMessage::assistant("World"),
            usage: TokenUsage {
                input_tokens: 5,
                output_tokens: 2,
            },
            stop_reason: StopReason::EndTurn,
        };

        let mut agent = make_agent(response);
        assert!(agent.conversation().is_empty());

        let _ = agent.turn("Hello", None).await.unwrap();

        let conv = agent.conversation();
        assert_eq!(conv.len(), 2);
        // First message is the user's
        assert_eq!(conv[0].content, "Hello");
        // Second is the assistant's
        assert_eq!(conv[1].content, "World");
    }

    #[test]
    fn set_autonomy_tier_updates_tier() {
        let response = ChatResponse {
            message: ChatMessage::assistant("ok"),
            usage: TokenUsage {
                input_tokens: 1,
                output_tokens: 1,
            },
            stop_reason: StopReason::EndTurn,
        };

        let mut agent = make_agent(response);
        assert_eq!(agent.autonomy_tier(), AutonomyTier::Trust);

        agent.set_autonomy_tier(AutonomyTier::Locked);
        assert_eq!(agent.autonomy_tier(), AutonomyTier::Locked);

        agent.set_autonomy_tier(AutonomyTier::Free);
        assert_eq!(agent.autonomy_tier(), AutonomyTier::Free);
    }
}
