# Aivyx Core

**Shared foundation crates for the [Aivyx](https://aivyx-studio.com) ecosystem** — a privacy-first, locally-hosted AI agent platform that encrypts everything at rest, enforces capability-based permissions, and maintains tamper-proof audit trails.

> **Version**: 0.1.0 · **License**: MIT · **Rust Edition**: 2024
>
> For technical architecture, see [ARCHITECTURE.md](ARCHITECTURE.md).
> For feature details, see [FEATURES.md](FEATURES.md).
> For the security model, see [SECURITY.md](SECURITY.md).

---

## Overview

`aivyx-core` is a Cargo workspace of **10 crates** (~45,200 lines of Rust, ~805 tests) that form the shared layer between:

- **[Aivyx](https://github.com/AivyxDev/aivyx)** — the public CLI + Desktop app (MIT)
- **[Aivyx Engine](https://aivyx-gitea.cloud/AivyxDev/aivyx-engine)** — the private server and orchestration layer (Proprietary)

Every capability that both products share — encryption, LLM providers, memory, MCP tools, agent sessions, audit logging, federation — lives here. This ensures a single source of truth for security invariants and protocol implementations.

### Design Principles

1. **Privacy first** — all data encrypted at rest with ChaCha20Poly1305; keys derived via HKDF-SHA256
2. **Capability attenuation** — agents can only access resources explicitly granted; permissions can be narrowed but never widened
3. **Tamper-proof audit** — every significant action is logged in an HMAC chain with Ed25519 signatures
4. **Provider agnostic** — works with OpenAI, Anthropic, Google Gemini, and Ollama (local)
5. **Protocol native** — implements MCP (agent-to-tool) and A2A (agent-to-agent) standards

---

## Crates

| Crate | Description | Key Types |
|-------|-------------|-----------|
| [`aivyx-core`](crates/aivyx-core) | Foundation types, traits, error types, ID types | `AivyxError`, `AgentId`, `SessionId`, `StorageBackend` |
| [`aivyx-crypto`](crates/aivyx-crypto) | HKDF key derivation, ChaCha20Poly1305 encryption | `MasterKey`, `EncryptedStore`, `EncryptedBackend` |
| [`aivyx-config`](crates/aivyx-config) | TOML configuration, provider settings, directory management | `AivyxConfig`, `ProviderConfig`, `AivyxDirs` |
| [`aivyx-audit`](crates/aivyx-audit) | Tamper-proof audit logging with HMAC chains | `AuditLog`, `AuditEvent`, `AbuseDetector` |
| [`aivyx-capability`](crates/aivyx-capability) | RBAC capability model and permission checks | `Capability`, `CapabilitySet`, `CapabilityToken` |
| [`aivyx-llm`](crates/aivyx-llm) | Multi-provider LLM abstraction with streaming | `LlmProvider`, `SttProvider`, `TtsProvider` |
| [`aivyx-mcp`](crates/aivyx-mcp) | Model Context Protocol tool interface | `McpClient`, `McpTransport`, `McpOAuthClient` |
| [`aivyx-memory`](crates/aivyx-memory) | Semantic memory, knowledge graph, retrieval | `MemoryManager`, `KnowledgeGraph`, `RetrievalRouter` |
| [`aivyx-agent`](crates/aivyx-agent) | Agent sessions, built-in tools, skill system | `AgentSession`, `AgentProfile`, `ToolDispatch` |
| [`aivyx-federation`](crates/aivyx-federation) | Multi-instance federation protocol | `FederationClient`, `TrustPolicy`, `FederationAuth` |

---

## Architecture

```
aivyx-core              ← Foundation (types, traits, errors)
├── aivyx-crypto        ← Encryption at rest (HKDF + ChaCha20Poly1305)
├── aivyx-config        ← TOML configuration, directory structure
├── aivyx-audit         ← Tamper-proof audit log (HMAC + Ed25519)
├── aivyx-capability    ← Permission model (RBAC + attenuation)
├── aivyx-llm           ← LLM providers (OpenAI, Claude, Gemini, Ollama)
│   ├── STT             ← Speech-to-text (Whisper, Ollama)
│   └── TTS             ← Text-to-speech (OpenAI TTS, edge-tts)
├── aivyx-mcp           ← MCP tool protocol (stdio, SSE, OAuth 2.1)
├── aivyx-memory        ← Semantic memory + knowledge graph
│   ├── GraphRAG        ← BFS traversal, community detection
│   ├── Consolidation   ← Clustering, decay, merge
│   ├── Retrieval       ← Vector, keyword, graph, multi-source
│   └── Outcomes        ← Tracking, feedback loops
├── aivyx-agent         ← Agent session (depends on all above)
│   ├── Built-in tools  ← File, network, analysis, document tools
│   ├── Skill system    ← Skill manifests, hot-loading
│   └── Sanitization    ← Prompt injection defense
└── aivyx-federation    ← Multi-instance protocol (Ed25519, trust policies)
```

For a deep dive into the architecture, see [ARCHITECTURE.md](ARCHITECTURE.md).

---

## Building

### Prerequisites

- **Rust** — see `rust-toolchain.toml` (edition 2024)
- **System libraries** — OpenSSL development headers for `reqwest`

### Commands

```bash
# Build all crates
cargo build --workspace

# Run all tests
cargo test --workspace

# Check formatting
cargo fmt --check

# Lint with warnings as errors
cargo clippy --workspace -- -D warnings

# Security audit
cargo audit
```

---

## Related Documentation

| Document | Description |
|----------|-------------|
| [ARCHITECTURE.md](ARCHITECTURE.md) | Technical architecture, data flow, and design decisions |
| [FEATURES.md](FEATURES.md) | Comprehensive feature inventory with implementation details |
| [SECURITY.md](SECURITY.md) | Application security model and cryptographic design |
| [SWOT.md](SWOT.md) | Strategic SWOT analysis |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Development guidelines and dependency rules |
| [CHANGELOG.md](CHANGELOG.md) | Version history |

---

## License

MIT — see [LICENSE](LICENSE).
