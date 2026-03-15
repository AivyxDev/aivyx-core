use aivyx_audit::{AuditEvent, AuditLog};
use aivyx_capability::{ActionPattern, Capability, CapabilitySet};
use aivyx_config::{AivyxConfig, AivyxDirs, ModelPricing};
use aivyx_core::{AgentId, AivyxError, CapabilityId, Principal, Result, ToolRegistry};
#[cfg(feature = "network-tools")]
use aivyx_crypto::derive_tool_key;
use aivyx_crypto::{EncryptedStore, MasterKey, derive_audit_key, derive_memory_key};
use aivyx_llm::cache::{CacheEvent, CacheObserver, CachingProvider};
use aivyx_llm::circuit_breaker::CircuitBreakerConfig;
use aivyx_llm::create_provider;
use aivyx_llm::resilient::{FailoverObserver, ProviderEvent, ResilientProvider};
use chrono::Utc;

use crate::agent::Agent;
use crate::built_in_tools::register_built_in_tools;
use crate::cost_tracker::CostTracker;
use crate::profile::AgentProfile;
use crate::rate_limiter::RateLimiter;

/// Creates and configures [`Agent`] instances from profiles and system config.
pub struct AgentSession {
    dirs: AivyxDirs,
    config: AivyxConfig,
    master_key: MasterKey,
    /// Shared Nexus store for agent social tools (None if Nexus is not initialized).
    #[cfg(feature = "nexus")]
    nexus_store: Option<std::sync::Arc<aivyx_nexus::store::NexusStore>>,
    /// Auto-relay client for forwarding posts/profiles to the Nexus hub.
    #[cfg(feature = "nexus")]
    nexus_relay: Option<std::sync::Arc<aivyx_nexus::NexusRelay>>,
    /// Federation authentication for Ed25519 signing of Nexus content.
    #[cfg(feature = "federation")]
    federation_auth: Option<std::sync::Arc<aivyx_federation::auth::FederationAuth>>,
}

impl AgentSession {
    /// Create a new session. The master key must already be unlocked.
    pub fn new(dirs: AivyxDirs, config: AivyxConfig, master_key: MasterKey) -> Self {
        Self {
            dirs,
            config,
            master_key,
            #[cfg(feature = "nexus")]
            nexus_store: None,
            #[cfg(feature = "nexus")]
            nexus_relay: None,
            #[cfg(feature = "federation")]
            federation_auth: None,
        }
    }

    /// Set the Nexus store for agent social tools.
    ///
    /// When set, all agents created by this session will have Nexus tools
    /// (publish, browse, interact, etc.) registered in their tool registry.
    #[cfg(feature = "nexus")]
    pub fn set_nexus_store(&mut self, store: std::sync::Arc<aivyx_nexus::store::NexusStore>) {
        self.nexus_store = Some(store);
        // Create the auto-relay client — zero config, just works.
        self.nexus_relay = Some(std::sync::Arc::new(aivyx_nexus::NexusRelay::new()));
    }

    /// Set the federation auth for Ed25519 signing of Nexus content.
    #[cfg(feature = "federation")]
    pub fn set_federation_auth(
        &mut self,
        auth: std::sync::Arc<aivyx_federation::auth::FederationAuth>,
    ) {
        self.federation_auth = Some(auth);
    }

    /// Access the file system directories.
    pub fn dirs(&self) -> &AivyxDirs {
        &self.dirs
    }

    /// Access the full application configuration.
    pub fn config(&self) -> &AivyxConfig {
        &self.config
    }

    /// Access the provider configuration.
    pub fn provider_config(&self) -> &aivyx_config::ProviderConfig {
        &self.config.provider
    }

    /// Access the master key (for key derivation).
    pub fn master_key(&self) -> &MasterKey {
        &self.master_key
    }

    /// Load an agent profile by name and create a configured Agent.
    pub async fn create_agent(&self, profile_name: &str) -> Result<Agent> {
        let profile_path = self.dirs.agents_dir().join(format!("{profile_name}.toml"));
        if !profile_path.exists() {
            return Err(AivyxError::Config(format!(
                "agent profile not found: {profile_name} (expected at {})",
                profile_path.display()
            )));
        }

        let profile = AgentProfile::load(&profile_path)?;
        self.create_agent_from_profile(&profile).await
    }

