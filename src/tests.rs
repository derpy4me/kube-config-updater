use super::config::{load_config, Config, Server};
use super::kube::{merge_into_main_kubeconfig, process_kubeconfig_file, KubeConfig};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use tempfile::{Builder, NamedTempFile, TempDir};

/// Serialises access to ~/.kube/config across all tests that read or write it,
/// preventing the parallel test runner from corrupting the file mid-test.
static KUBE_CONFIG_LOCK: Mutex<()> = Mutex::new(());

fn create_test_config(content: &str) -> NamedTempFile {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(content.as_bytes()).unwrap();
    file
}

fn setup_test_kubeconfig(dir: &TempDir, content: &str) -> std::path::PathBuf {
    let path = dir.path().join("test_kubeconfig");
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(content.as_bytes()).unwrap();
    path
}

const TEST_KUBECONFIG_CONTENT: &str = r#"
apiVersion: v1
kind: Config
current-context: old-context
clusters:
- name: old-cluster
  cluster:
    server: https://1.2.3.4:6443
    certificate-authority-data: FAKECERT
contexts:
- name: old-context
  context:
    cluster: old-cluster
    user: old-user
users:
- name: old-user
  user:
    client-key-data: FAKEKEY
    client-certificate-data: aGVsbG8gd29ybGQ=
"#;

#[test]
fn test_load_valid_config() {
    let config_content = r#"
        local_output_dir = "/tmp/kube_configs"

        [[server]]
        name = "server1"
        address = "1.1.1.1"
        target_cluster_ip = "10.0.0.1"
    "#;
    let config_file = create_test_config(config_content);
    let config = load_config(config_file.path().to_str().unwrap()).unwrap();

    assert_eq!(config.local_output_dir, "/tmp/kube_configs");
    assert_eq!(config.servers.len(), 1);
    assert_eq!(config.servers[0].name, "server1");
}

#[test]
fn test_load_non_existent_config() {
    let result = load_config("/tmp/non_existent_config.toml");
    assert!(result.is_err());
}

#[test]
fn test_server_user_fallback() {
    let config = Config {
        default_user: Some("default_user".to_string()),
        default_file_path: None,
        default_file_name: None,
        default_identity_file: None,
        local_output_dir: "".to_string(),
        servers: vec![
            Server {
                name: "server1".to_string(),
                address: "".to_string(),
                target_cluster_ip: "".to_string(),
                user: None, // Should use default
                file_path: None,
                file_name: None,
                context_name: None,
                identity_file: None,
            },
            Server {
                name: "server2".to_string(),
                address: "".to_string(),
                target_cluster_ip: "".to_string(),
                user: Some("server_user".to_string()), // Should use its own
                file_path: None,
                file_name: None,
                context_name: None,
                identity_file: None,
            },
        ],
    };

    assert_eq!(config.servers[0].user(&config).unwrap(), "default_user");
    assert_eq!(config.servers[1].user(&config).unwrap(), "server_user");
}

#[test]
fn test_server_identity_file_fallback() {
    let config = Config {
        default_user: None,
        default_file_path: None,
        default_file_name: None,
        default_identity_file: Some("default_key".to_string()),
        local_output_dir: "".to_string(),
        servers: vec![
            Server {
                name: "server1".to_string(),
                address: "".to_string(),
                target_cluster_ip: "".to_string(),
                user: None,
                file_path: None,
                file_name: None,
                context_name: None,
                identity_file: None, // Should use default
            },
            Server {
                name: "server2".to_string(),
                address: "".to_string(),
                target_cluster_ip: "".to_string(),
                user: None,
                file_path: None,
                file_name: None,
                context_name: None,
                identity_file: Some("server_key".to_string()), // Should use its own
            },
        ],
    };

    assert_eq!(config.servers[0].identity_file(&config).unwrap(), "default_key");
    assert_eq!(config.servers[1].identity_file(&config).unwrap(), "server_key");
}

