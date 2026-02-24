#[cfg(not(target_os = "macos"))]
use keyring::{Entry, Error as KeyringError};

#[cfg(not(target_os = "macos"))]
use base64::{engine::general_purpose, Engine as _};

#[cfg(not(target_os = "macos"))]
use std::collections::HashMap;

pub const SERVICE: &str = "kube_config_updater";
pub const DEFAULT_ACCOUNT: &str = "_default";

/// Result of a credential lookup.
///
/// Does NOT derive Debug to prevent passwords from appearing in logs or
/// debug output. A manual Debug impl is provided that redacts the password.
pub enum CredentialResult {
    Found(String),
    NotFound,
    Unavailable(String),
}

impl std::fmt::Debug for CredentialResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CredentialResult::Found(_) => write!(f, "CredentialResult::Found(<redacted>)"),
            CredentialResult::NotFound => write!(f, "CredentialResult::NotFound"),
            CredentialResult::Unavailable(msg) => {
                write!(f, "CredentialResult::Unavailable({msg})")
            }
        }
    }
}

/// Abstraction over the OS keyring, primarily to allow mock injection in tests.
pub trait KeyringBackend {
    fn get(&self, service: &str, account: &str) -> CredentialResult;
    fn set(&self, service: &str, account: &str, password: &str) -> Result<(), String>;
    fn delete(&self, service: &str, account: &str) -> Result<(), String>;
}

/// Production implementation backed by the OS keyring.
///
/// On macOS this uses the `security` CLI tool so that stored credentials are
/// not bound to a specific binary's code signature and survive app updates.
/// On other platforms it uses the `keyring` crate (D-Bus Secret Service on Linux).
pub struct RealKeyring;

/// macOS backend: wraps `/usr/bin/security` to read/write generic passwords in
/// the user's login Keychain without application-specific ACLs.
#[cfg(target_os = "macos")]
mod macos_keychain {
    use std::process::Command;

    pub fn get(service: &str, account: &str) -> Result<Option<String>, String> {
        let output = Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", service, "-a", account, "-w"])
            .output()
            .map_err(|e| format!("security command failed: {}", e))?;

        if output.status.success() {
            let password = String::from_utf8_lossy(&output.stdout)
                .trim_end_matches('\n')
                .to_string();
            Ok(Some(password))
        } else if output.status.code() == Some(44) {
            // errSecItemNotFound — no entry exists yet
            Ok(None)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if stderr.is_empty() {
                format!("security exited with {}", output.status)
            } else {
                stderr
            })
        }
    }

    pub fn set(service: &str, account: &str, password: &str) -> Result<(), String> {
        let status = Command::new("/usr/bin/security")
            .args([
                "add-generic-password",
                "-U", // update existing entry if present
                "-s", service,
                "-a", account,
                "-w", password,
            ])
            .status()
            .map_err(|e| format!("security command failed: {}", e))?;

        if status.success() {
            Ok(())
        } else {
            Err(format!("security add-generic-password exited with {}", status))
        }
    }

    pub fn delete(service: &str, account: &str) -> Result<(), String> {
        let status = Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-s", service, "-a", account])
            .status()
            .map_err(|e| format!("security command failed: {}", e))?;

        if status.success() || status.code() == Some(44) {
            // exit 44 = item not found; treat as success (idempotent)
            Ok(())
        } else {
            Err(format!(
                "security delete-generic-password exited with {}",
                status
            ))
        }
    }
}

#[cfg(target_os = "macos")]
impl KeyringBackend for RealKeyring {
    fn get(&self, service: &str, account: &str) -> CredentialResult {
        match macos_keychain::get(service, account) {
            Ok(Some(password)) => CredentialResult::Found(password),
            Ok(None) => CredentialResult::NotFound,
            Err(e) => CredentialResult::Unavailable(e),
        }
    }

    fn set(&self, service: &str, account: &str, password: &str) -> Result<(), String> {
        macos_keychain::set(service, account, password)
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), String> {
        macos_keychain::delete(service, account)
    }
}

