# Bitwarden/Vaultwarden Integration Design

**Date:** 2026-03-05
**Status:** Approved
**ADRs:** 0001, 0002, 0003, 0004

---

## Summary

Add an optional Bitwarden/Vaultwarden vault backend that provides both server configurations and SSH credentials from a company-managed vault. The existing keyring + file fallback system remains the default. Vault integration is enabled via a `[bitwarden]` section in `config.toml`.

---

## Architecture

```
                    ┌─────────────────────────┐
                    │     config.toml          │
                    │  ┌──────────┐ ┌────────┐ │
                    │  │[bitwarden]│ │[[server]]│ │
                    │  └──────────┘ └────────┘ │
                    └─────┬───────────┬───────┘
                          │           │
                    ┌─────▼─────┐  ┌──▼──────────┐
                    │ bitwarden │  │ Local servers │
                    │ module    │  │ + keyring/    │
                    │ (bw CLI)  │  │   file creds  │
                    └─────┬─────┘  └──┬───────────┘
                          │           │
                    ┌─────▼───────────▼──┐
                    │   Merged server    │
                    │   list + passwords │
                    │   (local wins)     │
                    └─────────┬──────────┘
                              │
                 ┌────────────┼────────────┐
                 │            │            │
           ┌─────▼───┐  ┌────▼────┐  ┌───▼──────┐
           │   TUI    │  │  CLI    │  │  Cron    │
           │ interactive│ │ (none)  │  │ headless │
           └──────────┘  └─────────┘  └─────────┘
```

### Key Decisions

| Decision | Choice | ADR |
|----------|--------|-----|
| Which Bitwarden product | Password Manager (`bw` CLI) | 0001 |
| Integration method | Shell out to `bw` CLI | 0002 |
| Credential delivery | Explicit HashMap (not transparent backend) | 0003 |
| Cron authentication | API key + password file | 0004 |
| Config storage | Custom fields on Login items | 0001 |
| Merge strategy | Local overrides vault by server name | — |
| Feature flag | Runtime `[bitwarden]` in config.toml | — |

---

## New Module: `src/bitwarden.rs`

All `bw` CLI interaction lives here. No Bitwarden logic leaks into other modules except at integration boundaries.

### Types

```rust
/// [bitwarden] section from config.toml
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct BitwardenConfig {
    pub enabled: bool,
    pub server_url: Option<String>,
    pub collection: Option<String>,
    pub item_prefix: Option<String>,
    pub password_file: Option<PathBuf>,
}

/// A server + credential from a vault item
pub struct VaultServer {
    pub server: config::Server,
    pub password: Option<String>,
    pub vault_item_id: String,
}

/// Vault lock/auth status
pub enum VaultStatus { Unauthenticated, Locked, Unlocked }

/// CLI wrapper — holds session key in memory
pub struct BwCli {
    session: Option<String>,
    server_url: Option<String>,
}
```

### `BwCli` API

| Method | Purpose |
|--------|---------|
| `is_available() -> bool` | Check if `bw` binary exists |
| `status() -> Result<VaultStatus>` | Get vault lock/auth state |
| `unlock(&mut self, password: &str) -> Result<()>` | Unlock with master password (TUI path) |
| `login_headless(&mut self, password_file: &Path) -> Result<()>` | API key + password file (cron path) |
| `ensure_session(&mut self, password_file: Option<&Path>) -> Result<()>` | Auto-detect: BW_SESSION → headless → Err |
| `fetch_servers(&self, prefix: &str, collection: Option<&str>) -> Result<Vec<VaultServer>>` | Fetch + parse vault items |
| `run(&self, args: &[&str]) -> Result<String>` | Internal: execute bw with session via env var |

### Security: session key delivery

```rust
// Session via env var — NOT visible in `ps`
cmd.env("BW_SESSION", session);
// NOT: cmd.args(["--session", session]);
```

### Vault item → Server mapping

```
Bitwarden field               → Server struct field
─────────────────────────────────────────────────────
name (strip prefix)            → server.name
fields["address"]              → server.address           (required)
fields["target_cluster_ip"]    → server.target_cluster_ip  (required)
login.username                 → server.user
fields["file_path"]            → server.file_path
fields["file_name"]            → server.file_name
fields["context_name"]         → server.context_name
fields["identity_file"]        → server.identity_file
login.password                 → vault_password (HashMap)
```

Items missing required fields are skipped with a warning — they don't fail the whole fetch.

---

## Config Changes: `src/config.rs`

```rust
pub struct Config {
    // ...existing fields...
    #[serde(default)]
    pub bitwarden: Option<BitwardenConfig>,
}
```

Example config.toml:

```toml
local_output_dir = "~/.kube"
default_user = "ubuntu"

[bitwarden]
enabled = true
server_url = "https://vault.strata.com"
collection = "K3s Production"
item_prefix = "k3s:"
password_file = "/etc/kube-config-updater/bw-password"

[[server]]
name = "special-node"   # local override — wins over vault item with same name
address = "10.0.0.99"
target_cluster_ip = "10.0.0.99"
```

---

## Server Merge

```rust
pub fn merge_servers(
    local_servers: &[Server],
    vault_servers: Vec<VaultServer>,
) -> (Vec<Server>, HashMap<String, ServerSource>, HashMap<String, String>)
```

