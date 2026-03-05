# Bitwarden/Vaultwarden Integration — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add optional Bitwarden/Vaultwarden vault backend for company-managed SSH credentials and server configs.

**Architecture:** New `src/bitwarden.rs` module wraps the `bw` CLI. Vault items are parsed into `VaultServer` structs and merged with local config. Credentials flow explicitly via HashMap (ADR-0003). TUI gets a vertical-slice `bitwarden.rs` feature for vault unlock.

**Tech Stack:** Rust, `serde_json` (existing dep), `bw` CLI (external), ratatui TUI framework (existing)

**Design doc:** `docs/plans/2026-03-05-bitwarden-integration-design.md`
**ADRs:** `docs/adr/0001-0004`

---

## Task 1: Config Extension — Add `BitwardenConfig` to `Config`

**Files:**
- Create: `src/bitwarden.rs` (types only — no CLI logic yet)
- Modify: `src/config.rs` — add `bitwarden` field to `Config`
- Modify: `src/main.rs` — add `mod bitwarden;`
- Test: `src/config.rs` (inline config_tests module)

**Step 1: Create `src/bitwarden.rs` with `BitwardenConfig`**

```rust
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Parsed from the `[bitwarden]` section in config.toml.
/// When absent, the tool behaves exactly as before.
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct BitwardenConfig {
    pub enabled: bool,
    pub server_url: Option<String>,
    pub collection: Option<String>,
    pub item_prefix: Option<String>,
    pub password_file: Option<PathBuf>,
}
```

**Step 2: Add `mod bitwarden;` to `src/main.rs`**

Add after `mod credentials;`:
```rust
mod bitwarden;
```

**Step 3: Add `bitwarden` field to `Config` in `src/config.rs`**

Add to the `Config` struct:
```rust
#[serde(default)]
pub bitwarden: Option<crate::bitwarden::BitwardenConfig>,
```

Also add to the empty config in `tui/mod.rs::run_tui_setup`:
- No change needed — `Option` defaults to `None` via struct literal, and we're not using `..Default::default()` there.

**Step 4: Write test — config with [bitwarden] section parses correctly**

Add to `config_tests` in `src/config.rs`:
```rust
#[test]
fn test_load_config_with_bitwarden_section() {
    let content = r#"
local_output_dir = "/tmp/kube"

[bitwarden]
enabled = true
server_url = "https://vault.example.com"
collection = "K3s Prod"
item_prefix = "k3s:"
"#;
    let f = write_temp_config(content);
    let config = load_config(f.path().to_str().unwrap()).expect("should parse");
    let bw = config.bitwarden.expect("bitwarden section should be present");
    assert!(bw.enabled);
    assert_eq!(bw.server_url.as_deref(), Some("https://vault.example.com"));
    assert_eq!(bw.collection.as_deref(), Some("K3s Prod"));
    assert_eq!(bw.item_prefix.as_deref(), Some("k3s:"));
}

#[test]
fn test_load_config_without_bitwarden_section() {
    let f = write_temp_config("local_output_dir = \"/tmp/kube\"\n");
    let config = load_config(f.path().to_str().unwrap()).expect("should parse");
    assert!(config.bitwarden.is_none());
}
```

**Step 5: Run tests**

Run: `cargo test config_tests -- --nocapture`
Expected: All pass, including the two new tests.

**Step 6: Commit**

```bash
git add src/bitwarden.rs src/config.rs src/main.rs
git commit -m "feat: add BitwardenConfig type and [bitwarden] config section"
```

---

## Task 2: Vault Item Parsing — `BwItem` → `VaultServer`

**Files:**
- Modify: `src/bitwarden.rs` — add BwItem, BwLogin, BwField, VaultServer, conversion logic
- Test: `src/bitwarden.rs` (inline tests module)

**Step 1: Add deserialization types and VaultServer**

In `src/bitwarden.rs`, add:

```rust
use std::collections::HashMap;

/// A server + credential extracted from a Bitwarden vault item.
pub struct VaultServer {
    pub server: crate::config::Server,
    pub password: Option<String>,
    pub vault_item_id: String,
}

/// Deserialized from `bw list items` / `bw get item` JSON output.
#[derive(Deserialize)]
pub(crate) struct BwItem {
    pub id: String,
    pub name: String,
    pub login: Option<BwLogin>,
    pub fields: Option<Vec<BwField>>,
    #[serde(rename = "collectionIds", default)]
    pub collection_ids: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct BwLogin {
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct BwField {
    pub name: String,
    pub value: Option<String>,
    #[serde(rename = "type")]
    pub field_type: u8,
}
```

**Step 2: Add `BwItem` → `VaultServer` conversion**

```rust
impl BwItem {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields.as_ref()?
            .iter()
            .find(|f| f.name == name)
            .and_then(|f| f.value.as_deref())
    }

    pub(crate) fn to_vault_server(&self, prefix: &str) -> Result<VaultServer, String> {
        let server_name = self.name.strip_prefix(prefix).unwrap_or(&self.name);

        let address = self.field("address")
            .ok_or_else(|| format!("vault item '{}' missing 'address' field", self.name))?;
        let target_ip = self.field("target_cluster_ip")
            .ok_or_else(|| format!("vault item '{}' missing 'target_cluster_ip' field", self.name))?;

        Ok(VaultServer {
            server: crate::config::Server {
                name: server_name.to_string(),
                address: address.to_string(),
                target_cluster_ip: target_ip.to_string(),
                user: self.login.as_ref().and_then(|l| l.username.clone()),
                file_path: self.field("file_path").map(|s| s.to_string()),
                file_name: self.field("file_name").map(|s| s.to_string()),
                context_name: self.field("context_name").map(|s| s.to_string()),
                identity_file: self.field("identity_file").map(|s| s.to_string()),
            },
            password: self.login.as_ref().and_then(|l| l.password.clone()),
            vault_item_id: self.id.clone(),
        })
    }
}
```

