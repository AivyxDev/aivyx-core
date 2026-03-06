# Aivyx Core

Shared foundation crates for the [Aivyx](https://aivyx-studio.com) ecosystem — a personal AI agent that runs locally, remembers context, and works alongside you.

## Crates

| Crate | Description |
|-------|-------------|
| [`aivyx-core`](crates/aivyx-core) | Types, traits, error types, and ID types |
| [`aivyx-crypto`](crates/aivyx-crypto) | HKDF key derivation, ChaCha20Poly1305 encryption |
| [`aivyx-config`](crates/aivyx-config) | TOML configuration parsing, provider settings |
| [`aivyx-audit`](crates/aivyx-audit) | Tamper-proof audit logging with HMAC chains |
| [`aivyx-capability`](crates/aivyx-capability) | RBAC capability model and permission checks |
| [`aivyx-llm`](crates/aivyx-llm) | LLM provider abstraction (OpenAI, Anthropic, Ollama, Gemini) |
| [`aivyx-mcp`](crates/aivyx-mcp) | Model Context Protocol tool interface |
| [`aivyx-memory`](crates/aivyx-memory) | Semantic triples, encrypted knowledge graph |
| [`aivyx-agent`](crates/aivyx-agent) | Agent session, built-in tools, tool dispatch |

## Architecture

```
aivyx-core          ← Foundation (types, traits)
├── aivyx-crypto    ← Encryption at rest
├── aivyx-audit     ← Tamper-proof audit log
├── aivyx-capability ← Permission model
├── aivyx-config    ← Configuration
│   └── aivyx-llm   ← LLM providers
│       └── aivyx-memory ← Encrypted knowledge graph
├── aivyx-mcp       ← MCP tool protocol
└── aivyx-agent     ← Agent session (depends on all above)
```

## Building

```bash
cargo build --workspace
cargo test --workspace
```

## License

MIT — see [LICENSE](LICENSE).