#[test]
fn test_server_file_path_fallback() {
    let config = Config {
        default_user: None,
        default_file_path: Some("/default/path".to_string()),
        default_file_name: Some("default_name".to_string()),
        default_identity_file: None,
        local_output_dir: "".to_string(),
        servers: vec![
            Server {
                name: "server1".to_string(),
                address: "".to_string(),
                target_cluster_ip: "".to_string(),
                user: None,
                file_path: None, // Should use default
                file_name: None, // Should use default
                context_name: None,
                identity_file: None,
            },
            Server {
                name: "server2".to_string(),
                address: "".to_string(),
                target_cluster_ip: "".to_string(),
                user: None,
                file_path: Some("/server/path".to_string()), // Should use its own
                file_name: Some("server_name".to_string()), // Should use its own
                context_name: None,
                identity_file: None,
            },
        ],
    };

    assert_eq!(
        config.servers[0].file_path(&config).unwrap(),
        "/default/path/default_name"
    );
    assert_eq!(
        config.servers[1].file_path(&config).unwrap(),
        "/server/path/server_name"
    );
}

#[test]
fn test_load_malformed_config() {
    let config_content = r#"
        local_output_dir = "/tmp/kube_configs"

        [[server]]
        name = "server1"
        address = "1.1.1.1"
        target_cluster_ip = "10.0.0.1"
    "#;
    let config_file = create_test_config(config_content);
    let result = load_config(config_file.path().to_str().unwrap());
    assert!(result.is_ok());

    let malformed_config_content = r#"
        local_output_dir = "/tmp/kube_configs"

        [[server]]
        name = "server1"
        address = "1.1.1.1"
        target_cluster_ip = 10.0.0.1" # Invalid TOML, string not closed
    "#;
    let malformed_config_file = create_test_config(malformed_config_content);
    let malformed_result = load_config(malformed_config_file.path().to_str().unwrap());
    assert!(malformed_result.is_err());
}

#[test]
fn test_process_kubeconfig_file_updates_content() {
    let temp_dir = Builder::new().prefix("test_kube").tempdir().unwrap();
    let kubeconfig_path = setup_test_kubeconfig(&temp_dir, TEST_KUBECONFIG_CONTENT);

    let target_ip = "9.9.9.9";
    let source_hash = "test_hash_123";
    let target_context = Some("new-context-name".to_string());

    process_kubeconfig_file(
        &kubeconfig_path,
        target_ip,
        source_hash,
        &target_context,
        "test-server",
        false,
    )
    .unwrap();

    let updated_content = fs::read_to_string(kubeconfig_path).unwrap();
    let updated_kubeconfig: super::kube::KubeConfig = serde_yaml::from_str(&updated_content).unwrap();

    // Check cluster server IP
    assert_eq!(
        updated_kubeconfig.clusters[0].cluster.server,
        "https://9.9.9.9:6443"
    );

    // Check context name and current-context
    assert_eq!(updated_kubeconfig.contexts[0].name, "new-context-name");
    assert_eq!(updated_kubeconfig.current_context, "new-context-name");

    // Check metadata
    let prefs = updated_kubeconfig.preferences.unwrap();
    assert_eq!(
        prefs.get("source-file-sha256").unwrap().as_str().unwrap(),
        source_hash
    );
    assert!(prefs.contains_key("script-last-updated"));
    assert!(!prefs.contains_key("certificate-expires-at"));
}

#[test]
fn test_process_kubeconfig_file_dry_run() {
    let temp_dir = Builder::new().prefix("test_kube_dry_run").tempdir().unwrap();
    let kubeconfig_path = setup_test_kubeconfig(&temp_dir, TEST_KUBECONFIG_CONTENT);

    let original_content = fs::read_to_string(&kubeconfig_path).unwrap();

    process_kubeconfig_file(
        &kubeconfig_path,
        "9.9.9.9",
        "test_hash_456",
        &Some("new-context".to_string()),
        "test-server",
        true,
    )
    .unwrap();

    let content_after_dry_run = fs::read_to_string(kubeconfig_path).unwrap();

    // Verify the file content has not changed
    assert_eq!(original_content, content_after_dry_run);
}

