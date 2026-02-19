use keyring::{Entry, Error as KeyringError};

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

/// Production implementation backed by the OS keyring via the `keyring` crate.
pub struct RealKeyring;

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

/// Look up a credential for the given server name.
///
/// Falls back to the DEFAULT_ACCOUNT entry when no server-specific entry
/// exists. Passwords are never written to any log call.
pub fn get_credential(server_name: &str) -> CredentialResult {
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

/// Store a credential for the given server name in the OS keyring.
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

/// Remove the credential for the given server name from the OS keyring.
pub fn delete_credential(server_name: &str) -> Result<(), String> {
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
}