    /// Load a profile by name and create an agent with a shared MemoryManager.
    ///
    /// Use this from the server to avoid per-agent `EncryptedStore` lock
    /// contention when the server already holds a shared MemoryManager.
    #[cfg(feature = "memory")]
    pub async fn create_agent_with_shared_memory(
        &self,
        profile_name: &str,
        shared_memory: std::sync::Arc<tokio::sync::Mutex<aivyx_memory::MemoryManager>>,
    ) -> Result<Agent> {
        let profile_path = self.dirs.agents_dir().join(format!("{profile_name}.toml"));
        if !profile_path.exists() {
            return Err(AivyxError::Config(format!(
                "agent profile not found: {profile_name} (expected at {})",
                profile_path.display()
            )));
        }

        let profile = AgentProfile::load(&profile_path)?;
        self.create_agent_for_server(&profile, shared_memory).await
    }

    /// Create an agent from an already-loaded profile.
    pub async fn create_agent_from_profile(&self, profile: &AgentProfile) -> Result<Agent> {
        self.create_agent_inner(profile, None).await
    }

    /// Create an agent for server use with a shared MemoryManager.
    ///
    /// When the server already holds a shared `MemoryManager`, pass it here
    /// to avoid opening a redundant `EncryptedStore` (which causes "Database
    /// already open" lock contention). Memory tools are registered using the
    /// shared manager instead.
    #[cfg(feature = "memory")]
    pub async fn create_agent_for_server(
        &self,
        profile: &AgentProfile,
        shared_memory: std::sync::Arc<tokio::sync::Mutex<aivyx_memory::MemoryManager>>,
    ) -> Result<Agent> {
        self.create_agent_inner(profile, Some(shared_memory)).await
    }

