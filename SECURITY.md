# Security — Aivyx Core

> Application security model and cryptographic design for the shared foundation crates.
>
> This document covers **application-level security** — how Aivyx protects data, enforces permissions,
> and defends against common threats. For operational security (SSH, VPS access, CI/CD), see the
> Engine's `OPS_REFERENCE.md`.
>
> **Last updated**: 2026-03-08

---

## Security Philosophy

Aivyx takes a **defense-in-depth** approach with four foundational principles:

1. **Encrypted by default** — there is no unencrypted storage path; all data is encrypted at rest
2. **Least privilege** — agents operate with the minimum permissions required (capability attenuation)
3. **Audit everything** — every significant action is recorded in a tamper-proof HMAC chain
4. **Fail closed** — missing permissions, invalid signatures, and expired tokens result in denial

---

## Encryption Architecture

### Key Hierarchy

```
User Passphrase
      │
      ▼
┌──────────┐
│ Argon2id │  Parameters: memory=64MB, iterations=3, parallelism=1
└────┬─────┘
     │
     ▼
┌──────────┐
│ MasterKey│  256-bit derived key
└────┬─────┘
     │
     ▼
┌──────────┐
│   HKDF   │  SHA-256, per-purpose info strings
└────┬─────┘
     │
     ├──▶ Store Key "agent-store"
     ├──▶ Store Key "memory-store"
     ├──▶ Store Key "audit-store"
     ├──▶ Store Key "cost-store"
     └──▶ Tenant Keys (derive_tenant_key)
```

### Algorithms

| Purpose | Algorithm | Key Size | Notes |
|---------|-----------|----------|-------|
| Passphrase → Key | Argon2id | 256-bit | Memory-hard, resists GPU attacks |
| Key derivation | HKDF-SHA256 | 256-bit | Per-purpose and per-tenant |
| Data encryption | ChaCha20Poly1305 | 256-bit | AEAD with 96-bit random nonce per write |
| Audit signing | Ed25519 | 256-bit | Optional per-entry signatures |
| Audit chaining | HMAC-SHA256 | 256-bit | Chain integrity verification |

### Implementation Details

- **Random nonces** — every `EncryptedStore.put()` generates a fresh 96-bit random nonce
- **Authenticated encryption** — ChaCha20Poly1305 provides both confidentiality and integrity
- **No key reuse** — HKDF derives unique keys for each store and each tenant
- **Zeroize on drop** — `MasterKey`, encryption keys, and `SecretString` values implement `Zeroize`, ensuring key material is overwritten in memory when dropped

---

## Secret Management

All sensitive values are protected by the `secrecy` crate:

- **API keys** wrapped in `SecretString` — prevents accidental logging or display
- **Passwords** wrapped in `SecretString` — never appear in debug output
- **Key material** implements `Zeroize` — memory is overwritten when values go out of scope
- **No secrets in logs** — `tracing` formatters never emit `SecretString` contents
- **No secrets in errors** — `AivyxError` variants never contain raw secret values

---

## Capability-Based Authorization

### Model

```
Agent Profile
    │
    ▼
CapabilitySet
    │
    ├── Capability::Filesystem { scope: "/home/user/**" }
    ├── Capability::Network { scope: "*.aivyx.ai" }
    └── Capability::Custom("memory") { scope: "read" }
```

### Security Properties

| Property | Guarantee |
|----------|-----------|
| **Attenuation** | Delegated capabilities can be narrowed, never widened |
| **Principle of least privilege** | Agents start with minimal permissions |
| **Scope enforcement** | Every tool call validated against granted scopes |
| **No ambient authority** | No implicit permissions — everything must be explicitly granted |
| **Wildcard detection** | `WildcardShell`, `WildcardFilesystem`, `WildcardNetwork` flag overly permissive grants |

### Capability Audit

The `CapabilityAuditReport` system can scan all agent profiles and flag security concerns:

- Wildcarded filesystem/shell/network access
- Unrestricted custom capabilities
- High autonomy combined with broad scope

---

## Tamper-Proof Audit Chain

### Design

