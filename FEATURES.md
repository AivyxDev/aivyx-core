# Features — Aivyx Core

> Comprehensive inventory of implemented features across all 10 shared crates.
>
> **Version**: 0.1.0 · **Last updated**: 2026-03-08

---

## Multi-Provider LLM Support

Unified abstraction over four LLM providers with consistent API:

| Provider | Chat | Streaming | Vision | Embeddings | STT | TTS |
|----------|------|-----------|--------|------------|-----|-----|
| **OpenAI** | ✅ | ✅ | ✅ | ✅ | ✅ (Whisper) | ✅ |
| **Anthropic Claude** | ✅ | ✅ | ✅ | — | — | — |
| **Google Gemini** | ✅ | ✅ | ✅ | ✅ | — | — |
| **Ollama** (local) | ✅ | ✅ | ✅ | ✅ | ✅ | — |
| **edge-tts** (local) | — | — | — | — | — | ✅ (free) |

- **Factory pattern** — `create_provider()` instantiates the correct provider from configuration
- **Multimodal messages** — `Content` enum supports `Text` and image `Blocks` (base64, URL)
- **Token counting** — per-turn input/output token tracking across all providers
- **Error normalization** — provider-specific errors mapped to `AivyxError`

---

## Encrypted-by-Default Storage

Every piece of persistent data is encrypted at rest — there is no unencrypted storage path.

- **ChaCha20Poly1305** — AEAD authenticated encryption with random nonce per write
- **HKDF-SHA256** — purpose-specific key derivation from a single master key
- **Argon2id** — passphrase-to-key stretching with memory-hard hashing
- **Per-tenant isolation** — `derive_tenant_key(master, tenant_id)` provides cryptographic tenant separation
- **Zeroize on drop** — all key material is zeroed from memory when no longer needed
- **Storage backends** — `redb` (embedded, default), with `StorageBackend` trait for PostgreSQL

---

## Tamper-Proof Audit Logging

HMAC-chained audit trail that detects any retroactive modification:

- **HMAC chain** — each log entry includes the MAC of the previous entry, forming an integrity chain
- **Ed25519 signatures** — optional per-entry cryptographic signatures for non-repudiation
- **30+ event types** — covering agent operations, security events, admin actions, enterprise events
- **Abuse detection** — `AbuseDetector` with sliding-window anomaly detection:
  - Excessive tool call frequency
  - Repeated permission denials
  - Scope escalation attempts
- **Search & filter** — time-range queries, agent/event-type filtering, text search
- **Export** — JSON and CSV export for compliance and analysis
- **Retention policies** — automatic pruning with configurable retention windows
- **Sink architecture** — pluggable output destinations (file, network, webhook)

---

## Capability-Based Permission Model

Fine-grained, attenuated permission system:

- **Capability types** — `Filesystem`, `Network`, `Shell`, `Custom(String)`, and more
- **Scope restrictions** — glob-based patterns for paths, URLs, and commands
  - Example: `Filesystem` scoped to `/home/user/projects/**`
  - Example: `Network` scoped to `*.aivyx.ai`
- **Attenuation** — capabilities can be narrowed when delegated but never widened
  - Parent grants `Filesystem(/home/user/)` → child can receive `Filesystem(/home/user/projects/)`
  - Child cannot escalate to `Filesystem(/)`
- **Capability tokens** — serializable, transferable permission bundles for cross-agent delegation
- **Pattern matching** — `CapabilityPatternMatcher` validates actions against granted scopes

---

## Semantic Memory & Knowledge Graph

Multi-strategy memory system with encrypted storage:

### Memory Types

| Kind | Description | Example |
|------|-------------|---------|
| `Fact` | Factual information | "User works at Acme Corp" |
| `Preference` | User preferences | "Prefers dark mode" |
| `Decision` | Decisions with rationale | "Chose PostgreSQL for persistence" |
| `Outcome` | Results of actions | "Deployment succeeded in 3 min" |

### Knowledge Graph (GraphRAG)

- **Semantic triples** — `(Subject, Predicate, Object)` with confidence scores and source tags
- **BFS traversal** — multi-hop graph exploration from any entity
- **Path finding** — shortest path between entities
- **Community detection** — connected component analysis for topic clustering
- **Entity search** — case-insensitive substring matching
- **Real-time updates** — graph updated on every triple insertion

### Memory Consolidation