#[cfg(not(target_os = "macos"))]
impl KeyringBackend for RealKeyring {
    fn get(&self, service: &str, account: &str) -> CredentialResult {
        match Entry::new(service, account) {
            Err(e) => CredentialResult::Unavailable(e.to_string()),
            Ok(entry) => match entry.get_password() {
                Ok(password) => CredentialResult::Found(password),
                Err(KeyringError::NoEntry) => CredentialResult::NotFound,
                Err(e) => CredentialResult::Unavailable(e.to_string()),
            },
        }
    }

    fn set(&self, service: &str, account: &str, password: &str) -> Result<(), String> {
        let entry = Entry::new(service, account).map_err(|e| e.to_string())?;
        entry.set_password(password).map_err(|e| e.to_string())
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), String> {
        let entry = Entry::new(service, account).map_err(|e| e.to_string())?;
        entry.delete_credential().map_err(|e| e.to_string())
    }
}

// ─── File-based fallback keyring (Linux / non-macOS) ──────────────────────────
//
// Used when the D-Bus Secret Service daemon is not available.
// Passwords are stored in a plain-text file with 0600 permissions (owner-read-only).
// This is the same security model as ~/.kube/config and ~/.ssh/id_rsa.
//
// File location: ~/.config/kube_config_updater/credentials
// Format: one entry per line, tab-separated: account_name <TAB> base64(password)
// Lines starting with '#' are comments.

#[cfg(not(target_os = "macos"))]
pub struct FileKeyring {
    path: std::path::PathBuf,
}

#[cfg(not(target_os = "macos"))]
impl FileKeyring {
    pub fn default_path() -> std::path::PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_default()
                    .join(".config")
            })
            .join("kube_config_updater")
            .join("credentials")
    }

    fn load(&self) -> HashMap<String, String> {
        let content = match std::fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(_) => return HashMap::new(),
        };
        let mut map = HashMap::new();
        for line in content.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            if let Some((account, b64)) = line.split_once('\t') {
                if let Ok(pw_bytes) = general_purpose::STANDARD.decode(b64.trim()) {
                    if let Ok(pw) = String::from_utf8(pw_bytes) {
                        map.insert(account.to_string(), pw);
                    }
                }
            }
        }
        map
    }

    fn save(&self, store: &HashMap<String, String>) -> Result<(), String> {
        use std::io::Write;

        let parent = self.path.parent().ok_or("invalid credentials path")?;
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create credentials directory: {}", e))?;

        // Restrict directory to owner-only before writing
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| format!("could not set directory permissions: {}", e))?;
        }

        let mut content = String::from(
            "# kube_config_updater credentials\n\
             # Stored with restricted permissions (0600) — only you can read this file.\n\
             # Same security model as ~/.kube/config and ~/.ssh/id_rsa.\n",
        );
        for (account, password) in store {
            let b64 = general_purpose::STANDARD.encode(password.as_bytes());
            content.push_str(&format!("{}\t{}\n", account, b64));
        }

        // Write to a temp file first, then rename atomically
        let tmp = self.path.with_extension("tmp");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|e| format!("could not write credentials file: {}", e))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("could not set file permissions: {}", e))?;
        }

        file.write_all(content.as_bytes())
            .map_err(|e| format!("could not write credentials: {}", e))?;
        drop(file);

        std::fs::rename(&tmp, &self.path)
            .map_err(|e| format!("could not finalize credentials file: {}", e))?;

        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
impl KeyringBackend for FileKeyring {
    fn get(&self, _service: &str, account: &str) -> CredentialResult {
        let store = self.load();
        match store.get(account) {
            Some(pw) => CredentialResult::Found(pw.clone()),
            None => CredentialResult::NotFound,
        }
    }

    fn set(&self, _service: &str, account: &str, password: &str) -> Result<(), String> {
        let mut store = self.load();
        store.insert(account.to_string(), password.to_string());
        self.save(&store)
    }