```
Entry[0]                Entry[1]                Entry[2]
┌──────────┐            ┌──────────┐            ┌──────────┐
│ Event    │            │ Event    │            │ Event    │
│ Time     │            │ Time     │            │ Time     │
│ Agent    │            │ Agent    │            │ Agent    │
│ prev_mac │──────────▶ │ prev_mac │──────────▶ │ prev_mac │
│ MAC      │            │ MAC      │            │ MAC      │
│ [Sig]    │            │ [Sig]    │            │ [Sig]    │
└──────────┘            └──────────┘            └──────────┘
```

- **Chain verification** — any modification to a past entry breaks the MAC chain for all subsequent entries
- **Append-only** — entries can only be appended, never modified or deleted (until retention prunes them)
- **Ed25519 signatures** — optional per-entry signatures provide non-repudiation
- **30+ event types** — comprehensive taxonomy covering all security-relevant actions

### Abuse Detection

The `AbuseDetector` monitors for anomalous agent behavior:

| Detection | Trigger | Action |
|-----------|---------|--------|
| Tool frequency spike | >N calls in sliding window | `SecurityAlert` audit event |
| Repeated denials | >N permission denials in window | `SecurityAlert` audit event |
| Scope escalation | Agent requests higher capability | `SecurityAlert` audit event |

Thresholds are configurable via `AbuseDetectorConfig`.

---

## Prompt Injection Defense

Three-layer defense against prompt injection attacks:

### Layer 1: Input Sanitization

`sanitize_user_input()` escapes known delimiter patterns:

- ChatML delimiters (`<|im_start|>`, `<|im_end|>`)
- Llama delimiters (`[INST]`, `[/INST]`, `<<SYS>>`)
- Mistral delimiters (`[INST]`, `[/INST]`)

### Layer 2: Tool Output Boundaries

`wrap_tool_output()` wraps all tool outputs with boundary markers:

```
─── TOOL OUTPUT START ───
[tool result here]
─── TOOL OUTPUT END ───
```

Combined with `TOOL_OUTPUT_INSTRUCTION` in the system prompt, this teaches the LLM to treat tool outputs as data, not instructions.

### Layer 3: Webhook Payload Sanitization

`sanitize_webhook_payload()` applies the same escaping to any external payload received via webhooks, preventing external actors from injecting instructions through webhook triggers.

---

## Supply Chain Security

- **`cargo audit`** — run in CI on every push, blocking on known vulnerabilities
- **`deny.toml`** — license compliance checking, advisory database, wildcard dependency bans
- **`Cargo.lock`** — committed to ensure reproducible builds
- **Minimal dependency surface** — careful crate selection, no unnecessary transitive dependencies

---

## Per-Tenant Isolation

For multi-tenant deployments:

| Layer | Mechanism |
|-------|-----------|
| **Cryptographic** | HKDF per-tenant key derivation — each tenant's data encrypted with a unique key |
| **Directory** | Separate `~/.aivyx/tenants/{id}/` directory tree per tenant |
| **API** | API tokens scoped to tenant with `tenant_id` field |
| **Resource** | Per-tenant quotas on agents, sessions, storage, and LLM tokens |
| **Billing** | Cost entries tagged with tenant ID for chargeback |

Even if the underlying storage medium is compromised, one tenant's data cannot be decrypted with another tenant's key.

---

## Threat Model Summary

| Threat | Mitigation |
|--------|-----------|
| Data at rest exposure | ChaCha20Poly1305 encryption on all stores |
| Key compromise | HKDF isolation — compromising one key doesn't expose others |
| Audit log tampering | HMAC chain + Ed25519 signatures |
| Prompt injection | 3-layer sanitization (input, tool output, webhook) |
| Privilege escalation | Capability attenuation — can narrow, never widen |
| Agent abuse | Sliding-window anomaly detection with configurable thresholds |
| Supply chain attack | `cargo audit` + `deny.toml` in CI |
| Secret leakage | `secrecy::SecretString` + `Zeroize` on drop |
| Cross-tenant access | Per-tenant HKDF keys + directory isolation |
