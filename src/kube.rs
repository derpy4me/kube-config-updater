use anyhow::Context as _;
use base64::{engine::general_purpose, Engine as _};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use x509_parser::prelude::parse_x509_pem;

/// Represents the top-level structure of a Kubernetes config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeConfig {
    /// The API version of the kubeconfig file format.
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    /// The kind of configuration file, typically "Config".
    pub kind: String,
    /// The name of the context that is currently active.
    #[serde(rename = "current-context")]
    pub current_context: String,
    /// A list of all clusters defined in the configuration.
    pub clusters: Vec<ClusterInfo>,
    /// A list of all contexts defined in the configuration.
    pub contexts: Vec<ContextInfo>,
    /// A list of all users defined in the configuration.
    pub users: Vec<UserInfo>,
    /// A map for storing arbitrary, non-standard data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferences: Option<IndexMap<String, serde_yaml::Value>>,
}

/// Represents the validity state of a locally cached certificate.
#[derive(Debug)]
pub enum CertStatus {
    /// Cert is still valid — no action needed
    Valid(chrono::DateTime<chrono::Utc>),
    /// Cert is expired — fetch needed (carries the past expiry date for display)
    Expired(chrono::DateTime<chrono::Utc>),
    /// No local file, missing field, parse error — treat as unknown, fetch to be safe
    Unknown,
}

/// Checks the local cached kubeconfig to determine if the certificate is still valid.
/// Returns CertStatus::Unknown when the answer cannot be determined (missing file,
/// missing field, parse error) — callers should treat Unknown as "needs fetch".
pub fn check_local_cert_expiry(path: &std::path::Path) -> CertStatus {
    if !path.exists() {
        return CertStatus::Unknown;
    }
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return CertStatus::Unknown,
    };
    let kubeconfig: KubeConfig = match serde_yaml::from_str(&content) {
        Ok(k) => k,
        Err(_) => return CertStatus::Unknown,
    };
    let prefs = match kubeconfig.preferences {
        Some(p) => p,
        None => return CertStatus::Unknown,
    };
    let expiry_str = match prefs.get("certificate-expires-at").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => return CertStatus::Unknown,
    };
    let expiry = match chrono::DateTime::parse_from_rfc3339(&expiry_str) {
        Ok(dt) => dt.with_timezone(&chrono::Utc),
        Err(_) => return CertStatus::Unknown,
    };
    if expiry <= chrono::Utc::now() {
        CertStatus::Expired(expiry)
    } else {
        CertStatus::Valid(expiry)
    }
}

/// A named cluster entry in the kubeconfig.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterInfo {
    /// The name of the cluster.
    pub name: String,
    /// The detailed configuration for the cluster.
    pub cluster: Cluster,
}

/// Contains the connection details for a Kubernetes cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cluster {
    /// The URL of the Kubernetes API server.
    pub server: String,
    /// The base64-encoded certificate authority data for the cluster.
    #[serde(rename = "certificate-authority-data")]
    pub certificate_authority: String,
}

/// A named context entry in the kubeconfig.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextInfo {
    /// The name of the context.
    pub name: String,
    /// The detailed configuration for the context.
    pub context: Context,
}

/// Defines a context by linking a cluster, a user, and an optional namespace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    /// The name of the user for this context.
    pub user: String,
    /// The name of the cluster for this context.
    pub cluster: String,
}

/// A named user entry in the kubeconfig.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    /// The name of the user.
    pub name: String,
    /// The detailed configuration for the user.
    pub user: User,
}

/// Contains the authentication credentials for a user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    /// The base64-encoded client certificate data.
    #[serde(rename = "client-certificate-data")]
    pub certificate_data: String,
    /// The base64-encoded client key data.
    #[serde(rename = "client-key-data")]
    pub key_data: String,
}

/// Adds a timestamp to the kubeconfig preferences indicating when it was last updated.
fn add_last_updated_timestamp(kubeconfig: &mut KubeConfig) -> Result<(), anyhow::Error> {
    let preferences = kubeconfig.preferences.get_or_insert_with(IndexMap::new);
    let now = chrono::Utc::now();
    preferences.insert(
        "script-last-updated".to_string(),
        serde_yaml::to_value(now.to_rfc3339())?,
    );
    Ok(())
}