    fn delete(&self, _service: &str, account: &str) -> Result<(), String> {
        let mut store = self.load();
        store.remove(account);
        self.save(&store)
    }
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Look up a credential for the given server name.
///
/// On non-macOS: tries the system keyring first; if unavailable (no D-Bus daemon),
/// falls back to the file store (transparent read — the user already consented when
/// they stored the credential via `set_credential_file`).
///
/// Falls back to the DEFAULT_ACCOUNT entry when no server-specific entry exists.
/// Passwords are never written to any log call.
pub fn get_credential(server_name: &str) -> CredentialResult {
    #[cfg(not(target_os = "macos"))]
    {
        let primary = get_credential_with(server_name, &RealKeyring);
        if matches!(primary, CredentialResult::Unavailable(_)) {
            let file = FileKeyring { path: FileKeyring::default_path() };
            return get_credential_with(server_name, &file);
        }
        return primary;
    }
    #[cfg(target_os = "macos")]
    get_credential_with(server_name, &RealKeyring)
}

pub fn get_credential_with(server_name: &str, backend: &dyn KeyringBackend) -> CredentialResult {
    match backend.get(SERVICE, server_name) {
        CredentialResult::NotFound => match backend.get(SERVICE, DEFAULT_ACCOUNT) {
            CredentialResult::Found(pw) => CredentialResult::Found(pw),
            _ => CredentialResult::NotFound,
        },
        other => other,
    }
}

/// Store a credential for the given server name using the primary keyring backend.
/// On Linux this requires a running D-Bus Secret Service daemon.
/// If unavailable, the caller should present a consent dialog and then call
/// `set_credential_file` instead.
pub fn set_credential(server_name: &str, password: &str) -> Result<(), String> {
    set_credential_with(server_name, password, &RealKeyring)
}

pub fn set_credential_with(
    server_name: &str,
    password: &str,
    backend: &dyn KeyringBackend,
) -> Result<(), String> {
    backend.set(SERVICE, server_name, password)
}

/// Store a credential in the file-based fallback store with 0600 permissions.
///
/// Only call this after the user has explicitly consented to file-based storage
/// (i.e., accepted the `KeyringFallbackConsent` dialog).
///
/// On macOS the security CLI is always available, so this path is never reached;
/// it is provided here only to keep the call-site cross-platform.
pub fn set_credential_file(server_name: &str, password: &str) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    {
        let file = FileKeyring { path: FileKeyring::default_path() };
        return set_credential_with(server_name, password, &file);
    }
    #[cfg(target_os = "macos")]
    set_credential_with(server_name, password, &RealKeyring)
}

/// Remove the credential for the given server name from the OS keyring.
/// On non-macOS, also removes from the file store (in case the credential was
/// stored there as a fallback).
pub fn delete_credential(server_name: &str) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    {
        // Try keyring first (may not be available — ignore that specific error)
        let _ = delete_credential_with(server_name, &RealKeyring);
        // Always also attempt to remove from file store (idempotent)
        let file = FileKeyring { path: FileKeyring::default_path() };
        return delete_credential_with(server_name, &file);
    }
    #[cfg(target_os = "macos")]
    delete_credential_with(server_name, &RealKeyring)
}

pub fn delete_credential_with(
    server_name: &str,
    backend: &dyn KeyringBackend,
) -> Result<(), String> {
    backend.delete(SERVICE, server_name)
}

/// Check whether credentials are available for a list of server names.
///
/// Returns a map of server name to credential availability status.
pub fn check_credentials<'a>(server_names: &'a [&'a str]) -> Vec<(&'a str, CredentialResult)> {
    check_credentials_with(server_names, &RealKeyring)
}

pub fn check_credentials_with<'a>(
    server_names: &[&'a str],
    backend: &dyn KeyringBackend,
) -> Vec<(&'a str, CredentialResult)> {
    server_names
        .iter()
        .map(|&name| {
            let result = get_credential_with(name, backend);
            (name, result)
        })
        .collect()
}

/// Returns true if the keyring error indicates the secret service daemon is not
/// available (rather than a transient or permission error). Used to decide whether
/// to offer the file-based fallback consent dialog.
pub fn keyring_error_is_unavailable(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("platform secure storage")
        || lower.contains("dbus")
        || lower.contains("org.freedesktop.secrets")
        || lower.contains("no storage access")
        || lower.contains("secret service")
}