    /// Internal agent creation with optional shared memory manager.
    async fn create_agent_inner(
        &self,
        profile: &AgentProfile,
        #[cfg(feature = "memory")] shared_memory: Option<
            std::sync::Arc<tokio::sync::Mutex<aivyx_memory::MemoryManager>>,
        >,
    ) -> Result<Agent> {
        let agent_id = AgentId::new();

        // Single shared audit log for all observers in this agent's lifecycle.
        // Each observer captures an Arc clone — avoids redundant key derivation
        // and file handle creation (was 6-8 separate instances before).
        let audit_key = derive_audit_key(&self.master_key);
        let shared_audit = std::sync::Arc::new(AuditLog::new(self.dirs.audit_path(), &audit_key));

        // Create LLM provider (per-agent override or global default).
        // Scope the EncryptedStore so its redb lock is released before the
        // memory manager opens its own handle on the same database.
        // The store stays open through provider + routing + cache creation,
        // then drops before memory manager setup.
        let (provider, provider_config): (Box<dyn aivyx_llm::provider::LlmProvider>, _) = {
            let store = EncryptedStore::open(self.dirs.store_path())?;
            let pc = self.config.resolve_provider(profile.provider.as_deref());
            let prov = create_provider(pc, &store, &self.master_key)?;
            let provider_config = pc.clone();

            let provider: Box<dyn aivyx_llm::provider::LlmProvider> =
                if profile.fallback_providers.is_empty() {
                    prov
                } else {
                    // Build resilient provider with circuit breaker and fallbacks.
                    let cb = &self.config.autonomy.circuit_breaker;
                    let cb_config = CircuitBreakerConfig {
                        failure_threshold: cb.failure_threshold,
                        recovery_timeout: std::time::Duration::from_secs(cb.recovery_timeout_secs),
                        success_threshold: cb.success_threshold,
                    };

                    let primary_name = profile.provider.as_deref().unwrap_or("default").to_string();

                    let mut resilient =
                        ResilientProvider::new(prov, primary_name, cb_config.clone());

                    for fb_name in &profile.fallback_providers {
                        let fb_pc = self.config.resolve_provider(Some(fb_name));
                        match create_provider(fb_pc, &store, &self.master_key) {
                            Ok(fb_prov) => {
                                resilient = resilient.with_fallback(
                                    fb_prov,
                                    fb_name.clone(),
                                    cb_config.clone(),
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    fallback = %fb_name,
                                    error = %e,
                                    "Failed to create fallback provider, skipping"
                                );
                            }
                        }
                    }

                    // Wire failover observer to bridge provider events → audit log.
                    let fo_audit_log = std::sync::Arc::clone(&shared_audit);
                    let fo_agent_id = agent_id;
                    let observer: FailoverObserver =
                        std::sync::Arc::new(move |event: ProviderEvent| {
                            let audit_event = match event {
                                ProviderEvent::CircuitOpened { provider, failures } => {
                                    AuditEvent::ProviderCircuitOpened {
                                        provider,
                                        consecutive_failures: failures,
                                        agent_id: fo_agent_id,
                                        timestamp: Utc::now(),
                                    }
                                }
                                ProviderEvent::FailoverActivated { from, to } => {
                                    AuditEvent::ProviderFailover {
                                        from_provider: from,
                                        to_provider: to,
                                        agent_id: fo_agent_id,
                                        timestamp: Utc::now(),
                                    }
                                }
                                ProviderEvent::CircuitClosed { provider } => {
                                    AuditEvent::ProviderCircuitClosed {
                                        provider,
                                        agent_id: fo_agent_id,
                                        timestamp: Utc::now(),
                                    }
                                }
                                ProviderEvent::AllProvidersDown => {
                                    tracing::error!("All LLM providers are down");
                                    return;
                                }
                            };
                            if let Err(e) = fo_audit_log.append(audit_event) {
                                tracing::warn!("Failed to audit provider failover event: {e}");
                            }
                        });

                    resilient = resilient.with_observer(observer);

                    Box::new(resilient)
                };

            // --- Routing layer (reuses the same store) ---
            let routing_config = profile.routing.as_ref().or(self.config.routing.as_ref());
            let provider: Box<dyn aivyx_llm::provider::LlmProvider> =
                if let Some(rc) = routing_config {
                    let mut tier_providers = std::collections::HashMap::new();
                    for (level, name) in [
                        (aivyx_llm::ComplexityLevel::Simple, &rc.simple),
                        (aivyx_llm::ComplexityLevel::Medium, &rc.medium),
                        (aivyx_llm::ComplexityLevel::Complex, &rc.complex),
                    ] {
                        if let Some(provider_name) = name {
                            let pc = self.config.resolve_provider(Some(provider_name));
                            match create_provider(pc, &store, &self.master_key) {
                                Ok(p) => {
                                    tier_providers.insert(level, p);
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        level = ?level,
                                        provider = %provider_name,
                                        error = %e,
                                        "Failed to create routing provider, using default"
                                    );
                                }
                            }
                        }
                    }

                    if tier_providers.is_empty() {
                        provider
                    } else {
                        let rt_audit_log = std::sync::Arc::clone(&shared_audit);
                        let rt_agent_id = agent_id;
                        let observer: aivyx_llm::RoutingObserver =
                            std::sync::Arc::new(move |event: aivyx_llm::RoutingEvent| {
                                let aivyx_llm::RoutingEvent::Routed {
                                    complexity,
                                    provider,
                                } = event;
                                if let Err(e) = rt_audit_log.append(AuditEvent::ModelRouted {
                                    agent_id: rt_agent_id,
                                    complexity: format!("{complexity}"),
                                    provider,
                                    timestamp: Utc::now(),
                                }) {
                                    tracing::warn!("Failed to audit routing event: {e}");
                                }
                            });
                        let routing = aivyx_llm::RoutingProvider::new(provider, tier_providers)
                            .with_observer(observer);
                        Box::new(routing)
                    }
                } else {
                    provider
                };

            // --- Caching layer (reuses the same store for embedding provider) ---
            let cache_config = profile.cache.as_ref().or(self.config.cache.as_ref());
            let provider: Box<dyn aivyx_llm::provider::LlmProvider> = if let Some(cc) = cache_config
            {
                if cc.enabled {
                    let mut caching = CachingProvider::new(provider, cc);

                    if cc.semantic_enabled
                        && let Some(ref emb_config) = self.config.embedding
                    {
                        match aivyx_llm::create_embedding_provider(
                            emb_config,
                            &store,
                            &self.master_key,
                        ) {
                            Ok(emb) => {
                                caching = caching.with_semantic(std::sync::Arc::from(emb));
                            }
                            Err(e) => {
                                tracing::warn!("Semantic cache disabled: {e}");
                            }
                        }
                    }

                    let cache_audit_log = std::sync::Arc::clone(&shared_audit);
                    let cache_agent_id = agent_id;
                    let cache_observer: CacheObserver =
                        std::sync::Arc::new(move |event: CacheEvent| {
                            let audit_event = match event {
                                CacheEvent::PromptCacheHit {
                                    prompt_hash,
                                    tokens_saved,
                                } => AuditEvent::PromptCacheHit {
                                    prompt_hash,
                                    tokens_saved,
                                    agent_id: cache_agent_id,
                                },
                                CacheEvent::SemanticCacheHit {
                                    similarity,
                                    tokens_saved,
                                } => AuditEvent::SemanticCacheHit {
                                    similarity,
                                    tokens_saved,
                                    agent_id: cache_agent_id,
                                },
                            };
                            if let Err(e) = cache_audit_log.append(audit_event) {
                                tracing::warn!("Failed to audit cache event: {e}");
                            }
                        });

                    caching = caching.with_observer(cache_observer);
                    Box::new(caching)
                } else {
                    provider
                }
            } else {
                provider
            };

            (provider, provider_config)
        }; // store lock released here — before memory manager opens its handle

