# Architecture — Aivyx Core

> Technical architecture guide for the shared foundation crates.
>
> **Last updated**: 2026-03-08

---

## System Context

Aivyx Core sits at the center of the Aivyx ecosystem, providing the shared foundation that both the public client and the private engine build upon:

```
┌───────────────────────────────────────────────────────────────┐
│                     Aivyx Ecosystem                          │
│                                                              │
│  ┌─────────────┐    ┌──────────────┐    ┌─────────────────┐  │
│  │ Aivyx CLI   │    │ Aivyx Desktop│    │ Aivyx Engine    │  │
│  │ (Public)    │    │ (Tauri)      │    │ (Private Server)│  │
│  └──────┬──────┘    └──────┬───────┘    └───────┬─────────┘  │
│         │                  │                    │            │
│         └──────────────────┼────────────────────┘            │
│                            │                                 │
│                   ┌────────▼────────┐                        │
│                   │   aivyx-core    │                        │
│                   │  (10 crates)    │                        │
│                   └─────────────────┘                        │
└───────────────────────────────────────────────────────────────┘
```

---

## Crate Dependency Graph

Dependencies flow downward. No cycles are permitted.

```
                    aivyx-core
                    (types, traits, errors, IDs)
                   ╱    │    │    │    ╲
                  ╱     │    │    │     ╲
           crypto    config  audit  capability  federation
           (HKDF,   (TOML,  (HMAC, (RBAC,      (Ed25519,
            store)   dirs)   sink)  tokens)      relay)
              │        │
              │    ┌───┘
              │    │
             llm ──┘
            (providers, STT, TTS,
             embedding, streaming)
              │
              │
           memory
           (store, graph, retrieval,
            consolidation, outcomes)
              │
              │
            agent
           (sessions, tools, skills,
            sanitization, profiles)
```

### Dependency Rules

| Crate | Can Depend On |
|-------|---------------|
| `aivyx-core` | External crates only |
| `aivyx-crypto` | `core` |
| `aivyx-config` | `core` |
| `aivyx-audit` | `core`, `crypto` |
| `aivyx-capability` | `core` |
| `aivyx-federation` | `core` |
| `aivyx-llm` | `core`, `config` |
| `aivyx-mcp` | `core`, `config` |
| `aivyx-memory` | `core`, `crypto`, `llm` |
| `aivyx-agent` | All crates above |

---

## Data Flow

### Agent Turn (Chat Message → Response)

```
User Message
    │
    ▼
┌──────────────────┐
│  AgentSession    │ ← aivyx-agent
│  (turn loop)     │
└────────┬─────────┘
         │
    ┌────▼─────┐     ┌──────────────┐
    │ Sanitize │     │ CapabilitySet│ ← aivyx-capability
    │ Input    │     │ (permission  │
    └────┬─────┘     │  check)      │
         │           └──────┬───────┘
         │                  │
    ┌────▼──────────────────▼──┐
    │      LlmProvider         │ ← aivyx-llm
    │  (OpenAI / Claude /      │
    │   Gemini / Ollama)       │
    └────────────┬─────────────┘
                 │
          ┌──────▼──────┐
          │ Tool Calls? │
          └──┬──────┬───┘
         Yes │      │ No
             │      │
    ┌────────▼───┐  │    ┌──────────────┐
    │ Tool       │  │    │ AuditLog     │ ← aivyx-audit
    │ Dispatch   │  │    │ (HMAC chain) │
    │ (built-in  │  │    └──────────────┘
    │  or MCP)   │  │
    └────────┬───┘  │    ┌──────────────┐
             │      │    │ EncryptedStore│ ← aivyx-crypto
             │      │    │ (persist)     │
    ┌────────▼──────▼┐   └──────────────┘
    │ MemoryManager  │ ← aivyx-memory
    │ (extract facts,│
    │  update graph) │
    └────────────────┘
```

### Encryption Model

All persistent data flows through `EncryptedStore`, which provides transparent encryption:

```
                Passphrase
                    │
                    ▼
              ┌───────────┐
              │  Argon2id  │ ← Key stretching
              └─────┬─────┘
                    │
                    ▼
              ┌───────────┐
              │ MasterKey  │ ← 256-bit key
              └─────┬─────┘
                    │
                    ▼
              ┌───────────┐
              │   HKDF    │ ← Per-purpose key derivation
              │ (SHA-256) │   (per-tenant, per-context)
              └─────┬─────┘
                    │
         ┌──────────┼──────────┐
         ▼          ▼          ▼
    ┌─────────┐ ┌─────────┐ ┌─────────┐
    │ Store A │ │ Store B │ │ Store C │
    │(agent)  │ │(memory) │ │(audit)  │
    └─────────┘ └─────────┘ └─────────┘
         │          │          │
         ▼          ▼          ▼
    ChaCha20Poly1305 Encryption
    (Random nonce per write)
         │          │          │
         ▼          ▼          ▼
    ┌──────────────────────────────┐
    │       redb (embedded DB)     │
    └──────────────────────────────┘
```

---

## Crate Deep Dive

### aivyx-core

The foundation crate that all others depend on. Contains:

- **Type system** — `AgentId`, `SessionId`, `TenantId`, `TaskId` (UUID-based typed IDs)
- **Error hierarchy** — `AivyxError` with `thiserror` for structured errors
- **A2A types** — Google Agent-to-Agent protocol data structures
- **Storage backend trait** — `StorageBackend` with `put`/`get`/`delete`/`list_keys`
- **Progress events** — `ProgressEvent` enum for real-time status streaming
- **Principal model** — `Principal` for identity and authorization context