**Step 3: Write tests for vault item parsing**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn sample_bw_json() -> &'static str {
        r#"[
            {
                "id": "uuid-1",
                "name": "k3s:prod-node",
                "login": { "username": "root", "password": "s3cret" },
                "fields": [
                    { "name": "address", "value": "192.168.1.10", "type": 0 },
                    { "name": "target_cluster_ip", "value": "10.0.0.1", "type": 0 },
                    { "name": "file_path", "value": "/etc/rancher/k3s", "type": 0 },
                    { "name": "context_name", "value": "prod", "type": 0 }
                ],
                "collectionIds": ["col-1"]
            },
            {
                "id": "uuid-2",
                "name": "k3s:staging",
                "login": { "username": "admin", "password": null },
                "fields": [
                    { "name": "address", "value": "192.168.2.10", "type": 0 },
                    { "name": "target_cluster_ip", "value": "10.0.0.2", "type": 0 }
                ],
                "collectionIds": []
            }
        ]"#
    }

    #[test]
    fn test_parse_bw_items() {
        let items: Vec<BwItem> = serde_json::from_str(sample_bw_json()).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "k3s:prod-node");
        assert_eq!(items[0].login.as_ref().unwrap().username.as_deref(), Some("root"));
    }

    #[test]
    fn test_bw_item_to_vault_server() {
        let items: Vec<BwItem> = serde_json::from_str(sample_bw_json()).unwrap();
        let vs = items[0].to_vault_server("k3s:").unwrap();
        assert_eq!(vs.server.name, "prod-node");
        assert_eq!(vs.server.address, "192.168.1.10");
        assert_eq!(vs.server.target_cluster_ip, "10.0.0.1");
        assert_eq!(vs.server.user.as_deref(), Some("root"));
        assert_eq!(vs.server.file_path.as_deref(), Some("/etc/rancher/k3s"));
        assert_eq!(vs.server.context_name.as_deref(), Some("prod"));
        assert_eq!(vs.password.as_deref(), Some("s3cret"));
        assert_eq!(vs.vault_item_id, "uuid-1");
    }

    #[test]
    fn test_bw_item_strips_prefix() {
        let items: Vec<BwItem> = serde_json::from_str(sample_bw_json()).unwrap();
        let vs = items[1].to_vault_server("k3s:").unwrap();
        assert_eq!(vs.server.name, "staging");
    }

    #[test]
    fn test_bw_item_missing_required_field() {
        let json = r#"[{
            "id": "uuid-bad",
            "name": "k3s:broken",
            "login": null,
            "fields": [{ "name": "address", "value": "1.2.3.4", "type": 0 }],
            "collectionIds": []
        }]"#;
        let items: Vec<BwItem> = serde_json::from_str(json).unwrap();
        let result = items[0].to_vault_server("k3s:");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("target_cluster_ip"));
    }

    #[test]
    fn test_bw_item_null_password() {
        let items: Vec<BwItem> = serde_json::from_str(sample_bw_json()).unwrap();
        let vs = items[1].to_vault_server("k3s:").unwrap();
        assert!(vs.password.is_none());
    }
}
```

**Step 4: Run tests**

Run: `cargo test bitwarden::tests -- --nocapture`
Expected: All 5 tests pass.

**Step 5: Commit**

```bash
git add src/bitwarden.rs
git commit -m "feat: add BwItem deserialization and VaultServer conversion"
```

---

## Task 3: Server Merge Logic

**Files:**
- Modify: `src/bitwarden.rs` — add `ServerSource` enum and `merge_servers` function
- Test: `src/bitwarden.rs` tests module

**Step 1: Add `ServerSource` and `merge_servers`**

```rust
/// Tracks whether a server came from config.toml or the Bitwarden vault.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ServerSource {
    Local,
    Vault,
}

/// Merge vault servers into local server list. Local wins by name.
///
/// Returns:
/// 1. Merged server list (local first, then vault-only)
/// 2. Source map (server_name → ServerSource) for UI display
/// 3. Vault passwords (server_name → password) for fetch
pub fn merge_servers(
    local_servers: &[crate::config::Server],
    vault_servers: Vec<VaultServer>,
) -> (Vec<crate::config::Server>, HashMap<String, ServerSource>, HashMap<String, String>) {
    let mut merged = Vec::new();
    let mut sources = HashMap::new();
    let mut passwords = HashMap::new();

    let local_names: std::collections::HashSet<&str> =
        local_servers.iter().map(|s| s.name.as_str()).collect();

    // Local servers first — all tagged as Local
    for s in local_servers {
        sources.insert(s.name.clone(), ServerSource::Local);
        merged.push(s.clone());
    }

    // Vault servers that don't collide with local names
    for vs in vault_servers {
        if local_names.contains(vs.server.name.as_str()) {
            log::debug!(
                "Vault server '{}' overridden by local config entry",
                vs.server.name
            );
            continue;
        }
        sources.insert(vs.server.name.clone(), ServerSource::Vault);
        if let Some(pw) = vs.password {
            passwords.insert(vs.server.name.clone(), pw);
        }
        merged.push(vs.server);
    }

    (merged, sources, passwords)
}
```

**Step 2: Write merge tests**

Add to the tests module in `src/bitwarden.rs`:

```rust
#[test]
fn test_merge_local_wins() {
    let local = vec![crate::config::Server {
        name: "prod-node".to_string(),
        address: "local-addr".to_string(),
        target_cluster_ip: "local-ip".to_string(),
        user: None, file_path: None, file_name: None,
        context_name: None, identity_file: None,
    }];
    let vault = vec![VaultServer {
        server: crate::config::Server {
            name: "prod-node".to_string(),
            address: "vault-addr".to_string(),
            target_cluster_ip: "vault-ip".to_string(),
            user: None, file_path: None, file_name: None,
            context_name: None, identity_file: None,
        },
        password: Some("vault-pw".to_string()),
        vault_item_id: "uuid".to_string(),
    }];
    let (merged, sources, passwords) = merge_servers(&local, vault);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].address, "local-addr"); // local wins
    assert_eq!(sources[&"prod-node".to_string()], ServerSource::Local);
    assert!(!passwords.contains_key("prod-node")); // vault pw NOT used
}

