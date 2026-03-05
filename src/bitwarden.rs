use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

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

/// A server + credential extracted from a Bitwarden vault item.
#[derive(Debug)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub field_type: u8,
}

impl BwItem {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .as_ref()?
            .iter()
            .find(|f| f.name == name)
            .and_then(|f| f.value.as_deref())
    }

    pub(crate) fn to_vault_server(&self, prefix: &str) -> Result<VaultServer, String> {
        let server_name = self.name.strip_prefix(prefix).unwrap_or(&self.name);

        let address = self
            .field("address")
            .ok_or_else(|| format!("vault item '{}' missing 'address' field", self.name))?;
        let target_ip = self
            .field("target_cluster_ip")
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
) -> (
    Vec<crate::config::Server>,
    HashMap<String, ServerSource>,
    HashMap<String, String>,
) {
    let mut merged = Vec::new();
    let mut sources = HashMap::new();
    let mut passwords = HashMap::new();

    let local_names: std::collections::HashSet<&str> = local_servers.iter().map(|s| s.name.as_str()).collect();

    // Local servers first — all tagged as Local
    for s in local_servers {
        sources.insert(s.name.clone(), ServerSource::Local);
        merged.push(s.clone());
    }

    // Vault servers that don't collide with local names
    for vs in vault_servers {
        if local_names.contains(vs.server.name.as_str()) {
            log::debug!("Vault server '{}' overridden by local config entry", vs.server.name);
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
        let resp: BwStatusResponse =
            serde_json::from_str(&output).map_err(|e| format!("Failed to parse bw status: {}", e))?;
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
            .args(["unlock", "--raw", "--passwordenv", "BW_PASSWORD"])
            .env("BW_PASSWORD", master_password)
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
                Ok(VaultStatus::Locked) => {
                    self.session = None;
                }
                Ok(VaultStatus::Unauthenticated) => {
                    self.session = None;
                }
                Err(_) => {
                    self.session = None;
                }
            }
        }

        // Try headless if API key env vars + password file are available
        let has_api_key = std::env::var("BW_CLIENTID").is_ok() && std::env::var("BW_CLIENTSECRET").is_ok();

        if has_api_key && let Some(pf) = password_file {
            return self.login_headless(pf);
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
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Fetch vault items matching prefix, optionally filtered by collection.
    /// Skips items that fail to parse (logs warning).
    pub fn fetch_servers(&self, prefix: &str, collection: Option<&str>) -> Result<Vec<VaultServer>, String> {
        let mut args = vec!["list", "items", "--search", prefix];
        let collection_owned;
        if let Some(c) = collection {
            collection_owned = c.to_string();
            args.extend(["--collectionid", &collection_owned]);
        }

        let output = self.run(&args)?;
        let items: Vec<BwItem> =
            serde_json::from_str(&output).map_err(|e| format!("Failed to parse vault items: {}", e))?;

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

        let output = cmd.output().map_err(|e| format!("bw command failed to start: {}", e))?;

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

/// Check that a password file has restrictive permissions (0600).
/// Returns Ok(()) if permissions are safe, Err with warning message if not.
#[cfg(unix)]
pub fn check_password_file_permissions(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let metadata =
        std::fs::metadata(path).map_err(|e| format!("Cannot read password file '{}': {}", path.display(), e))?;
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(format!(
            "Password file '{}' has permissions {:04o} — should be 0600. Fix with: chmod 600 {}",
            path.display(),
            mode,
            path.display()
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn check_password_file_permissions(_path: &std::path::Path) -> Result<(), String> {
    Ok(()) // No permission check on non-Unix platforms
}

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

    #[test]
    fn test_merge_local_wins() {
        let local = vec![crate::config::Server {
            name: "prod-node".to_string(),
            address: "local-addr".to_string(),
            target_cluster_ip: "local-ip".to_string(),
            user: None,
            file_path: None,
            file_name: None,
            context_name: None,
            identity_file: None,
        }];
        let vault = vec![VaultServer {
            server: crate::config::Server {
                name: "prod-node".to_string(),
                address: "vault-addr".to_string(),
                target_cluster_ip: "vault-ip".to_string(),
                user: None,
                file_path: None,
                file_name: None,
                context_name: None,
                identity_file: None,
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
            user: None,
            file_path: None,
            file_name: None,
            context_name: None,
            identity_file: None,
        }];
        let vault = vec![VaultServer {
            server: crate::config::Server {
                name: "vault-only".to_string(),
                address: "2.2.2.2".to_string(),
                target_cluster_ip: "2.2.2.2".to_string(),
                user: Some("admin".to_string()),
                file_path: None,
                file_name: None,
                context_name: None,
                identity_file: None,
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
            user: None,
            file_path: None,
            file_name: None,
            context_name: None,
            identity_file: None,
        }];
        let (merged, sources, passwords) = merge_servers(&local, vec![]);
        assert_eq!(merged.len(), 1);
        assert_eq!(sources[&"s1".to_string()], ServerSource::Local);
        assert!(passwords.is_empty());
    }

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
}