        // Set up tool registry
        let mut tools = ToolRegistry::new();
        register_built_in_tools(&mut tools, &profile.tool_ids);

        // Build capability set from profile entries.
        let capabilities = Self::build_capabilities(&profile.capabilities, agent_id);

        // Wire memory tools into the registry before Agent::new() consumes it.
        // If a shared memory manager was provided (server mode), use it instead
        // of creating a per-agent one (which would lock the store).
        #[cfg(feature = "memory")]
        let memory_manager = if let Some(shared_mgr) = shared_memory {
            aivyx_memory::register_memory_tools(&mut tools, shared_mgr.clone(), agent_id);
            tracing::info!(
                "Memory enabled for agent '{}' (shared server manager)",
                profile.name,
            );
            Some(shared_mgr)
        } else if let Some(ref embedding_config) = self.config.embedding {
            match self.create_memory_manager(embedding_config) {
                Ok(mgr) => {
                    let arc_mgr = std::sync::Arc::new(tokio::sync::Mutex::new(mgr));
                    aivyx_memory::register_memory_tools(&mut tools, arc_mgr.clone(), agent_id);
                    tracing::info!(
                        "Memory enabled for agent '{}' (embedding: {:?})",
                        profile.name,
                        embedding_config
                    );
                    Some(arc_mgr)
                }
                Err(e) => {
                    tracing::warn!(
                        "Memory unavailable for agent '{}': {e} (continuing without memory)",
                        profile.name,
                    );
                    None
                }
            }
        } else {
            None
        };

