# Contributing to Aivyx Core

Thank you for your interest in contributing to the shared foundation of the Aivyx ecosystem.

## Architecture

`aivyx-core` is a Cargo workspace of 10 crates that form the shared layer between the public [Aivyx agent](https://github.com/AivyxDev/aivyx-app) and the private Engine server.

```
aivyx-core (types, traits, errors)
├── aivyx-crypto (HKDF, ChaCha20Poly1305, passphrase)
├── aivyx-config (TOML, provider settings, SMTP)
├── aivyx-audit (tamper-proof log, HMAC chain)
├── aivyx-capability (RBAC, permission checks)
├── aivyx-llm (OpenAI, Anthropic, Gemini, Ollama)
├── aivyx-mcp (MCP tool protocol client)
├── aivyx-memory (semantic triples, encrypted KG)
├── aivyx-agent (sessions, tools, skill system)
└── aivyx-federation (multi-instance protocol)
```

## Development

```bash
# Build all crates
cargo build --workspace

# Run all tests
cargo test --workspace

# Check formatting
cargo fmt --check

# Lint
cargo clippy --workspace
```

## Guidelines

- **No breaking changes** without a version bump — both `aivyx` and `aivyx-engine` depend on these crates
- **All public types** must derive `Debug`, `Clone` and implement `serde::{Serialize, Deserialize}` where appropriate
- **Error types** go in `aivyx-core::AivyxError` — don't create new error enums in sub-crates
- **Secrets** must be wrapped in `secrecy::SecretString` — never log or display raw secrets
- **Tests** are required for all new public functions

## Crate Dependency Rules

```
aivyx-core ← everything depends on this
aivyx-crypto ← config, audit, memory, agent
aivyx-config ← llm, mcp, agent
aivyx-audit ← agent
aivyx-capability ← agent
aivyx-llm ← memory, agent
aivyx-mcp ← agent
aivyx-memory ← agent
aivyx-agent ← top-level, depends on all above
aivyx-federation ← depends only on aivyx-core
```

New crate dependencies must follow this DAG — no cycles allowed.

## License

All contributions are licensed under MIT.
