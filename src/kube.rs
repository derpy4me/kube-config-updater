use base64::{engine::general_purpose, Engine as _};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use x509_parser::prelude::parse_x509_pem;

/// Represents the top-level structure of a Kubernetes config file.
#[derive(Debug, Serialize, Deserialize)]
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

/// A named cluster entry in the kubeconfig.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClusterInfo {
    /// The name of the cluster.
    pub name: String,
    /// The detailed configuration for the cluster.
    pub cluster: Cluster,
}

/// Contains the connection details for a Kubernetes cluster.
#[derive(Debug, Serialize, Deserialize)]
pub struct Cluster {
    /// The URL of the Kubernetes API server.
    pub server: String,
    /// The base64-encoded certificate authority data for the cluster.
    #[serde(rename = "certificate-authority-data")]
    pub certificate_authority: String,
}

/// A named context entry in the kubeconfig.
#[derive(Debug, Serialize, Deserialize)]
pub struct ContextInfo {
    /// The name of the context.
    pub name: String,
    /// The detailed configuration for the context.
    pub context: Context,
}

/// Defines a context by linking a cluster, a user, and an optional namespace.
#[derive(Debug, Serialize, Deserialize)]
pub struct Context {
    /// The name of the user for this context.
    pub user: String,
    /// The name of the cluster for this context.
    pub cluster: String,
}

/// A named user entry in the kubeconfig.
#[derive(Debug, Serialize, Deserialize)]
pub struct UserInfo {
    /// The name of the user.
    pub name: String,
    /// The detailed configuration for the user.
    pub user: User,
}

/// Contains the authentication credentials for a user.
#[derive(Debug, Serialize, Deserialize)]
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
    let user_name = &kubeconfig
        .contexts
        .iter()
        .find(|c| c.name == kubeconfig.current_context)
        .ok_or_else(|| anyhow::anyhow!("Could not find user for context '{}'", kubeconfig.current_context))?
        .context
        .user;

    let user_info = kubeconfig
        .users
        .iter()
        .find(|u| u.name == *user_name)
        .ok_or_else(|| anyhow::anyhow!("Could not find user info for user '{}'", user_name))?;

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

/// Updates the cluster's server URL to point to the new target IP address.
fn update_cluster_info(kubeconfig: &mut KubeConfig, target_ip: &str) -> Result<(), anyhow::Error> {
    if let Some(cluster_info) = kubeconfig.clusters.get_mut(0) {
        log::info!(
            "Updating cluster '{}' server from '{}' to 'https://{}:6443'",
            cluster_info.name,
            cluster_info.cluster.server,
            target_ip
        );
        cluster_info.cluster.server = format!("https://{}:6443", target_ip);
    } else {
        anyhow::bail!("No clusters found in the kubeonfig file.")
    }

    Ok(())
}

/// Updates the context name and sets the `current-context` field if a target context is provided.
fn update_context_info(kubeconfig: &mut KubeConfig, target_context: &Option<String>) -> Result<(), anyhow::Error> {
    if let Some(context) = target_context {
        if let Some(context_info) = kubeconfig.contexts.get_mut(0) {
            log::info!("Updating context name from '{}' to '{}'", context_info.name, context);
            context_info.name = context.to_string();
        } else {
            anyhow::bail!("No contexts found in the kubeconfig file.");
        }

        log::info!("Setting current-context to '{}'", context);
        kubeconfig.current_context = context.to_string();
    } else {
        log::debug!("Leaving default context")
    }

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

    let content = fs::read_to_string(local_path)?;
    let mut kubeconfig: KubeConfig = serde_yaml::from_str(&content)?;

    add_metadata(&mut kubeconfig, source_hash)?;
    update_cluster_info(&mut kubeconfig, target_ip)?;
    update_context_info(&mut kubeconfig, target_context)?;

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