        // Discover and register MCP tools from configured servers.
        #[cfg(feature = "mcp")]
        let mcp_pool = {
            // Build an observer that bridges MCP tool call events to the audit log.
            let mcp_audit_log = std::sync::Arc::clone(&shared_audit);
            let obs_agent_id = agent_id;
            let observer: aivyx_mcp::McpToolCallObserver = std::sync::Arc::new(move |event| {
                let audit_event = match event {
                    aivyx_mcp::McpToolCallEvent::Started {
                        server_name,
                        tool_name,
                    } => AuditEvent::McpToolCallStarted {
                        server_name,
                        tool_name,
                        agent_id: obs_agent_id,
                        timestamp: chrono::Utc::now(),
                    },
                    aivyx_mcp::McpToolCallEvent::Completed {
                        server_name,
                        tool_name,
                        duration_ms,
                    } => AuditEvent::McpToolCallCompleted {
                        server_name,
                        tool_name,
                        agent_id: obs_agent_id,
                        duration_ms,
                        timestamp: chrono::Utc::now(),
                    },
                    aivyx_mcp::McpToolCallEvent::Failed {
                        server_name,
                        tool_name,
                        error,
                    } => AuditEvent::McpToolCallFailed {
                        server_name,
                        tool_name,
                        agent_id: obs_agent_id,
                        error,
                        timestamp: chrono::Utc::now(),
                    },
                    aivyx_mcp::McpToolCallEvent::TaskPolled {
                        server_name,
                        task_id,
                        state,
                    } => AuditEvent::McpTaskCompleted {
                        server_name,
                        task_id,
                        state,
                        duration_ms: 0,
                        timestamp: chrono::Utc::now(),
                    },
                    aivyx_mcp::McpToolCallEvent::SamplingDispatched { server_name } => {
                        AuditEvent::McpSamplingRequested {
                            server_name,
                            max_tokens: None,
                            timestamp: chrono::Utc::now(),
                        }
                    }
                    aivyx_mcp::McpToolCallEvent::ElicitationDispatched {
                        server_name,
                        action,
                    } => AuditEvent::McpElicitationRequested {
                        server_name,
                        action_taken: action,
                        timestamp: chrono::Utc::now(),
                    },
                };
                if let Err(e) = mcp_audit_log.append(audit_event) {
                    tracing::warn!("Failed to audit MCP tool call: {e}");
                }
            });

            self.discover_mcp_tools(&mut tools, &profile.mcp_servers, Some(observer))
                .await
        };

        // Autonomy tier: profile override > config default
        let autonomy_tier = profile
            .autonomy_tier
            .unwrap_or(self.config.autonomy.default_tier);

        // Rate limiter from config
        let rate_limiter = RateLimiter::new(self.config.autonomy.max_tool_calls_per_minute);

        // Cost tracker with model-aware pricing
        let pricing = ModelPricing::default_for_model(provider_config.model_name());
        let cost_tracker = CostTracker::new(
            self.config.autonomy.max_cost_per_session_usd,
            pricing.input_cost_per_token,
            pricing.output_cost_per_token,
        );

        let mut agent = Agent::new(
            agent_id,
            profile.name.clone(),
            profile.effective_soul(),
            profile.max_tokens,
            autonomy_tier,
            provider,
            tools,
            capabilities,
            rate_limiter,
            cost_tracker,
            Some(AuditLog::new(self.dirs.audit_path(), &audit_key)),
            self.config.autonomy.max_retries,
            self.config.autonomy.retry_base_delay_ms,
        );

        // Store MCP pool on agent for lifecycle management.
        #[cfg(feature = "mcp")]
        if let Some(pool) = mcp_pool {
            agent.set_mcp_pool(pool);
        }

        // Set the memory manager on the agent for runtime memory retrieval
        #[cfg(feature = "memory")]
        if let Some(mgr) = memory_manager {
            agent.set_memory_manager(mgr);
        }

        // Self-improvement tools — contextual registration (need dirs + agent name)
        agent.register_tool(Box::new(crate::self_tools::SelfProfileTool::new(
            self.dirs.clone(),
            profile.name.clone(),
        )));
        agent.register_tool(Box::new(crate::self_tools::SelfUpdateTool::new(
            self.dirs.clone(),
            profile.name.clone(),
            Some(AuditLog::new(self.dirs.audit_path(), &audit_key)),
        )));
        agent.register_tool(Box::new(crate::self_tools::SkillCreateTool::new(
            self.dirs.clone(),
            profile.name.clone(),
            Some(AuditLog::new(self.dirs.audit_path(), &audit_key)),
        )));