#[test]
fn test_process_kubeconfig_file_hash_change_warning() {
    let temp_dir = Builder::new().prefix("test_kube_hash_change").tempdir().unwrap();
    let kubeconfig_path = setup_test_kubeconfig(&temp_dir, TEST_KUBECONFIG_CONTENT);

    // First run, should just write the file
    process_kubeconfig_file(
        &kubeconfig_path,
        "9.9.9.9",
        "first_hash",
        &None,
        "test-server",
        false,
    )
    .unwrap();

    // Second run with a different hash, should trigger a warning
    // (We can't easily check for logs here, but we're ensuring it runs without panic)
    let result = process_kubeconfig_file(
        &kubeconfig_path,
        "9.9.9.9",
        "second_hash",
        &None,
        "test-server",
        false,
    );
    assert!(result.is_ok());
}

#[test]
fn test_process_kubeconfig_no_context_update() {
    let temp_dir = Builder::new().prefix("test_kube_no_context").tempdir().unwrap();
    let kubeconfig_path = setup_test_kubeconfig(&temp_dir, TEST_KUBECONFIG_CONTENT);

    // When no target_context is set, server_name is used as the unique_name
    process_kubeconfig_file(
        &kubeconfig_path,
        "8.8.8.8",
        "some_hash",
        &None, // No target context — server_name becomes the unique_name
        "my-server",
        false,
    )
    .unwrap();

    let updated_content = fs::read_to_string(kubeconfig_path).unwrap();
    let updated_kubeconfig: super::kube::KubeConfig = serde_yaml::from_str(&updated_content).unwrap();

    // Context, cluster, and current-context should all be renamed to server_name
    assert_eq!(updated_kubeconfig.contexts[0].name, "my-server");
    assert_eq!(updated_kubeconfig.current_context, "my-server");
    assert_eq!(updated_kubeconfig.clusters[0].name, "my-server");
}

#[test]
fn test_cert_expiry_no_file() {
    let path = std::path::Path::new("/tmp/this_file_does_not_exist_xyz123");
    assert!(matches!(super::kube::check_local_cert_expiry(path), super::kube::CertStatus::Unknown));
}

#[test]
fn test_cert_expiry_no_field() {
    // Valid YAML but no certificate-expires-at field
    let content = r#"
apiVersion: v1
kind: Config
current-context: test
clusters: []
contexts: []
users: []
"#;
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(content.as_bytes()).unwrap();
    let result = super::kube::check_local_cert_expiry(file.path());
    assert!(matches!(result, super::kube::CertStatus::Unknown));
}

#[test]
fn test_cert_expiry_expired() {
    let content = r#"
apiVersion: v1
kind: Config
current-context: test
clusters: []
contexts: []
users: []
preferences:
  certificate-expires-at: "1970-01-01T00:00:00+00:00"
"#;
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(content.as_bytes()).unwrap();
    let result = super::kube::check_local_cert_expiry(file.path());
    assert!(matches!(result, super::kube::CertStatus::Expired(_)));
}

#[test]
fn test_cert_expiry_valid() {
    let content = r#"
apiVersion: v1
kind: Config
current-context: test
clusters: []
contexts: []
users: []
preferences:
  certificate-expires-at: "2099-01-01T00:00:00+00:00"
"#;
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(content.as_bytes()).unwrap();
    let result = super::kube::check_local_cert_expiry(file.path());
    assert!(matches!(result, super::kube::CertStatus::Valid(_)));
}

#[test]
fn test_cert_expiry_bad_date() {
    let content = r#"
apiVersion: v1
kind: Config
current-context: test
clusters: []
contexts: []
users: []
preferences:
  certificate-expires-at: "not-a-valid-date"
"#;
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(content.as_bytes()).unwrap();
    let result = super::kube::check_local_cert_expiry(file.path());
    assert!(matches!(result, super::kube::CertStatus::Unknown));
}

// ---------------------------------------------------------------------------
// Helpers for merge_into_main_kubeconfig tests
// ---------------------------------------------------------------------------

