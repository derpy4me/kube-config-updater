# ADR-0002: Shell Out to `bw` CLI Rather Than Use SDK or HTTP API

## Status

Accepted

## Context

Three integration approaches exist for accessing Bitwarden Password Manager vault data:

1. **Shell out to `bw` CLI** — spawn `bw` processes, parse JSON output
2. **Rust SDK (`bitwarden` crate)** — native async Rust API
3. **Direct HTTP API** — call Bitwarden/Vaultwarden REST endpoints directly

The Rust SDK only covers Secrets Manager, not the Password Manager vault (see ADR-0001 — we chose Password Manager). The HTTP API requires client-side vault decryption (PBKDF2/Argon2 key derivation + AES-CBC), which would mean reimplementing Bitwarden's crypto layer.

The project is currently fully synchronous (no async runtime).

## Decision

We will shell out to the `bw` CLI for all vault operations.

The `bw` CLI handles authentication, session management, vault encryption/decryption, and server communication. We treat it as an opaque tool — send commands, parse JSON output.

Session keys are passed via the `BW_SESSION` environment variable (not `--session` CLI arg) to avoid exposure in process listings (`ps`).

This was chosen because:
- It handles all crypto — we don't reimplement Bitwarden's encryption
- It works identically with Bitwarden Cloud and Vaultwarden
- The JSON output is stable and well-documented
- It follows the same pattern the project already uses for `security` on macOS (shelling out to a system CLI for credential access)
- No new Rust dependencies or async runtime needed

The Rust SDK was rejected because it only covers Secrets Manager, not Password Manager. It would also pull in `tokio` and significantly increase binary size and compile time.

The direct HTTP API was rejected because vault items are encrypted at rest — the client must decrypt them. This would require implementing PBKDF2/Argon2 key derivation and AES-CBC decryption, which is error-prone and a maintenance burden.

## Consequences

**Positive:**
- Zero new Rust dependencies for vault access
- All cryptographic complexity delegated to a battle-tested tool
- Project remains fully synchronous
- JSON parsing with existing `serde_json` dependency

**Negative:**
- `bw` CLI must be installed on the user's machine (~100MB Node.js app)
- Each `bw` command spawns a new process — slower than an in-process SDK
- Error handling depends on parsing stderr strings, which may change across `bw` versions

**Neutral:**
- `serde_json` is already a dependency (used for state file) — no new crate needed
- The `bw` CLI stores its own encrypted cache at `~/.config/Bitwarden CLI/` — this is Bitwarden's concern, not ours