/// Returns the path to the file-based credential store (for display in UI messages).
pub fn credential_file_path() -> String {
    #[cfg(not(target_os = "macos"))]
    {
        FileKeyring::default_path().to_string_lossy().into_owned()
    }
    #[cfg(target_os = "macos")]
    {
        "macOS Keychain (via security CLI)".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct MockKeyring {
        store: Mutex<HashMap<(String, String), String>>,
    }

    impl MockKeyring {
        fn new() -> Self {
            MockKeyring {
                store: Mutex::new(HashMap::new()),
            }
        }
    }

    impl KeyringBackend for MockKeyring {
        fn get(&self, service: &str, account: &str) -> CredentialResult {
            let store = self.store.lock().unwrap();
            match store.get(&(service.to_string(), account.to_string())) {
                Some(pw) => CredentialResult::Found(pw.clone()),
                None => CredentialResult::NotFound,
            }
        }

        fn set(&self, service: &str, account: &str, password: &str) -> Result<(), String> {
            let mut store = self.store.lock().unwrap();
            store.insert(
                (service.to_string(), account.to_string()),
                password.to_string(),
            );
            Ok(())
        }

        fn delete(&self, service: &str, account: &str) -> Result<(), String> {
            let mut store = self.store.lock().unwrap();
            store.remove(&(service.to_string(), account.to_string()));
            Ok(())
        }
    }

    #[test]
    fn test_get_credential_found() {
        let mock = MockKeyring::new();
        mock.set(SERVICE, "my-server", "secret").unwrap();
        let result = get_credential_with("my-server", &mock);
        assert!(matches!(result, CredentialResult::Found(pw) if pw == "secret"));
    }

    #[test]
    fn test_get_credential_falls_back_to_default() {
        let mock = MockKeyring::new();
        mock.set(SERVICE, DEFAULT_ACCOUNT, "default-secret").unwrap();
        let result = get_credential_with("unknown-server", &mock);
        assert!(matches!(result, CredentialResult::Found(pw) if pw == "default-secret"));
    }

    #[test]
    fn test_get_credential_not_found() {
        let mock = MockKeyring::new();
        let result = get_credential_with("no-such-server", &mock);
        assert!(matches!(result, CredentialResult::NotFound));
    }

    #[test]
    fn test_set_and_delete_credential() {
        let mock = MockKeyring::new();
        set_credential_with("srv", "pw", &mock).unwrap();
        assert!(matches!(
            get_credential_with("srv", &mock),
            CredentialResult::Found(_)
        ));
        delete_credential_with("srv", &mock).unwrap();
        assert!(matches!(
            get_credential_with("srv", &mock),
            CredentialResult::NotFound
        ));
    }

    #[test]
    fn test_check_credentials() {
        let mock = MockKeyring::new();
        mock.set(SERVICE, "s1", "pw1").unwrap();
        let results = check_credentials_with(&["s1", "s2"], &mock);
        assert_eq!(results.len(), 2);
        assert!(matches!(&results[0].1, CredentialResult::Found(_)));
        assert!(matches!(&results[1].1, CredentialResult::NotFound));
    }

    #[test]
    fn test_debug_redacts_password() {
        let found = CredentialResult::Found("super-secret".to_string());
        let debug_str = format!("{found:?}");
        assert!(!debug_str.contains("super-secret"));
        assert!(debug_str.contains("redacted"));
    }

    #[test]
    fn test_keyring_error_is_unavailable_detects_dbus() {
        assert!(keyring_error_is_unavailable(
            "Platform secure storage failure: DBus error: Failed to execute program org.freedesktop.secrets: No such file or directory"
        ));
        assert!(keyring_error_is_unavailable("dbus connection failed"));
        assert!(!keyring_error_is_unavailable("wrong password"));
        assert!(!keyring_error_is_unavailable("authentication failed"));
    }
}