#[test]
fn test_merge_vault_added() {
    let local = vec![crate::config::Server {
        name: "local-only".to_string(),
        address: "1.1.1.1".to_string(),
        target_cluster_ip: "1.1.1.1".to_string(),
        user: None, file_path: None, file_name: None,
        context_name: None, identity_file: None,
    }];
    let vault = vec![VaultServer {
        server: crate::config::Server {
            name: "vault-only".to_string(),
            address: "2.2.2.2".to_string(),
            target_cluster_ip: "2.2.2.2".to_string(),
            user: Some("admin".to_string()),
            file_path: None, file_name: None,
            context_name: None, identity_file: None,
        },
        password: Some("pw123".to_string()),
        vault_item_id: "uuid".to_string(),
    }];
    let (merged, sources, passwords) = merge_servers(&local, vault);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].name, "local-only");
    assert_eq!(merged[1].name, "vault-only");
    assert_eq!(sources[&"vault-only".to_string()], ServerSource::Vault);
    assert_eq!(passwords["vault-only"], "pw123");
}

#[test]
fn test_merge_empty_vault() {
    let local = vec![crate::config::Server {
        name: "s1".to_string(),
        address: "1.1.1.1".to_string(),
        target_cluster_ip: "1.1.1.1".to_string(),
        user: None, file_path: None, file_name: None,
        context_name: None, identity_file: None,
    }];
    let (merged, sources, passwords) = merge_servers(&local, vec![]);
    assert_eq!(merged.len(), 1);
    assert_eq!(sources[&"s1".to_string()], ServerSource::Local);
    assert!(passwords.is_empty());
}
```

**Step 3: Run tests**

Run: `cargo test bitwarden::tests -- --nocapture`
Expected: All 8 tests pass (5 from Task 2 + 3 new).

**Step 4: Commit**

```bash
git add src/bitwarden.rs
git commit -m "feat: add server merge logic (local overrides vault by name)"
```

---

## Task 4: `BwCli` Wrapper — Status, Run, Unlock, Headless Auth

**Files:**
- Modify: `src/bitwarden.rs` — add BwCli struct and methods
- Test: `src/bitwarden.rs` tests module (unit tests for parsing; CLI tests are integration-only)

**Step 1: Add `VaultStatus` and `BwCli`**

```rust
use std::process::Command;

#[derive(Debug, PartialEq)]
pub enum VaultStatus {
    Unauthenticated,
    Locked,
    Unlocked,
}

#[derive(Deserialize)]
struct BwStatusResponse {
    status: String,
}

pub struct BwCli {
    session: Option<String>,
    server_url: Option<String>,
}

impl BwCli {
    pub fn new() -> Self {
        BwCli {
            session: std::env::var("BW_SESSION").ok().filter(|s| !s.is_empty()),
            server_url: None,
        }
    }

    pub fn with_server_url(mut self, url: Option<&str>) -> Self {
        self.server_url = url.map(|s| s.to_string());
        self
    }