/// Parses the client certificate to find its expiration date and adds it to the preferences.
fn add_cert_expiration(kubeconfig: &mut KubeConfig) -> Result<(), anyhow::Error> {
    let Some(context_entry) = kubeconfig
        .contexts
        .iter()
        .find(|c| c.name == kubeconfig.current_context)
    else {
        log::warn!(
            "Could not find context '{}' to extract cert expiry — skipping",
            kubeconfig.current_context
        );
        return Ok(());
    };
    let user_name = &context_entry.context.user;

    let Some(user_info) = kubeconfig.users.iter().find(|u| u.name == *user_name) else {
        log::warn!(
            "Could not find user '{}' to extract cert expiry — skipping",
            user_name
        );
        return Ok(());
    };

    let pem_data = general_purpose::STANDARD.decode(&user_info.user.certificate_data)?;
    match parse_x509_pem(&pem_data) {
        Ok((_, pem)) => {
            let cert = pem.parse_x509()?;
            let expiration_time = cert.validity().not_after.to_datetime();
            let timestamp = expiration_time.unix_timestamp();

            if let Some(chrono_dt) = chrono::DateTime::from_timestamp(timestamp, 0) {
                log::info!("Certificate for user '{}' expires on : {}", user_name, chrono_dt);
                let preferences = kubeconfig.preferences.get_or_insert_with(IndexMap::new);
                preferences.insert(
                    "certificate-expires-at".to_string(),
                    serde_yaml::to_value(chrono_dt.to_rfc3339())?,
                );
            } else {
                log::warn!("Could not convert certificate timestamp for user '{}'", user_name);
            }
        }
        Err(e) => log::warn!(
            "Failed to parse PEM certificate for user '{}': {}. Skipping...",
            user_name,
            e
        ),
    }

    Ok(())
}

/// Adds the SHA256 hash of the original source file to the kubeconfig preferences.
fn add_source_hash(kubeconfig: &mut KubeConfig, source_hash: &str) -> Result<(), anyhow::Error> {
    let preferences = kubeconfig.preferences.get_or_insert_with(IndexMap::new);
    preferences.insert("source-file-sha256".to_string(), serde_yaml::to_value(source_hash)?);
    Ok(())
}

/// A helper function to call all metadata-adding functions.
fn add_metadata(kubeconfig: &mut KubeConfig, source_hash: &str) -> Result<(), anyhow::Error> {
    log::debug!("Adding/updating script metadata...");
    add_source_hash(kubeconfig, source_hash)?;
    add_last_updated_timestamp(kubeconfig)?;
    add_cert_expiration(kubeconfig)?;
    Ok(())
}

/// Updates the cluster's server URL and renames the cluster entry to `unique_name`
/// so that each server's cluster is independently addressable after merging.
fn update_cluster_info(kubeconfig: &mut KubeConfig, target_ip: &str, unique_name: &str) -> Result<(), anyhow::Error> {
    if let Some(cluster_info) = kubeconfig.clusters.get_mut(0) {
        log::info!(
            "Updating cluster '{}' server from '{}' to 'https://{}:6443'",
            cluster_info.name,
            cluster_info.cluster.server,
            target_ip
        );
        cluster_info.cluster.server = format!("https://{}:6443", target_ip);
        cluster_info.name = unique_name.to_string();
    } else {
        anyhow::bail!("No clusters found in the kubeconfig file.")
    }

    Ok(())
}

/// Renames the context, user, and all cross-references to `unique_name` so that
/// multiple servers whose k3s configs all default to "default" can coexist in
/// a merged ~/.kube/config without overwriting each other's entries.
fn update_context_info(kubeconfig: &mut KubeConfig, unique_name: &str) -> Result<(), anyhow::Error> {
    if let Some(user) = kubeconfig.users.get_mut(0) {
        user.name = unique_name.to_string();
    }

    if let Some(context_info) = kubeconfig.contexts.get_mut(0) {
        log::info!("Updating context name from '{}' to '{}'", context_info.name, unique_name);
        context_info.name = unique_name.to_string();
        context_info.context.cluster = unique_name.to_string();
        context_info.context.user = unique_name.to_string();
    } else {
        anyhow::bail!("No contexts found in the kubeconfig file.");
    }

    log::info!("Setting current-context to '{}'", unique_name);
    kubeconfig.current_context = unique_name.to_string();

    Ok(())
}