- **Similarity clustering** — greedy cosine similarity clustering (threshold 0.85)
- **LLM-driven merge** — semantically similar memories merged via LLM
- **Decay pruning** — stale, unaccessed memories removed after 90 days
- **Strength reinforcement** — frequently accessed memories tagged `"high-confidence"`

### Retrieval Router

| Strategy | Method | Best For |
|----------|--------|----------|
| `Vector` | Cosine similarity over embeddings | Semantic similarity queries |
| `Keyword` | Token overlap scoring | Exact term matching |
| `Graph` | Knowledge graph traversal | Relationship queries |
| `MultiSource` | Combined results from all strategies | Complex queries |

- **Automatic strategy selection** — heuristic-based routing based on query patterns
- **Relevance filtering** — threshold filter removes low-relevance results

### Multimodal Memory

- **Attachments** — binary storage for images with `media_type` and `description`
- **Description-based embedding** — vision LLM generates text descriptions for image search
- **Linked triples** — knowledge triples can reference binary attachments

### Outcome Tracking & Self-Improvement

- **Outcome records** — per-step and per-tool success/failure with duration
- **Feedback analysis** — computes per-tool and per-role success rates
- **Pattern detection** — identifies tool combinations with >80% / <30% success rates
- **Planner integration** — feedback injected as `[PLANNER FEEDBACK]` blocks

---

## MCP Tool Protocol

Full Model Context Protocol client implementation:

- **Transports** — stdio (local processes) and SSE (HTTP-based remote servers)
- **OAuth 2.1** — PKCE authorization flow for authenticated remote MCP servers
  - Metadata discovery, token exchange, automatic refresh
  - Authorization headers injected on all SSE requests
- **Sampling** — bidirectional: MCP servers can request LLM completions
  - `SamplingHandler` trait for custom handling
  - `JsonRpcMessage` enum distinguishes responses from server requests
- **Elicitation** — handles MCP server requests for user input
  - `ElicitationHandler` trait with `AutoDismissElicitationHandler` for headless mode
- **Tool caching** — TTL-based caching of tool/resource results
- **MCP proxy** — protocol bridging between MCP servers
- **Plugin hot-reload** — config changes applied without restart

---

## Agent Sessions

Complete agent lifecycle management:

- **Session lifecycle** — create → turn → (tools) → context compression → summary → end
- **Context compression** — automatic conversation summarization when context grows too large
- **Agent profiles** — configurable persona, system prompt, model selection, and capabilities
- **Skill system** — TOML-based skill manifests with hot-loading from `~/.aivyx/skills/`
- **Cost tracking** — per-turn token usage and dollar cost calculation

### Built-In Tools

| Category | Tools |
|----------|-------|
| **Filesystem** | Read, write, list, search files |
| **Network** | HTTP requests, web scraping, URL fetch |
| **Shell** | Command execution (capability-gated) |
| **Analysis** | Data analysis, chart generation |
| **Documents** | PDF, XLSX, CSV extraction |
| **Federation** | Cross-instance agent communication |
| **Self** | Memory management, profile updates |
| **Infrastructure** | System information, diagnostics |

### Prompt Injection Defense

Three-layer defense against prompt injection:

1. **Input sanitization** — escapes ChatML, Llama, and Mistral delimiters
2. **Tool output boundaries** — wraps tool outputs with boundary markers + system instructions
3. **Webhook sanitization** — sanitizes external webhook payloads before processing

---

## Federation Protocol

Multi-instance agent collaboration:

- **Ed25519 authentication** — cryptographic signing and verification of all inter-instance messages
- **Trust policies** — per-peer `TrustPolicy` with `allowed_scopes` and `max_tier`
- **Relay** — proxy chat and task requests between federated instances
- **Health monitoring** — peer liveness tracking with `last_seen` timestamps
- **Multi-region failover** — capability-aware peer selection with automatic retry across healthy peers
- **Federated teams** — agents from different instances collaborating in shared sessions

---

## Configuration

TOML-based configuration with sensible defaults:

```toml
[provider]
name = "openai"          # or "claude", "gemini", "ollama"
model = "gpt-4o"
api_key_env = "OPENAI_API_KEY"

[memory]
embedding_model = "text-embedding-3-small"
embedding_dimensions = 1536
consolidation_threshold = 0.85

[mcp.servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/home/user"]
```

- **Directory management** — `AivyxDirs` auto-creates `~/.aivyx/` structure
- **Channel config** — Slack, Discord, email, webhook integration settings
- **Project config** — per-project overrides via `.aivyx.toml`
- **Embedding config** — model, dimensions, and provider settings