    pub fn is_available() -> bool {
        Command::new("bw")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    pub fn status(&self) -> Result<VaultStatus, String> {
        let output = self.run(&["status"])?;
        let resp: BwStatusResponse = serde_json::from_str(&output)
            .map_err(|e| format!("Failed to parse bw status: {}", e))?;
        match resp.status.as_str() {
            "unlocked" => Ok(VaultStatus::Unlocked),
            "locked" => Ok(VaultStatus::Locked),
            "unauthenticated" => Ok(VaultStatus::Unauthenticated),
            other => Err(format!("Unknown vault status: {}", other)),
        }
    }

    /// Unlock vault with master password (interactive/TUI path).
    pub fn unlock(&mut self, master_password: &str) -> Result<(), String> {
        let output = Command::new("bw")
            .args(["unlock", "--raw"])
            .env("BW_PASSWORD", master_password)
            .arg("--passwordenv")
            .arg("BW_PASSWORD")
            .env("BW_NOINTERACTION", "true")
            .output()
            .map_err(|e| format!("bw unlock failed to start: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(format!("bw unlock failed: {}", stderr));
        }

        self.session = Some(String::from_utf8_lossy(&output.stdout).trim().to_string());
        Ok(())
    }

    /// Headless login: API key env vars + password file (cron path).
    /// Expects BW_CLIENTID and BW_CLIENTSECRET in the environment.
    pub fn login_headless(&mut self, password_file: &std::path::Path) -> Result<(), String> {
        // Step 1: Login with API key (idempotent — succeeds if already logged in)
        let login_output = Command::new("bw")
            .args(["login", "--apikey", "--nointeraction"])
            .env("BW_NOINTERACTION", "true")
            .output()
            .map_err(|e| format!("bw login failed to start: {}", e))?;

        // Login may fail if already logged in — that's OK
        if !login_output.status.success() {
            let stderr = String::from_utf8_lossy(&login_output.stderr).trim().to_string();
            if !stderr.contains("already logged in") && !stderr.contains("You are already logged in") {
                return Err(format!("bw login --apikey failed: {}", stderr));
            }
        }

        // Step 2: Unlock with password file
        let unlock_output = Command::new("bw")
            .args(["unlock", "--raw", "--passwordfile"])
            .arg(password_file)
            .env("BW_NOINTERACTION", "true")
            .output()
            .map_err(|e| format!("bw unlock failed to start: {}", e))?;

        if !unlock_output.status.success() {
            let stderr = String::from_utf8_lossy(&unlock_output.stderr).trim().to_string();
            return Err(format!("bw unlock --passwordfile failed: {}", stderr));
        }

        self.session = Some(String::from_utf8_lossy(&unlock_output.stdout).trim().to_string());
        Ok(())
    }

    /// Auto-detect auth method: BW_SESSION → headless → Err.
    /// Interactive unlock is handled separately by the TUI.
    pub fn ensure_session(&mut self, password_file: Option<&std::path::Path>) -> Result<(), String> {
        // Already have a session (from env)?
        if self.session.is_some() {
            // Verify it's actually unlocked
            match self.status() {
                Ok(VaultStatus::Unlocked) => return Ok(()),
                Ok(VaultStatus::Locked) => { self.session = None; }
                Ok(VaultStatus::Unauthenticated) => { self.session = None; }
                Err(_) => { self.session = None; }
            }
        }

        // Try headless if API key env vars + password file are available
        let has_api_key = std::env::var("BW_CLIENTID").is_ok()
            && std::env::var("BW_CLIENTSECRET").is_ok();

        if has_api_key {
            if let Some(pf) = password_file {
                return self.login_headless(pf);
            }
        }

        // Check if already logged in but just locked
        match self.status() {
            Ok(VaultStatus::Locked) => {
                Err("Vault is locked — unlock required".to_string())
            }
            Ok(VaultStatus::Unauthenticated) => {
                Err("Not logged in to Bitwarden. Run `bw login` first, or set BW_CLIENTID + BW_CLIENTSECRET environment variables.".to_string())
            }
            Ok(VaultStatus::Unlocked) => {
                // Shouldn't reach here since session was None, but handle gracefully
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Fetch vault items matching prefix, optionally filtered by collection.
    /// Skips items that fail to parse (logs warning).
    pub fn fetch_servers(
        &self,
        prefix: &str,
        collection: Option<&str>,
    ) -> Result<Vec<VaultServer>, String> {
        let mut args = vec!["list", "items", "--search", prefix];
        let collection_owned;
        if let Some(c) = collection {
            collection_owned = c.to_string();
            args.extend(["--collectionid", &collection_owned]);
        }

        let output = self.run(&args)?;
        let items: Vec<BwItem> = serde_json::from_str(&output)
            .map_err(|e| format!("Failed to parse vault items: {}", e))?;

        let mut servers = Vec::new();
        for item in &items {
            match item.to_vault_server(prefix) {
                Ok(vs) => servers.push(vs),
                Err(e) => log::warn!("Skipping vault item '{}': {}", item.name, e),
            }
        }

        Ok(servers)
    }

    /// Run a bw command with session key via env var (not --session arg).
    fn run(&self, args: &[&str]) -> Result<String, String> {
        let mut cmd = Command::new("bw");
        cmd.args(args);
        cmd.env("BW_NOINTERACTION", "true");

        if let Some(ref session) = self.session {
            cmd.env("BW_SESSION", session);
        }

        let output = cmd.output()
            .map_err(|e| format!("bw command failed to start: {}", e))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if stderr.is_empty() {
                format!("bw {} exited with {}", args.first().unwrap_or(&""), output.status)
            } else {
                stderr
            })
        }
    }
}
```

**Step 2: Add password file permission check**

```rust
/// Check that a password file has restrictive permissions (0600).
/// Returns Ok(()) if permissions are safe, Err with warning message if not.
#[cfg(unix)]
pub fn check_password_file_permissions(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(path)
        .map_err(|e| format!("Cannot read password file '{}': {}", path.display(), e))?;
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(format!(
            "Password file '{}' has permissions {:04o} — should be 0600. Fix with: chmod 600 {}",
            path.display(), mode, path.display()
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn check_password_file_permissions(_path: &std::path::Path) -> Result<(), String> {
    Ok(()) // No permission check on non-Unix platforms
}
```

**Step 3: Write test for permission check**

```rust
#[test]
#[cfg(unix)]
fn test_password_file_permission_check() {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    tmp.write_all(b"password").unwrap();

    // Set to 0600 — should pass
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(check_password_file_permissions(tmp.path()).is_ok());

    // Set to 0644 — should fail
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
    let err = check_password_file_permissions(tmp.path()).unwrap_err();
    assert!(err.contains("0644"));
    assert!(err.contains("chmod 600"));
}

#[test]
fn test_bw_status_parsing() {
    // Test that BwStatusResponse deserializes correctly
    let json = r#"{"status": "locked"}"#;
    let resp: BwStatusResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.status, "locked");
}
```

**Step 4: Run tests**

Run: `cargo test bitwarden::tests -- --nocapture`
Expected: All tests pass. Note: `BwCli` methods that call `bw` are not unit-testable without the binary — they'll be tested in integration.

**Step 5: Commit**

```bash
git add src/bitwarden.rs
git commit -m "feat: add BwCli wrapper with status, unlock, headless auth, fetch"
```

---

## Task 5: Wire `process_server` — Accept Optional Vault Password

**Files:**
- Modify: `src/fetch.rs` — add `vault_password` param to `process_server` and `process_servers`
- Modify: `src/tui/mod.rs` — update `spawn_fetch` to pass vault password
- Modify: `src/tests.rs` — update `process_server` call sites
- Test: Existing tests should still pass with the new param

**Step 1: Add `vault_password` param to `process_server`**

In `src/fetch.rs`, change the signature:
```rust
pub(crate) fn process_server(
    server: &crate::config::Server,
    config: &crate::config::Config,
    dry_run: bool,
    force: bool,
    vault_password: Option<&str>,
) -> Result<ServerResult, anyhow::Error> {
```

Replace the credential lookup block (around line 48):
```rust
// Step 2: Look up credential
let password: Option<String> = if let Some(pw) = vault_password {
    Some(pw.to_string())
} else {
    match crate::credentials::get_credential(&server.name) {
        crate::credentials::CredentialResult::Found(pw) => Some(pw),
        crate::credentials::CredentialResult::NotFound => None,
        crate::credentials::CredentialResult::Unavailable(reason) => {
            log::warn!(
                "[{}] Keyring unavailable ({}). Skipping.",
                server.name, reason
            );
            return Ok(ServerResult::Skipped(SkipReason::KeyringUnavailable));
        }
    }
};
```

**Step 2: Update `process_servers` to accept vault passwords**

Change signature:
```rust
pub(crate) fn process_servers(
    config: &crate::config::Config,
    servers_to_process: &[String],
    dry_run: bool,
    vault_passwords: &std::collections::HashMap<String, String>,
) -> Result<(), anyhow::Error> {
```

Update the call inside the parallel iterator:
```rust
let result = process_server(
    server, config, dry_run, false,
    vault_passwords.get(&server.name).map(|s| s.as_str()),
);
```

**Step 3: Update all call sites**

In `src/main.rs` (None branch, ~line 195):
```rust
None => {
    fetch::process_servers(&config, &cli.servers, cli.dry_run, &std::collections::HashMap::new())?;
}
```

In `src/tui/mod.rs::spawn_fetch` — add `vault_password: Option<String>` param:
```rust
pub(crate) fn spawn_fetch(
    server: crate::config::Server,
    config: crate::config::Config,
    dry_run: bool,
    force: bool,
    vault_password: Option<String>,
    tx: mpsc::Sender<AppEvent>,
) {
    std::thread::spawn(move || {
        let result = crate::fetch::process_server(
            &server, &config, dry_run, force,
            vault_password.as_deref(),
        )
        .map(|_| ())
        .map_err(|e| friendly_error(&e));
        // ... rest unchanged
    });
}
```

Update `start_fetch` to pass vault password:
```rust
pub(crate) fn start_fetch(
    app: &mut AppState,
    server: crate::config::Server,
    tx: &mpsc::Sender<AppEvent>,
) {
    let name = server.name.clone();
    let vault_pw = app.vault_passwords.get(&name).cloned();
    app.pre_fetch_expiry.insert(name.clone(), app.cert_cache.get(&name).copied().flatten());
    app.in_progress.insert(name);
    spawn_fetch(server, app.config.clone(), app.dry_run, true, vault_pw, tx.clone());
}
```

Update all other `spawn_fetch` call sites to pass `None` where there's no vault password available (grep for `spawn_fetch(` in features/).

In `src/tests.rs` — update all `process_server` calls to add `None` as the last argument.

**Step 4: Run all tests**

Run: `cargo test`
Expected: All 35+ existing tests pass with the new `None` parameter.

**Step 5: Commit**

```bash
git add src/fetch.rs src/main.rs src/tui/mod.rs src/tests.rs
git commit -m "feat: add vault_password param to process_server for explicit credential delivery"
```

---

## Task 6: TUI State — Add Vault Fields to `AppState`

**Files:**
- Modify: `src/tui/app.rs` — add ServerSource (re-export), new View variant, new AppEvent, AppState fields

**Step 1: Add re-export and new types**

In `src/tui/app.rs`, add at the top:
```rust
pub use crate::bitwarden::ServerSource;
```

Add `View::BitwardenUnlock`:
```rust
// In the View enum, add:
BitwardenUnlock {
    error: Option<String>,
},
```

Add `AppEvent::BitwardenComplete`:
```rust
// In the AppEvent enum, add:
BitwardenComplete {
    result: Result<Vec<crate::bitwarden::VaultServer>, String>,
},
```

**Step 2: Add vault fields to `AppState`**

```rust
pub struct AppState {
    // ... existing fields ...
    pub server_sources: HashMap<String, ServerSource>,
    pub vault_passwords: HashMap<String, String>,
    pub bw_session: Option<String>,
}
```

Update `AppState::new()` to initialize the new fields:
```rust
server_sources: HashMap::new(),
vault_passwords: HashMap::new(),
bw_session: None,
```

**Step 3: Verify compilation**

Run: `cargo check`
Expected: Clean (no errors). Warnings about unused fields are OK at this stage.

**Step 4: Commit**

```bash
git add src/tui/app.rs
git commit -m "feat: add vault state fields to AppState (ServerSource, vault_passwords, bw_session)"
```

---

## Task 7: TUI Feature — Bitwarden Unlock View

**Files:**
- Create: `src/tui/features/bitwarden.rs` — render + handle_key for unlock prompt
- Modify: `src/tui/features/mod.rs` — add `pub mod bitwarden;`
- Modify: `src/tui/mod.rs` — dispatch new view in render_app and handle_key, handle BitwardenComplete event

**Step 1: Create `src/tui/features/bitwarden.rs`**

```rust
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::tui::app::{AppEvent, AppState, MaskedInput, View};
use super::centered_rect;

pub fn render(frame: &mut ratatui::Frame, app: &AppState) {
    let error = match &app.view {
        View::BitwardenUnlock { error } => error.clone(),
        _ => None,
    };

    let area = centered_rect(60, 12, frame.area());
    let block = Block::default()
        .title(" Bitwarden Vault Unlock ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::vertical([
        Constraint::Length(1), // status
        Constraint::Length(1), // blank
        Constraint::Length(1), // prompt label
        Constraint::Length(1), // password input
        Constraint::Length(1), // blank
        Constraint::Length(2), // error or hint
    ])
    .split(inner);

    let status_text = if app.in_progress.contains("__bitwarden__") {
        "Unlocking vault..."
    } else {
        "Enter your Bitwarden master password to unlock the vault."
    };
    frame.render_widget(
        Paragraph::new(status_text).style(Style::default().fg(Color::White)),
        rows[0],
    );

    frame.render_widget(
        Paragraph::new("Master Password:").style(Style::default().fg(Color::Gray)),
        rows[2],
    );

    let masked = app.credential_input.masked_display();
    let display = if masked.is_empty() { "·".repeat(0) } else { masked };
    frame.render_widget(
        Paragraph::new(display).style(Style::default().fg(Color::Yellow)),
        rows[3],
    );

    if let Some(err) = error {
        frame.render_widget(
            Paragraph::new(err)
                .style(Style::default().fg(Color::Red))
                .wrap(Wrap { trim: false }),
            rows[5],
        );
    } else {
        frame.render_widget(
            Paragraph::new("[Enter] Unlock  [Esc] Skip (local servers only)")
                .style(Style::default().fg(Color::DarkGray)),
            rows[5],
        );
    }
}

pub fn handle_key(
    app: &mut AppState,
    key: KeyEvent,
    tx: &std::sync::mpsc::Sender<AppEvent>,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            // Skip vault unlock — proceed with local servers only
            app.credential_input.clear();
            app.view = View::Dashboard;
            false
        }
        KeyCode::Enter => {
            if app.credential_input.value.is_empty() {
                return false;
            }
            // Spawn vault unlock + fetch on background thread
            let password = app.credential_input.value.clone();
            app.credential_input.clear();
            app.in_progress.insert("__bitwarden__".to_string());

            let bw_config = app.config.bitwarden.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                let result = do_bitwarden_unlock(&password, bw_config.as_ref());
                let _ = tx.send(AppEvent::BitwardenComplete { result });
            });
            false
        }
        KeyCode::Backspace => {
            app.credential_input.pop();
            false
        }
        KeyCode::Char(c) => {
            app.credential_input.push(c);
            false
        }
        _ => false,
    }
}

fn do_bitwarden_unlock(
    password: &str,
    bw_config: Option<&crate::bitwarden::BitwardenConfig>,
) -> Result<Vec<crate::bitwarden::VaultServer>, String> {
    let bw_config = bw_config.ok_or("Bitwarden not configured")?;
    let mut cli = crate::bitwarden::BwCli::new()
        .with_server_url(bw_config.server_url.as_deref());

    cli.unlock(password)?;

    let prefix = bw_config.item_prefix.as_deref().unwrap_or("k3s:");
    cli.fetch_servers(prefix, bw_config.collection.as_deref())
}

/// Called by the event loop when BitwardenComplete arrives.
pub fn on_complete(
    app: &mut AppState,
    result: Result<Vec<crate::bitwarden::VaultServer>, String>,
) {
    app.in_progress.remove("__bitwarden__");

    match result {
        Ok(vault_servers) => {
            let (merged, sources, passwords) = crate::bitwarden::merge_servers(
                &app.config.servers,
                vault_servers,
            );
            app.config.servers = merged;
            app.server_sources = sources;
            app.vault_passwords = passwords;
            app.refresh_cert_cache();
            if !app.config.servers.is_empty() {
                app.table_state.select(Some(0));
            }
            let vault_count = app.server_sources.values()
                .filter(|s| **s == crate::bitwarden::ServerSource::Vault)
                .count();
            app.notification = Some((
                format!("Vault unlocked — {} server(s) loaded", vault_count),
                std::time::Instant::now(),
            ));
            app.view = View::Dashboard;
        }
        Err(msg) => {
            app.view = View::BitwardenUnlock {
                error: Some(msg),
            };
        }
    }
}
```

**Step 2: Register module in `src/tui/features/mod.rs`**

Add:
```rust
pub mod bitwarden;
```

**Step 3: Wire into `src/tui/mod.rs`**

Add `View::BitwardenUnlock` to `render_app`:
```rust
// In the ViewKind enum, add:
BitwardenUnlock,

// In the match on app.view, add:
View::BitwardenUnlock { .. } => ViewKind::BitwardenUnlock,

// In the match on kind, add:
ViewKind::BitwardenUnlock => features::bitwarden::render(frame, app),
```

Add `View::BitwardenUnlock` to `handle_key`:
```rust
View::BitwardenUnlock { .. } => features::bitwarden::handle_key(app, key, tx),
```

Add `AppEvent::BitwardenComplete` to event_loop:
```rust
Ok(AppEvent::BitwardenComplete { result }) => {
    features::bitwarden::on_complete(app, result);
}
```

**Step 4: Verify compilation**

Run: `cargo check`
Expected: Clean.

**Step 5: Commit**

```bash
git add src/tui/features/bitwarden.rs src/tui/features/mod.rs src/tui/mod.rs
git commit -m "feat: add BitwardenUnlock TUI view with vault fetch on success"
```

---

## Task 8: TUI Startup — Bitwarden Detection and Auto-Unlock

**Files:**
- Modify: `src/tui/mod.rs` — add vault loading to `run_tui`

**Step 1: Add Bitwarden startup check to `run_tui`**

In `run_tui`, after `app.refresh_cert_cache()` and before the table state select, add:

```rust
// Bitwarden vault integration
if let Some(ref bw_config) = config.bitwarden {
    if bw_config.enabled {
        if !crate::bitwarden::BwCli::is_available() {
            app.view = View::Error {
                message: "Bitwarden CLI (bw) not found. Install: npm i -g @bitwarden/cli".to_string(),
            };
        } else {
            // Check password_file permissions if configured
            if let Some(ref pf) = bw_config.password_file {
                if let Err(warning) = crate::bitwarden::check_password_file_permissions(pf) {
                    log::warn!("{}", warning);
                }
            }

            // Try auto-session (BW_SESSION env or headless)
            let mut bw_cli = crate::bitwarden::BwCli::new()
                .with_server_url(bw_config.server_url.as_deref());

            match bw_cli.ensure_session(bw_config.password_file.as_deref()) {
                Ok(()) => {
                    // Session acquired — fetch vault servers
                    let prefix = bw_config.item_prefix.as_deref().unwrap_or("k3s:");
                    match bw_cli.fetch_servers(prefix, bw_config.collection.as_deref()) {
                        Ok(vault_servers) => {
                            let (merged, sources, passwords) =
                                crate::bitwarden::merge_servers(&app.config.servers, vault_servers);
                            app.config.servers = merged;
                            app.server_sources = sources;
                            app.vault_passwords = passwords;
                            app.refresh_cert_cache();
                        }
                        Err(e) => {
                            app.notification = Some((
                                format!("Vault fetch failed: {}", e),
                                std::time::Instant::now(),
                            ));
                        }
                    }
                }
                Err(e) => {
                    if e.contains("locked") || e.contains("Locked") {
                        app.view = View::BitwardenUnlock { error: None };
                    } else {
                        app.view = View::Error { message: e };
                    }
                }
            }
        }
    }
}
```

**Step 2: Verify compilation**

Run: `cargo check`
Expected: Clean.

**Step 3: Commit**

```bash
git add src/tui/mod.rs
git commit -m "feat: add Bitwarden startup detection and auto-unlock in TUI"
```

---

## Task 9: CLI/Cron Path — Vault Loading Before `process_servers`

**Files:**
- Modify: `src/main.rs` — add vault loading in the `None` command branch

**Step 1: Add vault loading to CLI path**

Replace the `None` branch in `main.rs`:

```rust
None => {
    // Load vault servers if bitwarden is enabled
    let vault_passwords = if let Some(ref bw_config) = config.bitwarden {
        if bw_config.enabled {
            if !bitwarden::BwCli::is_available() {
                anyhow::bail!(
                    "Bitwarden CLI (bw) not found but [bitwarden] is enabled in config. \
                     Install: npm i -g @bitwarden/cli"
                );
            }

            if let Some(ref pf) = bw_config.password_file {
                if let Err(warning) = bitwarden::check_password_file_permissions(pf) {
                    log::warn!("{}", warning);
                }
            }

            let mut bw_cli = bitwarden::BwCli::new()
                .with_server_url(bw_config.server_url.as_deref());

            bw_cli.ensure_session(bw_config.password_file.as_deref())
                .map_err(|e| anyhow::anyhow!("Bitwarden: {}", e))?;

            let prefix = bw_config.item_prefix.as_deref().unwrap_or("k3s:");
            let vault_servers = bw_cli.fetch_servers(prefix, bw_config.collection.as_deref())
                .map_err(|e| anyhow::anyhow!("Bitwarden fetch: {}", e))?;

            let (merged, _sources, passwords) =
                bitwarden::merge_servers(&config.servers, vault_servers);
            config.servers = merged;
            log::info!("Loaded {} vault servers", passwords.len());
            passwords
        } else {
            std::collections::HashMap::new()
        }
    } else {
        std::collections::HashMap::new()
    };

    fetch::process_servers(&config, &cli.servers, cli.dry_run, &vault_passwords)?;
}
```

Note: `config` must be `let mut config` for this to work — update the binding where config is loaded.

**Step 2: Verify compilation**

Run: `cargo check`
Expected: Clean.

**Step 3: Run tests**

Run: `cargo test`
Expected: All pass.

**Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: add vault server loading to CLI/cron path"
```

---

## Task 10: Dashboard — Source Badge and Vault Guards

**Files:**
- Modify: `src/tui/features/dashboard.rs` — source badge column, disable D/c for vault servers

**Step 1: Read `dashboard.rs` to identify table rendering location**

Read the file to find where the table columns are defined and where the `D` and `c` key handlers are.

**Step 2: Add source badge to table rendering**

In the table row rendering, add a "Source" indicator. Look for where rows are constructed and add:

```rust
let source = app.server_sources.get(&server.name)
    .copied()
    .unwrap_or(crate::bitwarden::ServerSource::Local);
let source_str = match source {
    crate::bitwarden::ServerSource::Local => "",
    crate::bitwarden::ServerSource::Vault => "[vault]",
};
```

Add this as a column in the table row.

For the credential column, modify to show "Vault" for vault-sourced servers:

```rust
let cred_display = if source == crate::bitwarden::ServerSource::Vault {
    "Vault".to_string()
} else {
    // existing credential check logic
};
```

**Step 3: Guard `D` key for vault servers**

In the `handle_key` function, where `D` triggers delete, add:

```rust
KeyCode::Char('D') => {
    if let Some(name) = selected_server_name(app) {
        let source = app.server_sources.get(&name)
            .copied()
            .unwrap_or(crate::bitwarden::ServerSource::Local);
        if source == crate::bitwarden::ServerSource::Vault {
            app.notification = Some((
                "Vault servers are managed in Bitwarden".to_string(),
                std::time::Instant::now(),
            ));
            return false;
        }
        app.view = View::DeleteConfirm(name);
    }
    false
}
```

**Step 4: Guard `c` key for vault servers**

Similar guard for the credential menu:

```rust
KeyCode::Char('c') => {
    if let Some(name) = selected_server_name(app) {
        let source = app.server_sources.get(&name)
            .copied()
            .unwrap_or(crate::bitwarden::ServerSource::Local);
        if source == crate::bitwarden::ServerSource::Vault {
            app.notification = Some((
                "Credentials managed by vault".to_string(),
                std::time::Instant::now(),
            ));
            return false;
        }
        app.view = View::CredentialMenu(name);
    }
    false
}
```

**Step 5: Verify compilation**

Run: `cargo check`
Expected: Clean.

**Step 6: Commit**

```bash
git add src/tui/features/dashboard.rs
git commit -m "feat: add vault source badge and disable D/c for vault servers"
```

---

## Task 11: Detail View — Show Source and Use Vault Password

**Files:**
- Modify: `src/tui/features/detail.rs` — source line, vault password in fetch, guard c key

**Step 1: Read `detail.rs` to identify rendering and key handling locations**

Read the file to find where server details are rendered and where `f` and `c` keys are handled.

**Step 2: Add source line to detail rendering**

In the detail render function, add a line showing the server source:

```rust
let source = app.server_sources.get(&server_name)
    .copied()
    .unwrap_or(crate::bitwarden::ServerSource::Local);
let source_str = match source {
    crate::bitwarden::ServerSource::Local => "Local (config.toml)",
    crate::bitwarden::ServerSource::Vault => "Vault (Bitwarden)",
};
// Render as a row in the detail view
```

**Step 3: Guard `c` key for vault servers**

Add the same vault guard as in dashboard.

**Step 4: Verify compilation**

Run: `cargo check`
Expected: Clean.

**Step 5: Commit**

```bash
git add src/tui/features/detail.rs
git commit -m "feat: show server source in detail view, guard credentials for vault servers"
```

---

## Task 12: Error Handling — Add Bitwarden-Specific `friendly_error` Patterns

**Files:**
- Modify: `src/tui/mod.rs` — add bw-specific patterns to `friendly_error`

**Step 1: Add new patterns**

In `friendly_error`, add before the fallback return:

```rust
if lower.contains("bw") && (lower.contains("not found") || lower.contains("no such file")) {
    return "Bitwarden CLI (bw) not installed. Run: npm i -g @bitwarden/cli".to_string();
}
if lower.contains("vault") && lower.contains("locked") {
    return "Vault is locked — unlock with your master password.".to_string();
}
if lower.contains("invalid master password") || lower.contains("invalid password") {
    return "Wrong master password. Try again.".to_string();
}
```

**Step 2: Verify compilation**

Run: `cargo check`
Expected: Clean.

**Step 3: Commit**

```bash
git add src/tui/mod.rs
git commit -m "feat: add Bitwarden-specific friendly error messages"
```

---

## Task 13: Final Integration Test and Cleanup

**Files:**
- All modified files
- Run full test suite

**Step 1: Run full test suite**

Run: `cargo test`
Expected: All tests pass.

**Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings.

**Step 3: Run formatter**

Run: `cargo fmt --check`
Expected: Clean.

**Step 4: Build release**

Run: `cargo build --release`
Expected: Clean build.

**Step 5: Manual smoke test (if bw is installed)**

```bash
# Without bitwarden config — should work exactly as before
cargo run -- --dry-run

# TUI without bitwarden — should work exactly as before
cargo run -- tui
```

**Step 6: Final commit if any cleanup needed**

```bash
git add -A
git commit -m "chore: final cleanup for Bitwarden integration"
```

---

## Summary

| Task | Description | New/Modified Files |
|------|-------------|-------------------|
| 1 | Config extension | bitwarden.rs (create), config.rs, main.rs |
| 2 | Vault item parsing | bitwarden.rs |
| 3 | Server merge logic | bitwarden.rs |
| 4 | BwCli wrapper | bitwarden.rs |
| 5 | Wire process_server | fetch.rs, tui/mod.rs, tests.rs |
| 6 | TUI state fields | tui/app.rs |
| 7 | Unlock TUI view | tui/features/bitwarden.rs (create), features/mod.rs, tui/mod.rs |
| 8 | TUI startup detection | tui/mod.rs |
| 9 | CLI/cron vault loading | main.rs |
| 10 | Dashboard badges + guards | tui/features/dashboard.rs |
| 11 | Detail view source | tui/features/detail.rs |
| 12 | Friendly errors | tui/mod.rs |
| 13 | Integration test + cleanup | All |

Tasks 1-4 are pure additions (no existing code changes). Task 5 is the signature change that touches existing code. Tasks 6-12 are TUI integration. Task 13 is verification.
