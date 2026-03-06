# ADR-0003: Explicit Vault Password Delivery Over Transparent Backend

## Status

Accepted

## Context

Vault-sourced servers carry their SSH password embedded in the Bitwarden vault item. The existing credential lookup (`get_credential()` in `credentials.rs`) checks keyring → file fallback → default account. We need to decide how vault passwords reach `process_server`.

Two approaches:

1. **Explicit delivery (Option A)** — Vault passwords are stored in a `HashMap<String, String>` during Bitwarden loading. `process_server` receives an optional `vault_password: Option<&str>` parameter. When present, it skips `get_credential()` entirely.

2. **Transparent backend (Option B)** — Implement `KeyringBackend` trait for Bitwarden so `get_credential()` transparently checks vault → keyring → file. This requires the `BwCli` state (session key, loaded items) to be accessible from `get_credential()`, which currently has no state.

## Decision

We will use explicit vault password delivery (Option A).

`process_server` gains a `vault_password: Option<&str>` parameter. When `Some`, it uses that password directly. When `None`, it falls back to the existing `get_credential()` flow.

The TUI stores vault passwords in `AppState.vault_passwords: HashMap<String, String>`. The CLI path builds the same map during startup and passes it through.

This was chosen because:
- Vault servers are fundamentally a different source than local+keyring servers — making this explicit is clearer than hiding it behind a trait
- No shared mutable state needed — the HashMap is built once during vault loading
- Easier to test — no global/static state for the vault session
- The `credentials.rs` module stays unchanged — its responsibility is local credential storage, not vault access

Option B was rejected because it would require threading `BwCli` state (session key, item cache) into the `get_credential()` call path, which currently takes no state. This would either require global state or a refactor of every call site.

## Consequences

**Positive:**
- Clear separation: vault credentials flow through an explicit path, local credentials through `get_credential()`
- `credentials.rs` requires zero changes
- Easy to test: mock the HashMap, no keyring interaction needed
- No hidden coupling between vault session state and credential lookup

**Negative:**
- `process_server` signature changes — all call sites must be updated (3 in TUI, 1 in CLI)
- Two credential paths to maintain: vault HashMap and keyring/file lookup

**Neutral:**
- The `vault_password` parameter is `None` for all existing (non-vault) code paths — behavior is unchanged when Bitwarden is disabled