        // Discover SKILL.md skills from user-global directory.
        {
            let skill_dirs = vec![self.dirs.skills_dir()];
            match crate::skill_loader::SkillLoader::discover(&skill_dirs) {
                Ok(loader) if loader.has_skills() => {
                    tracing::info!(
                        "Skills: discovered {} skills for agent '{}'",
                        loader.skill_names().len(),
                        profile.name
                    );
                    let arc_loader = std::sync::Arc::new(tokio::sync::Mutex::new(loader));
                    agent.register_tool(Box::new(crate::built_in_tools::SkillActivateTool::new(
                        arc_loader.clone(),
                    )));
                    agent.set_skill_loader(arc_loader);
                }
                Ok(_) => {} // No skills found
                Err(e) => {
                    tracing::warn!(
                        "Skill discovery failed for agent '{}': {e} (continuing without skills)",
                        profile.name
                    );
                }
            }
        }

        // Plugin management tools — contextual registration (need dirs + audit)
        agent.register_tool(Box::new(crate::plugin_tools::PluginListTool::new(
            self.dirs.clone(),
        )));
        agent.register_tool(Box::new(crate::plugin_tools::PluginInstallTool::new(
            self.dirs.clone(),
            Some(AuditLog::new(self.dirs.audit_path(), &audit_key)),
        )));
        agent.register_tool(Box::new(crate::plugin_tools::PluginRemoveTool::new(
            self.dirs.clone(),
            Some(AuditLog::new(self.dirs.audit_path(), &audit_key)),
        )));

        // Phase 11D: Contextual infrastructure tools (need AivyxDirs)
        #[cfg(feature = "infrastructure-tools")]
        agent.register_tool(Box::new(
            crate::infrastructure_tools::ScheduleTaskTool::new(self.dirs.clone()),
        ));

        // Phase 11C: Contextual network/communication tools (need encrypted store access)
        #[cfg(feature = "network-tools")]
        {
            let tool_key = derive_tool_key(&self.master_key);
            let provider_config = self.config.resolve_provider(None).clone();
            agent.register_tool(Box::new(crate::network_tools::TranslateTool::new(
                self.dirs.clone(),
                provider_config,
                tool_key,
            )));
        }
        #[cfg(feature = "network-tools")]
        {
            let tool_key = derive_tool_key(&self.master_key);
            agent.register_tool(Box::new(crate::network_tools::NotificationSendTool::new(
                self.dirs.clone(),
                self.config.clone(),
                tool_key,
            )));
        }
        #[cfg(feature = "network-tools")]
        {
            let tool_key = derive_tool_key(&self.master_key);
            agent.register_tool(Box::new(crate::network_tools::EmailSendTool::new(
                self.dirs.clone(),
                self.config.smtp.clone(),
                tool_key,
            )));
        }

        // Nexus social tools — register if store is available and profile allows it.
        #[cfg(feature = "nexus")]
        if profile.nexus_enabled
            && let Some(ref nexus_store) = self.nexus_store
        {
            let instance_id = self
                .config
                .federation
                .as_ref()
                .map(|f| f.instance_id.clone())
                .unwrap_or_else(|| "local".to_string());

            let nexus_ctx = std::sync::Arc::new(crate::nexus_tools::NexusContext {
                store: std::sync::Arc::clone(nexus_store),
                redaction: std::sync::Arc::new(aivyx_nexus::redact::RedactionFilter::new()),
                agent_id: aivyx_nexus::types::AgentProfile::canonical_id(
                    &profile.name,
                    &instance_id,
                ),
                instance_id,
                relay: self.nexus_relay.clone(),
                #[cfg(feature = "federation")]
                federation_auth: self.federation_auth.clone(),
            });

            crate::nexus_tools::register_nexus_tools(agent.tool_registry_mut(), nexus_ctx);

            // Pass the data policy from config so the [NEXUS CONTEXT] block
            // includes appropriate data-sharing guardrails.
            let data_policy = self
                .config
                .nexus
                .as_ref()
                .map(|n| n.data_policy)
                .unwrap_or_default();
            agent.set_nexus_enabled(data_policy);
            tracing::info!(
                "Nexus enabled for agent '{}' (7 social tools + community context, policy={:?})",
                profile.name,
                data_policy,
            );
        }

