use super::config::{load_config, Config, Server};
use super::kube::process_kubeconfig_file;
use std::fs;
use std::io::Write;
use tempfile::{Builder, NamedTempFile, TempDir};

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
        false,
    );
    assert!(result.is_ok());
}

#[test]
fn test_process_kubeconfig_no_context_update() {
    let temp_dir = Builder::new().prefix("test_kube_no_context").tempdir().unwrap();
    let kubeconfig_path = setup_test_kubeconfig(&temp_dir, TEST_KUBECONFIG_CONTENT);

    process_kubeconfig_file(
        &kubeconfig_path,
        "8.8.8.8",
        "some_hash",
        &None, // No target context
        false,
    )
    .unwrap();

    let updated_content = fs::read_to_string(kubeconfig_path).unwrap();
    let updated_kubeconfig: super::kube::KubeConfig = serde_yaml::from_str(&updated_content).unwrap();

    // Context name and current-context should remain unchanged
    assert_eq!(updated_kubeconfig.contexts[0].name, "old-context");
    assert_eq!(updated_kubeconfig.current_context, "old-context");
}