fn make_kubeconfig_yaml(context_name: &str, server_ip: &str) -> String {
    format!(
        r#"apiVersion: v1
kind: Config
current-context: {context_name}
clusters:
- name: {context_name}
  cluster:
    server: https://{server_ip}:6443
    certificate-authority-data: FAKECERT
contexts:
- name: {context_name}
  context:
    cluster: {context_name}
    user: {context_name}-user
users:
- name: {context_name}-user
  user:
    client-certificate-data: FAKECERT
    client-key-data: FAKEKEY
"#
    )
}

fn write_fetched_file(dir: &TempDir, context_name: &str, server_ip: &str) -> PathBuf {
    let path = dir.path().join("fetched_kubeconfig");
    fs::write(&path, make_kubeconfig_yaml(context_name, server_ip)).unwrap();
    path
}

fn main_kubeconfig_path() -> PathBuf {
    dirs::home_dir()
        .expect("home dir must exist")
        .join(".kube")
        .join("config")
}

/// Remove entries matching `context_name` from ~/.kube/config after a test.
/// Operates on clusters, contexts, and users. Does nothing if the file does not exist.
fn cleanup_test_context(context_name: &str) {
    let path = main_kubeconfig_path();
    if !path.exists() {
        return;
    }
    let content = fs::read_to_string(&path).unwrap();
    let mut config: KubeConfig = match serde_yaml::from_str(&content) {
        Ok(c) => c,
        Err(_) => return,
    };
    let user_name = format!("{}-user", context_name);
    config.clusters.retain(|c| c.name != context_name);
    config.contexts.retain(|c| c.name != context_name);
    config.users.retain(|u| u.name != user_name);
    let updated = serde_yaml::to_string(&config).unwrap();
    fs::write(&path, updated).unwrap();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_merge_dry_run() {
    let _kube_guard = KUBE_CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let context_name = "test-merge-DONOTKEEP-dryrun";
    let temp_dir = Builder::new().prefix("test_merge_dry_run").tempdir().unwrap();
    let fetched_path = write_fetched_file(&temp_dir, context_name, "10.99.0.1");

    let main_path = main_kubeconfig_path();
    let content_before = if main_path.exists() {
        fs::read_to_string(&main_path).ok()
    } else {
        None
    };
    let mtime_before = main_path.metadata().ok().and_then(|m| m.modified().ok());

    let result = merge_into_main_kubeconfig(&fetched_path, "test-server-dryrun", true);
    assert!(result.is_ok(), "dry_run merge returned error: {:?}", result);

    // File must not have been modified
    if let Some(before) = content_before {
        let content_after = fs::read_to_string(&main_path).unwrap();
        assert_eq!(before, content_after, "~/.kube/config was modified by a dry_run call");
    }
    // Verify mtime unchanged as a belt-and-suspenders check
    if let Some(mtime_after) = main_path.metadata().ok().and_then(|m| m.modified().ok()) {
        assert_eq!(
            mtime_before.unwrap(),
            mtime_after,
            "~/.kube/config mtime changed during dry_run"
        );
    }
}

#[test]
fn test_merge_no_preferences_copied() {
    let _kube_guard = KUBE_CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let context_name = "test-merge-DONOTKEEP-noprefs";
    let temp_dir = Builder::new().prefix("test_merge_noprefs").tempdir().unwrap();

    // Fetched file has preferences set
    let yaml = format!(
        r#"apiVersion: v1
kind: Config
current-context: {context_name}
clusters:
- name: {context_name}
  cluster:
    server: https://10.99.0.2:6443
    certificate-authority-data: FAKECERT
contexts:
- name: {context_name}
  context:
    cluster: {context_name}
    user: {context_name}-user
users:
- name: {context_name}-user
  user:
    client-certificate-data: FAKECERT
    client-key-data: FAKEKEY
preferences:
  certificate-expires-at: "2099-01-01T00:00:00+00:00"
  source-file-sha256: "deadbeef"
"#
    );
    let fetched_path = temp_dir.path().join("fetched_kubeconfig");
    fs::write(&fetched_path, &yaml).unwrap();

    let result = merge_into_main_kubeconfig(&fetched_path, "test-server-noprefs", false);
    assert!(result.is_ok(), "merge returned error: {:?}", result);

    let main_path = main_kubeconfig_path();
    let content = fs::read_to_string(&main_path).unwrap();
    let main_config: KubeConfig = serde_yaml::from_str(&content).unwrap();

    // The main config's preferences must not contain the source's preference keys
    if let Some(prefs) = &main_config.preferences {
        assert!(
            !prefs.contains_key("certificate-expires-at") || {
                // If the main config already had this key before the test, that is fine.
                // We only care that the value from the fetched file ("2099-01-01") was not injected.
                prefs
                    .get("certificate-expires-at")
                    .and_then(|v| v.as_str())
                    .map(|s| !s.starts_with("2099"))
                    .unwrap_or(true)
            },
            "certificate-expires-at from fetched file was incorrectly merged into main config"
        );
    }

    cleanup_test_context(context_name);
}

#[test]
fn test_merge_replaces_existing() {
    let _kube_guard = KUBE_CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let context_name = "test-merge-DONOTKEEP-replace";
    let temp_dir = Builder::new().prefix("test_merge_replace").tempdir().unwrap();

    // First merge with IP A
    let fetched_path = write_fetched_file(&temp_dir, context_name, "10.99.0.10");
    merge_into_main_kubeconfig(&fetched_path, "test-server-replace", false).unwrap();

    // Second merge with IP B (overwrite)
    let fetched_path2 = {
        let p = temp_dir.path().join("fetched_v2");
        fs::write(&p, make_kubeconfig_yaml(context_name, "10.99.0.20")).unwrap();
        p
    };
    merge_into_main_kubeconfig(&fetched_path2, "test-server-replace", false).unwrap();

    let main_path = main_kubeconfig_path();
    let content = fs::read_to_string(&main_path).unwrap();
    let main_config: KubeConfig = serde_yaml::from_str(&content).unwrap();

    // Only one cluster entry for this context name
    let matching_clusters: Vec<_> = main_config
        .clusters
        .iter()
        .filter(|c| c.name == context_name)
        .collect();
    assert_eq!(
        matching_clusters.len(),
        1,
        "expected exactly 1 cluster named '{}', got {}",
        context_name,
        matching_clusters.len()
    );
    assert_eq!(
        matching_clusters[0].cluster.server,
        "https://10.99.0.20:6443",
        "cluster server was not replaced with the second IP"
    );

    // Only one context entry
    let matching_contexts: Vec<_> = main_config
        .contexts
        .iter()
        .filter(|c| c.name == context_name)
        .collect();
    assert_eq!(
        matching_contexts.len(),
        1,
        "expected exactly 1 context named '{}'",
        context_name
    );

    cleanup_test_context(context_name);
}

#[test]
fn test_merge_preserves_other_contexts() {
    let _kube_guard = KUBE_CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let context_name = "test-merge-DONOTKEEP-preserve";
    let temp_dir = Builder::new().prefix("test_merge_preserve").tempdir().unwrap();

    let main_path = main_kubeconfig_path();
    let contexts_before: usize = if main_path.exists() {
        let content = fs::read_to_string(&main_path).unwrap();
        let config: KubeConfig = serde_yaml::from_str(&content).unwrap();
        config.contexts.len()
    } else {
        0
    };

    let fetched_path = write_fetched_file(&temp_dir, context_name, "10.99.0.30");
    merge_into_main_kubeconfig(&fetched_path, "test-server-preserve", false).unwrap();

    let content_after = fs::read_to_string(&main_path).unwrap();
    let config_after: KubeConfig = serde_yaml::from_str(&content_after).unwrap();
    let contexts_after = config_after.contexts.len();

    // We added exactly one new context
    assert_eq!(
        contexts_after,
        contexts_before + 1,
        "expected {} contexts after merge, got {}",
        contexts_before + 1,
        contexts_after
    );

    // The new context is present
    assert!(
        config_after.contexts.iter().any(|c| c.name == context_name),
        "merged context '{}' not found in main config",
        context_name
    );

    cleanup_test_context(context_name);
}

#[test]
fn test_merge_dry_run_nonexistent_fetched_returns_ok() {
    // In dry-run mode, a non-existent fetched file is fine — the real run
    // would have written it first. The function should return Ok and log.
    let result = merge_into_main_kubeconfig(
        std::path::Path::new("/tmp/this_does_not_exist_kube_test_xyz"),
        "test-server-nonexistent",
        true,
    );
    assert!(result.is_ok(), "expected Ok for missing fetched file in dry-run, got: {:?}", result.err());
}

#[test]
fn test_merge_non_dry_run_returns_err_for_nonexistent_fetched() {
    // Outside dry-run, a missing fetched file is a real error.
    let result = merge_into_main_kubeconfig(
        std::path::Path::new("/tmp/this_does_not_exist_kube_test_xyz"),
        "test-server-nonexistent",
        false,
    );
    assert!(result.is_err(), "expected Err for missing fetched file in real run, got Ok");
}

#[test]
fn test_merge_dry_run_valid_file_leaves_main_unchanged() {
    let _kube_guard = KUBE_CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let context_name = "test-merge-DONOTKEEP-dryrun2";
    let temp_dir = Builder::new().prefix("test_merge_dryrun2").tempdir().unwrap();
    let fetched_path = write_fetched_file(&temp_dir, context_name, "10.99.0.99");

    let main_path = main_kubeconfig_path();
    let content_before = if main_path.exists() {
        Some(fs::read_to_string(&main_path).unwrap())
    } else {
        None
    };

    let result = merge_into_main_kubeconfig(&fetched_path, "test-server-dryrun2", true);
    assert!(result.is_ok(), "dry_run merge returned error: {:?}", result);

    // Main config content must be byte-for-byte identical
    if let Some(before) = content_before {
        let after = fs::read_to_string(&main_path).unwrap();
        assert_eq!(
            before, after,
            "~/.kube/config was modified by second dry_run call"
        );
    }
}

// ---------------------------------------------------------------------------
// process_server early-return tests
// ---------------------------------------------------------------------------

/// When a locally cached kubeconfig has a future certificate-expires-at,
/// process_server must return Skipped(CertValid) without opening any SSH connection.
#[test]
fn test_process_server_cert_valid_skips_ssh() {
    use super::fetch::{process_server, ServerResult, SkipReason};

    let temp_dir = Builder::new()
        .prefix("test_proc_srv_cert_valid")
        .tempdir()
        .unwrap();

    let server_name = "test-proc-cert-valid";

    // Write a cached kubeconfig whose cert expires far in the future
    let local_path = temp_dir.path().join(server_name);
    fs::write(
        &local_path,
        r#"apiVersion: v1
kind: Config
current-context: test-ctx
clusters:
- name: test-ctx
  cluster:
    server: https://10.0.0.1:6443
    certificate-authority-data: FAKECERT
contexts:
- name: test-ctx
  context:
    cluster: test-ctx
    user: test-user
users:
- name: test-user
  user:
    client-certificate-data: FAKECERT
    client-key-data: FAKEKEY
preferences:
  certificate-expires-at: "2099-01-01T00:00:00+00:00"
"#,
    )
    .unwrap();

    let server = Server {
        name: server_name.to_string(),
        // RFC 5737 TEST-NET — guaranteed unreachable, so any SSH attempt would fail
        address: "192.0.2.1".to_string(),
        target_cluster_ip: "10.0.0.1".to_string(),
        user: Some("testuser".to_string()),
        file_path: Some("/etc/kubernetes".to_string()),
        file_name: Some("admin.conf".to_string()),
        context_name: None,
        identity_file: None,
    };

    let cfg = Config {
        default_user: None,
        default_file_path: None,
        default_file_name: None,
        default_identity_file: None,
        local_output_dir: temp_dir.path().to_string_lossy().into_owned(),
        servers: vec![],
    };

    let result = process_server(&server, &cfg, false, false);
    assert!(result.is_ok(), "expected Ok, got Err: {:?}", result.err());
    assert!(
        matches!(result.unwrap(), ServerResult::Skipped(SkipReason::CertValid(_))),
        "expected Skipped(CertValid), got something else"
    );
}
