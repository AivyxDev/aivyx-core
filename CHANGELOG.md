# Changelog

All notable changes to aivyx-core will be documented in this file.

## [0.1.0] — 2026-03-07

### 🎉 Initial Release

10 shared crates extracted from monorepo into standalone workspace:

- `aivyx-core` — types, traits, error types
- `aivyx-crypto` — HKDF-SHA256, ChaCha20Poly1305, passphrase handling
- `aivyx-config` — TOML config, provider settings
- `aivyx-audit` — tamper-proof HMAC chain, ed25519 signing
- `aivyx-capability` — RBAC permission model
- `aivyx-llm` — multi-provider LLM (OpenAI, Anthropic, Gemini, Ollama)
- `aivyx-mcp` — Model Context Protocol client
- `aivyx-memory` — semantic triples, encrypted knowledge graph
- `aivyx-agent` — agent sessions, tool dispatch, skill system
- `aivyx-federation` — multi-instance federation protocol