Returns:
1. Merged server list (local first, then vault servers not overridden by name)
2. Source map for TUI display
3. Vault passwords for fetch

---

## TUI Integration (Vertical Slice)

### New types in `tui/app.rs`

```rust
#[derive(Clone, Copy, PartialEq)]
pub enum ServerSource { Local, Vault }

// New View variant
View::BitwardenUnlock { error: Option<String> }

// New event
AppEvent::BitwardenComplete { result: Result<Vec<VaultServer>, String> }

// New AppState fields
pub server_sources: HashMap<String, ServerSource>,
pub vault_passwords: HashMap<String, String>,
pub bw_session: Option<String>,
```

### New file: `src/tui/features/bitwarden.rs`

| Function | Purpose |
|----------|---------|
| `render(frame, app)` | Password prompt with status/error |
| `handle_key(app, key, tx) -> bool` | Capture password via MaskedInput, spawn unlock on Enter |

Unlock spawns a background thread that:
1. Creates `BwCli`, calls `unlock(password)`
2. On success: calls `fetch_servers(prefix, collection)`
3. Sends `AppEvent::BitwardenComplete { result }` to event loop

### Dashboard changes (`features/dashboard.rs`)

- Source badge: servers show `[vault]` or `[local]`
- Credential column: vault servers show "Vault" instead of "Stored"/"Not stored"
- `D` key: disabled for vault servers ("Vault servers are managed in Bitwarden")
- `c` key: disabled for vault servers ("Credentials managed by vault")

### Detail view changes (`features/detail.rs`)

- Shows `Source: Vault` or `Source: Local`
- Fetch uses `vault_passwords` map for vault servers
- `c` key disabled for vault servers

### Startup flow (`tui/mod.rs`)

```
run_tui(config, ...)
  ├─ config.bitwarden.enabled?
  │   ├─ No → proceed as today
  │   └─ Yes → BwCli::is_available()?
  │       ├─ No → View::Error("Install bw CLI...")
  │       └─ Yes → BwCli::ensure_session(password_file)?
  │           ├─ Ok → fetch_servers → merge → Dashboard
  │           └─ Err(locked) → View::BitwardenUnlock
  │           └─ Err(other) → View::Error(msg)
  └─ Dashboard
```

---

## CLI/Cron Path: `src/main.rs`

### Non-TUI flow

```rust
None => {
    let (config, vault_passwords) = if bitwarden_enabled(&config) {
        load_vault_servers(config)?
    } else {
        (config, HashMap::new())
    };
    fetch::process_servers(&config, &cli.servers, cli.dry_run, &vault_passwords)?;
}
```

### `process_server` signature change

```rust
pub(crate) fn process_server(
    server: &Server,
    config: &Config,
    dry_run: bool,
    force: bool,
    vault_password: Option<&str>,  // NEW — when Some, skip get_credential()
) -> Result<ServerResult, anyhow::Error>
```

### Cron auth cascade (`BwCli::ensure_session`)

```
1. BW_SESSION env → use directly
2. BW_CLIENTID + BW_CLIENTSECRET + password_file → bw login --apikey + bw unlock --passwordfile
3. Neither → Err (TUI handles interactive separately)
```

---

## Error Handling

### New `friendly_error` patterns

| Condition | Message |
|-----------|---------|
| `bw` not found | "Bitwarden CLI not installed. Run: npm i -g @bitwarden/cli" |
| Vault locked | "Vault is locked. Press Enter to unlock." |
| Wrong master password | "Wrong master password. Try again." |
| Network error to vault | "Cannot reach Bitwarden server at <url>" |
| Collection empty | "No items found in collection '<name>'" |
| Item missing fields | Skipped with warning (doesn't fail batch) |

### Password file permission check

On startup, if `password_file` is configured, check Unix permissions. Warn (like SSH does) if the file is world/group-readable. Format: "Password file has permissions 0644 — should be 0600."

---

## File Change Summary

| File | Action | Changes |
|------|--------|---------|
| `src/bitwarden.rs` | **Create** | BwCli, BwItem, VaultServer, BitwardenConfig, merge_servers |
| `src/tui/features/bitwarden.rs` | **Create** | render + handle_key for unlock view |
| `src/main.rs` | Modify | mod bitwarden, vault loading in CLI path |
| `src/config.rs` | Modify | bitwarden field on Config |
| `src/fetch.rs` | Modify | vault_password param on process_server/process_servers |
| `src/tui/app.rs` | Modify | ServerSource, BitwardenUnlock view, BitwardenComplete event, AppState fields |
| `src/tui/mod.rs` | Modify | Startup check, event handler, spawn_fetch with vault password |
| `src/tui/features/mod.rs` | Modify | pub mod bitwarden |
| `src/tui/features/dashboard.rs` | Modify | Source badge, disable D/c for vault servers |
| `src/tui/features/detail.rs` | Modify | Source line, vault password in fetch |

**Unchanged:** ssh.rs, kube.rs, state.rs, credentials.rs

---

## Security Model

See the README Security section for the user-facing summary. Key points:

- Session keys held in memory only, never written to disk
- Master password cleared immediately after `bw unlock` call
- Vault passwords in HashMap cleared on process exit
- `bw` session passed via env var, not CLI arg (avoids `ps` exposure)
- `password_file` requires `chmod 600` — tool warns on overly permissive files
- Access control delegated to Bitwarden — the tool sees only what the user's account can access