### aivyx-crypto

Handles all cryptographic operations:

- **Key derivation** — HKDF-SHA256 for deriving per-purpose keys from a master key
- **Encryption** — ChaCha20Poly1305 authenticated encryption with random nonces
- **Master key** — Argon2id password hashing for passphrase → key conversion
- **Encrypted store** — Transparent encrypt/decrypt wrapper over redb
- **Secret management** — `secrecy::SecretString` wrappers with `Zeroize` on drop

### aivyx-config

Configuration management:

- **Config parsing** — TOML-based `AivyxConfig` with provider, MCP, memory, and channel settings
- **Directory management** — `AivyxDirs` for `~/.aivyx/` structure (data, models, skills, plugins)
- **Autonomy policies** — `AutonomyPolicy` controlling what agents can do without approval
- **Plugin config** — MCP plugin definitions, template gallery
- **Embedding config** — Model selection and dimension settings

### aivyx-audit

Tamper-proof audit trail:

- **HMAC chain** — each entry's MAC includes the previous entry's MAC, creating a hash chain
- **Ed25519 signing** — optional cryptographic signatures on audit entries
- **Event taxonomy** — 30+ audit event types covering agent actions, security events, admin operations
- **Abuse detection** — sliding-window anomaly detection on tool call patterns
- **Search & export** — filtering, time-range queries, JSON/CSV export
- **Retention** — configurable retention policies with automatic pruning

### aivyx-capability

Permission and authorization model:

- **Capabilities** — fine-grained permissions (`Filesystem`, `Network`, `Shell`, `Custom(...)`)
- **Scope restrictions** — path patterns, URL patterns, command patterns
- **Capability tokens** — serializable, transferable permission bundles
- **Attenuation** — capabilities can be narrowed when delegating but never widened
- **Pattern matching** — glob-based scope matching for flexible permission rules

### aivyx-llm

Multi-provider LLM abstraction:

- **Providers** — OpenAI, Anthropic Claude, Google Gemini, Ollama (local)
- **Streaming** — async streaming responses via `tokio` channels
- **Multimodal** — vision support (image analysis) across all providers
- **Embeddings** — text embedding for vector search (configurable dimensions)
- **STT** — Speech-to-text via OpenAI Whisper and Ollama
- **TTS** — Text-to-speech via OpenAI TTS and edge-tts (free/local)
- **Factory** — `create_provider()` instantiates the correct provider from config

### aivyx-mcp

Model Context Protocol client:

- **Transports** — stdio (local processes) and SSE (remote servers)
- **OAuth 2.1** — PKCE flow for authenticated remote MCP servers
- **Sampling** — bidirectional support for MCP server-initiated LLM requests
- **Elicitation** — handling MCP server requests for user input
- **Caching** — tool/resource result caching with TTL
- **Proxy** — MCP-to-MCP proxy for protocol bridging

### aivyx-memory

Semantic memory and knowledge management:

- **Memory store** — encrypted storage of facts, preferences, decisions, outcomes
- **Knowledge graph** — entity-relationship triples with BFS traversal and community detection
- **Vector search** — cosine similarity over embedded memory entries
- **Consolidation** — clustering similar memories, decay pruning, LLM-driven merge
- **Retrieval router** — automatic strategy selection (vector/keyword/graph/multi-source)
- **Outcome tracking** — per-tool and per-step success/failure recording
- **Profile extraction** — automatic user preference extraction from conversations

### aivyx-agent

Agent session management and tool dispatch:

- **Session lifecycle** — create, turn, context compression, summary, end
- **Built-in tools** — filesystem, network, shell, analysis, document processing
- **MCP tool dispatch** — routing tool calls to appropriate MCP servers
- **Skill system** — TOML skill manifests with hot-loading
- **Prompt sanitization** — 3-layer defense against prompt injection
- **Cost tracking** — per-turn token usage and cost calculation
- **Rate limiting** — GCRA-based rate limiting per agent
- **Agent profiles** — configurable personality, capabilities, and model preferences

### aivyx-federation

Multi-instance federation:

- **Ed25519 auth** — cryptographic signing and verification of inter-instance messages
- **Trust policies** — per-peer allowed scopes and maximum capability tiers
- **Relay** — proxying chat and task requests between instances
- **Health monitoring** — peer liveness tracking with configurable timeouts
- **Failover** — capability-aware peer selection with automatic retry

---

## Storage Architecture

All persistent state is stored in `redb` (an embedded key-value database) via `EncryptedStore`:

```
~/.aivyx/
├── data/
│   ├── agent.redb          # Agent sessions, turns, context
│   ├── memory.redb         # Memory entries, knowledge triples
│   ├── audit.redb          # Audit log entries (HMAC chain)
│   └── cost.redb           # Cost tracking ledger
├── models/                 # Downloaded Ollama models
├── skills/                 # Skill manifest files
├── plugins/                # MCP plugin configurations
├── roles/                  # Custom agent role templates
└── tenants/{id}/           # Per-tenant isolated directories
    └── data/               # Tenant-specific encrypted stores
```

For multi-tenant deployments, each tenant gets an isolated directory tree with HKDF-derived per-tenant encryption keys, ensuring cryptographic isolation between tenants.

---

## Testing

The workspace contains ~711 tests covering all crates:

```bash
# Run all tests
cargo test --workspace

# Run tests for a specific crate
cargo test -p aivyx-memory

# Run with output
cargo test --workspace -- --nocapture
```

Test categories include unit tests, integration tests, and property-based tests (via `proptest`).
