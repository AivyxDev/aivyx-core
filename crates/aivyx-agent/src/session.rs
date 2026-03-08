use aivyx_audit::AuditLog;
use aivyx_capability::{ActionPattern, Capability, CapabilitySet};
use aivyx_config::{AivyxConfig, AivyxDirs, ModelPricing};
use aivyx_core::{AgentId, AivyxError, CapabilityId, Principal, Result, ToolRegistry};
#[cfg(feature = "network-tools")]
use aivyx_crypto::derive_tool_key;
use aivyx_crypto::{EncryptedStore, MasterKey, derive_audit_key, derive_memory_key};
use aivyx_llm::create_provider;
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
}

impl AgentSession {
    /// Create a new session. The master key must already be unlocked.
    pub fn new(dirs: AivyxDirs, config: AivyxConfig, master_key: MasterKey) -> Self {
        Self {
            dirs,
            config,
            master_key,
        }
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

    /// Create an agent from an already-loaded profile.
    pub async fn create_agent_from_profile(&self, profile: &AgentProfile) -> Result<Agent> {
        // Create LLM provider (per-agent override or global default).
        // Scope the EncryptedStore so its redb lock is released before the
        // memory manager opens its own handle on the same database.
        let (provider, provider_config) = {
            let store = EncryptedStore::open(self.dirs.store_path())?;
            let pc = self.config.resolve_provider(profile.provider.as_deref());
            let prov = create_provider(pc, &store, &self.master_key)?;
            (prov, pc.clone())
        };

        // Set up tool registry
        let mut tools = ToolRegistry::new();
        register_built_in_tools(&mut tools, &profile.tool_ids);

        // Build capability set from profile entries.
        let agent_id = AgentId::new();
        let capabilities = Self::build_capabilities(&profile.capabilities, agent_id);

        // Wire memory tools into the registry before Agent::new() consumes it.
        // Memory manager creation must happen early so tools can reference it.
        #[cfg(feature = "memory")]
        let memory_manager = if let Some(ref embedding_config) = self.config.embedding {
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
        {
            self.discover_mcp_tools(&mut tools, &profile.mcp_servers)
                .await;
        }

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

        // Audit log
        let audit_key = derive_audit_key(&self.master_key);
        let audit_log = AuditLog::new(self.dirs.audit_path(), &audit_key);

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
            Some(audit_log),
            self.config.autonomy.max_retries,
            self.config.autonomy.retry_base_delay_ms,
        );

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
        {
            let self_audit_key = derive_audit_key(&self.master_key);
            let self_audit_log = AuditLog::new(self.dirs.audit_path(), &self_audit_key);
            agent.register_tool(Box::new(crate::self_tools::SelfUpdateTool::new(
                self.dirs.clone(),
                profile.name.clone(),
                Some(self_audit_log),
            )));
        }

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
        {
            let plugin_audit_key = derive_audit_key(&self.master_key);
            let plugin_audit_log = AuditLog::new(self.dirs.audit_path(), &plugin_audit_key);
            agent.register_tool(Box::new(crate::plugin_tools::PluginInstallTool::new(
                self.dirs.clone(),
                Some(plugin_audit_log),
            )));
        }
        {
            let remove_audit_key = derive_audit_key(&self.master_key);
            let remove_audit_log = AuditLog::new(self.dirs.audit_path(), &remove_audit_key);
            agent.register_tool(Box::new(crate::plugin_tools::PluginRemoveTool::new(
                self.dirs.clone(),
                Some(remove_audit_log),
            )));
        }

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
    #[cfg(feature = "mcp")]
    async fn discover_mcp_tools(
        &self,
        tools: &mut ToolRegistry,
        mcp_servers: &[aivyx_config::McpServerConfig],
    ) {
        use std::sync::Arc;
        use std::time::Duration;

        if mcp_servers.is_empty() {
            return;
        }

        let cache = Arc::new(aivyx_mcp::ToolResultCache::new(Duration::from_secs(300)));

        // Connect, initialize, and list tools from all servers in parallel.
        let futures: Vec<_> = mcp_servers
            .iter()
            .map(|server_config| {
                let name = server_config.name.clone();
                let timeout = Duration::from_secs(server_config.timeout_secs);
                async move {
                    let client = aivyx_mcp::McpClient::connect(server_config).await?;
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
                    Ok::<_, AivyxError>((name, client, result))
                }
            })
            .collect();

        let results = futures_util::future::join_all(futures).await;

        // Register discovered tools sequentially (ToolRegistry is not Send).
        for result in results {
            match result {
                Ok((name, client, tool_defs)) => {
                    let count = tool_defs.len();
                    for def in tool_defs {
                        let proxy = aivyx_mcp::McpProxyTool::new(
                            def,
                            client.clone(),
                            &name,
                            Some(cache.clone()),
                        );
                        tools.register(Box::new(proxy));
                    }
                    tracing::info!("MCP '{}': registered {} tools", name, count);
                }
                Err(e) => {
                    tracing::warn!("MCP discovery failed: {e}");
                }
            }
        }
    }

    /// Create a MemoryManager from embedding configuration.
    #[cfg(feature = "memory")]
    fn create_memory_manager(
        &self,
        embedding_config: &aivyx_config::EmbeddingConfig,
    ) -> Result<aivyx_memory::MemoryManager> {
        let store = EncryptedStore::open(self.dirs.store_path())?;
        let embedding_provider =
            aivyx_llm::create_embedding_provider(embedding_config, &store, &self.master_key)?;
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