/// Reads a local kubeconfig file, applies modifications, and writes it back.
///
/// This is the main function for processing a fetched kubeconfig. It reads the file,
/// adds metadata, updates cluster and context information, and then saves the file.
pub fn process_kubeconfig_file(
    local_path: &Path,
    target_ip: &str,
    source_hash: &str,
    target_context: &Option<String>,
    server_name: &str,
    dry_run: bool,
) -> Result<(), anyhow::Error> {
    log::debug!("Processing file {:?}...", local_path);

    if !dry_run && local_path.exists() {
        let old_content = fs::read_to_string(local_path)?;
        if let Ok(old_kubeconfig) = serde_yaml::from_str::<KubeConfig>(&old_content)
            && let Some(prefs) = old_kubeconfig.preferences
            && let Some(old_hash) = prefs.get("source-file-sha256").and_then(|v| v.as_str())
            && old_hash != source_hash
        {
            log::warn!(
                "[{:?}] Source file on remote has changed since last run (SHA256: {} -> {})",
                local_path.file_name().unwrap_or_default(),
                &old_hash[..8],
                &source_hash[..8]
            );
        }
    }

    if dry_run && !local_path.exists() {
        log::info!(
            "DRY-RUN: No local file at {:?} — skipping kubeconfig processing (would write on a real run)",
            local_path
        );
        return Ok(());
    }

    let content = fs::read_to_string(local_path)?;
    let mut kubeconfig: KubeConfig = serde_yaml::from_str(&content)?;

    let unique_name = target_context.as_deref().unwrap_or(server_name);

    add_metadata(&mut kubeconfig, source_hash)?;
    update_cluster_info(&mut kubeconfig, target_ip, unique_name)?;
    update_context_info(&mut kubeconfig, unique_name)?;

    let updated_content = serde_yaml::to_string(&kubeconfig)?;

    if dry_run {
        log::info!(
            "DRY-RUN: Would have updated kubeconfig file at {:?}",
            local_path
        );
        // Optionally, you could print the diff or the would-be content here
        // log::info!("---\n{}---", updated_content);
    } else {
        fs::write(local_path, updated_content)?;
        log::info!("Successfully updated and saved kubeconfig file");
    }

    Ok(())
}

/// Parse the client certificate expiry directly from raw kubeconfig bytes.
/// Used for server probing — reads the cert without writing anything locally.
/// Returns `None` if content can't be parsed or no cert data is present.
pub fn parse_cert_expiry_from_bytes(content: &[u8]) -> Option<chrono::DateTime<chrono::Utc>> {
    let content_str = std::str::from_utf8(content).ok()?;
    let kubeconfig: KubeConfig = serde_yaml::from_str(content_str).ok()?;

    let context_entry = kubeconfig
        .contexts
        .iter()
        .find(|c| c.name == kubeconfig.current_context)?;
    let user_name = &context_entry.context.user;
    let user_info = kubeconfig.users.iter().find(|u| u.name == *user_name)?;

    let pem_data = general_purpose::STANDARD.decode(&user_info.user.certificate_data).ok()?;
    let (_, pem) = parse_x509_pem(&pem_data).ok()?;
    let cert = pem.parse_x509().ok()?;
    let timestamp = cert.validity().not_after.to_datetime().unix_timestamp();
    chrono::DateTime::from_timestamp(timestamp, 0)
}

/// Merges cluster, context, and user entries from a fetched per-server kubeconfig
/// into the main ~/.kube/config file. Existing entries with the same name are replaced.
/// Preferences and current_context in the main config are never modified.
pub fn merge_into_main_kubeconfig(
    fetched_path: &Path,
    server_name: &str,
    dry_run: bool,
) -> Result<(), anyhow::Error> {
    if dry_run && !fetched_path.exists() {
        log::info!(
            "[{}] DRY-RUN: Would merge processed config into ~/.kube/config",
            server_name
        );
        return Ok(());
    }

    let content = fs::read_to_string(fetched_path)?;
    let fetched: KubeConfig = serde_yaml::from_str(&content)?;

    let main_config_path = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?
        .join(".kube")
        .join("config");

    let mut main_config = if main_config_path.exists() {
        let main_content = fs::read_to_string(&main_config_path)?;
        serde_yaml::from_str::<KubeConfig>(&main_content)?
    } else {
        KubeConfig {
            api_version: "v1".to_string(),
            kind: "Config".to_string(),
            current_context: String::new(),
            clusters: Vec::new(),
            contexts: Vec::new(),
            users: Vec::new(),
            preferences: None,
        }
    };

    // Upsert clusters
    for cluster in &fetched.clusters {
        main_config.clusters.retain(|c| c.name != cluster.name);
        main_config.clusters.push(cluster.clone());
    }
    // Upsert contexts
    for context in &fetched.contexts {
        main_config.contexts.retain(|c| c.name != context.name);
        main_config.contexts.push(context.clone());
    }
    // Upsert users
    for user in &fetched.users {
        main_config.users.retain(|u| u.name != user.name);
        main_config.users.push(user.clone());
    }

    if dry_run {
        log::info!(
            "[{}] DRY-RUN: Would merge {} cluster(s), {} context(s), {} user(s) into {:?}",
            server_name,
            fetched.clusters.len(),
            fetched.contexts.len(),
            fetched.users.len(),
            main_config_path
        );
    } else {
        let updated = serde_yaml::to_string(&main_config)?;
        if let Some(parent) = main_config_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating directory {:?}", parent))?;
        }
        fs::write(&main_config_path, updated)
            .with_context(|| format!("writing {:?}", main_config_path))?;
        log::info!("[{}] Merged cluster/context/user into ~/.kube/config", server_name);
    }

    Ok(())
}