        Ok(agent)
    }

    /// Load an agent and auto-detect the active project from the current
    /// working directory.
    ///
    /// If `cwd` is inside a registered project's path, the agent's context is
    /// automatically scoped to that project (project prompt injection +
    /// project-scoped memory recall).
    pub async fn create_agent_with_context(
        &self,
        profile_name: &str,
        cwd: Option<&std::path::Path>,
    ) -> Result<Agent> {
        let mut agent = self.create_agent(profile_name).await?;
        if let Some(dir) = cwd {
            if let Some(project) = self.config.find_project_by_path(dir) {
                agent.set_active_project(project.clone());
            }
            // Scan project-local skills (override user-global with same name)
            let project_skills_dir = dir.join(".aivyx").join("skills");
            if project_skills_dir.exists() {
                let skill_dirs = vec![project_skills_dir, self.dirs.skills_dir()];
                match crate::skill_loader::SkillLoader::discover(&skill_dirs) {
                    Ok(loader) if loader.has_skills() => {
                        let arc_loader = std::sync::Arc::new(tokio::sync::Mutex::new(loader));
                        agent.register_tool(Box::new(
                            crate::built_in_tools::SkillActivateTool::new(arc_loader.clone()),
                        ));
                        agent.set_skill_loader(arc_loader);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("Project skill discovery failed: {e}");
                    }
                }
            }
        }
        Ok(agent)
    }

    /// Discover tools from configured MCP servers and register them.
    ///
    /// Servers are connected and initialized **in parallel** for faster startup.
    /// Each server's tools are wrapped as [`McpProxyTool`] instances in the
    /// agent's tool registry. Connection failures are logged but do not prevent
    /// agent creation.
    ///
    /// Returns an [`McpServerPool`](aivyx_mcp::McpServerPool) tracking all
    /// active connections for lifecycle management and graceful shutdown.
    #[cfg(feature = "mcp")]
    async fn discover_mcp_tools(
        &self,
        tools: &mut ToolRegistry,
        mcp_servers: &[aivyx_config::McpServerConfig],
        observer: Option<aivyx_mcp::McpToolCallObserver>,
    ) -> Option<std::sync::Arc<aivyx_mcp::McpServerPool>> {
        use std::sync::Arc;
        use std::time::Duration;

        if mcp_servers.is_empty() {
            return None;
        }

        // Validate all configs before connecting.
        for config in mcp_servers {
            if let Err(e) = config.validate() {
                tracing::error!("MCP config validation failed: {e}");
                return None;
            }
        }

        let cache = Arc::new(aivyx_mcp::ToolResultCache::new(Duration::from_secs(300)));
        let pool = Arc::new(aivyx_mcp::McpServerPool::new());

        // Connect, initialize, and list tools from all servers in parallel.
        let futures: Vec<_> = mcp_servers
            .iter()
            .map(|server_config| {
                let name = server_config.name.clone();
                let timeout = Duration::from_secs(server_config.timeout_secs);
                let config_ref = server_config.clone();
                async move {
                    let elicitation: Option<std::sync::Arc<dyn aivyx_mcp::ElicitationHandler>> =
                        Some(std::sync::Arc::new(
                            aivyx_mcp::AutoDismissElicitationHandler,
                        ));
                    let client = aivyx_mcp::McpClient::connect_with_handlers(
                        &config_ref,
                        None, // StorageBackend — not yet available in session
                        None, // SamplingHandler — requires LLM provider wiring
                        elicitation,
                    )
                    .await?;
                    let client = Arc::new(client);
                    // Apply per-server timeout to the init+list sequence.
                    let result = tokio::time::timeout(timeout, async {
                        client.initialize().await?;
                        client.list_tools().await
                    })
                    .await
                    .map_err(|_| {
                        AivyxError::Other(format!(
                            "MCP server '{name}' timed out after {timeout:?}"
                        ))
                    })??;
                    Ok::<_, AivyxError>((name, client, result, config_ref))
                }
            })
            .collect();

        let results = futures_util::future::join_all(futures).await;

        // Register discovered tools sequentially (ToolRegistry is not Send).
        for result in results {
            match result {
                Ok((name, client, tool_defs, server_config)) => {
                    // Track client in pool for lifecycle management.
                    pool.insert(name.clone(), client.clone(), server_config.clone())
                        .await;

                    // Filter tools by allow/block configuration.
                    let filtered: Vec<_> = tool_defs
                        .into_iter()
                        .filter(|def| server_config.is_tool_allowed(&def.name))
                        .collect();
                    let total = filtered.len();
                    for mut def in filtered {
                        // Detect name collisions and prefix with server name.
                        if tools.has_name(&def.name) {
                            let prefixed = format!("{}:{}", name, def.name);
                            tracing::warn!(
                                "MCP tool '{}' conflicts with existing tool, registering as '{}'",
                                def.name,
                                prefixed
                            );
                            def.name = prefixed;
                        }
                        let proxy = if let Some(obs) = &observer {
                            aivyx_mcp::McpProxyTool::with_observer(
                                def,
                                pool.clone(),
                                &name,
                                Some(cache.clone()),
                                obs.clone(),
                            )
                        } else {
                            aivyx_mcp::McpProxyTool::new(
                                def,
                                pool.clone(),
                                &name,
                                Some(cache.clone()),
                            )
                        };
                        tools.register(Box::new(proxy));
                    }
                    tracing::info!("MCP '{}': registered {} tools", name, total);
                }
                Err(e) => {
                    tracing::warn!("MCP discovery failed: {e}");
                }
            }
        }

        Some(pool)
    }

    /// Create a MemoryManager from embedding configuration.
    ///
    /// The `EncryptedStore` handle is scoped so its redb lock is released
    /// before the memory-specific SQLite DB opens. This prevents "Database
    /// already open" errors when the server's global memory manager also
    /// holds a handle on `store.db`.
    #[cfg(feature = "memory")]
    fn create_memory_manager(
        &self,
        embedding_config: &aivyx_config::EmbeddingConfig,
    ) -> Result<aivyx_memory::MemoryManager> {
        // Scope the EncryptedStore so its redb lock is released before we
        // open the memory-specific database.
        let embedding_provider = {
            let store = EncryptedStore::open(self.dirs.store_path())?;
            aivyx_llm::create_embedding_provider(embedding_config, &store, &self.master_key)?
        }; // store lock released here
        let memory_db_path = self.dirs.memory_dir().join("memory.db");
        let memory_store = aivyx_memory::MemoryStore::open(&memory_db_path)?;
        let memory_key = derive_memory_key(&self.master_key);
        aivyx_memory::MemoryManager::new(
            memory_store,
            std::sync::Arc::from(embedding_provider),
            memory_key,
            self.config.memory.max_memories,
        )
    }

    /// Create an LLM provider from the session's configuration.
    ///
    /// Used by team-level tools (like task decomposition) that need to make
    /// LLM calls outside of an agent's turn loop. Uses the global default
    /// provider configuration.
    pub fn create_llm_provider(&self) -> Result<Box<dyn aivyx_llm::LlmProvider>> {
        let store = EncryptedStore::open(self.dirs.store_path())?;
        let provider_config = self.config.resolve_provider(None);
        create_provider(provider_config, &store, &self.master_key)
    }

    /// Create an `AuditLog` for team-level audit events.
    pub fn create_audit_log(&self) -> AuditLog {
        let audit_key = derive_audit_key(&self.master_key);
        AuditLog::new(self.dirs.audit_path(), &audit_key)
    }

    /// Convert profile capability entries into a `CapabilitySet` granted to the agent.
    fn build_capabilities(
        profile_caps: &[crate::profile::ProfileCapability],
        agent_id: AgentId,
    ) -> CapabilitySet {
        let mut set = CapabilitySet::new();
        let principal = Principal::Agent(agent_id);

        for pc in profile_caps {
            if let Some(pattern) = ActionPattern::new(&pc.pattern) {
                let cap = Capability {
                    id: CapabilityId::new(),
                    scope: pc.scope.clone(),
                    pattern,
                    granted_to: vec![principal.clone()],
                    granted_by: Principal::System,
                    created_at: Utc::now(),
                    expires_at: None,
                    revoked: false,
                    parent_id: None,
                };
                set.grant(cap);
            }
        }

        set
    }
}
