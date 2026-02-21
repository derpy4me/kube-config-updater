use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, value};

/// Represents the main application configuration, loaded from a TOML file.
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Config {
    /// The default username to use for SSH connections if not specified per server.
    pub default_user: Option<String>,
    /// The default remote file path to use if not specified per server.
    pub default_file_path: Option<String>,
    /// The default remote file name to use if not specified per server.
    pub default_file_name: Option<String>,
    /// The default SSH identity file to use if not specified per server.
    pub default_identity_file: Option<String>,
    /// The local directory where fetched kubeconfig files will be stored.
    pub local_output_dir: String,
    /// A list of server configurations to process.
    #[serde(rename = "server")]
    pub servers: Vec<Server>,
}

/// Represents a single remote server to be processed.
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Server {
    /// A unique name for the server, used for local file naming.
    pub name: String,
    /// The SSH address (e.g., "host.example.com") of the server.
    pub address: String,
    /// The target IP address for the Kubernetes cluster.
    pub target_cluster_ip: String,
    /// The username for this specific server, overriding the default.
    pub user: Option<String>,
    /// The remote file path for this server, overriding the default.
    pub file_path: Option<String>,
    /// The remote file name for this server, overriding the default.
    pub file_name: Option<String>,
    /// The desired context name to set in the kubeconfig file.
    pub context_name: Option<String>,
    /// The SSH identity file for this specific server, overriding the default.
    pub identity_file: Option<String>,
}

impl Server {
    /// Gets the username for the server, falling back to the default from the main config.
    pub fn user<'a>(&'a self, config: &'a Config) -> Result<&'a str, anyhow::Error> {
        let user = self
            .user
            .as_deref()
            .or(config.default_user.as_deref())
            .ok_or_else(|| anyhow::anyhow!("[{}] user not specified in config", self.name))?;
        Ok(user)
    }

    /// Constructs the full remote file path for the server, combining path and name.
    /// Falls back to the defaults from the main config if not specified.
    pub fn file_path(&self, config: &Config) -> Result<String, anyhow::Error> {
        let file_path = self
            .file_path
            .as_deref()
            .or(config.default_file_path.as_deref())
            .ok_or_else(|| anyhow::anyhow!("[{}] file_path not specified in config", self.name))?;

        let file_name = self
            .file_name
            .as_deref()
            .or(config.default_file_name.as_deref())
            .ok_or_else(|| anyhow::anyhow!("[{}] file_name not specified in config", self.name))?;

        let full_path = file_path.to_owned() + "/" + file_name;
        log::debug!("Remote file path: {}", full_path);
        Ok(full_path)
    }

    /// Gets the identity file for the server, falling back to the default from the main config.
    pub fn identity_file<'a>(&'a self, config: &'a Config) -> Option<&'a str> {
        self.identity_file
            .as_deref()
            .or(config.default_identity_file.as_deref())
    }
}

/// Loads the application configuration from a specified TOML file path.
///
/// # Arguments
///
/// * `path` - A string slice that holds the path to the configuration file.
///
/// # Errors
///
/// This function will return an error if the file does not exist, cannot be read,
/// or if the content cannot be parsed as a valid `Config` structure.
pub fn load_config(path: &str) -> Result<Config, anyhow::Error> {
    match load_config_optional(path)? {
        Some(config) => Ok(config),
        None => anyhow::bail!("Configuration file not found at '{}'. Run `kube_config_updater tui` to set up.", path),
    }
}

/// Like `load_config` but returns `Ok(None)` when the file does not exist,
/// and `Err` only when the file exists but is invalid.
pub fn load_config_optional(path: &str) -> Result<Option<Config>, anyhow::Error> {
    log::debug!("Attempting to load configuration from '{}'...", path);

    if !Path::new(path).exists() {
        return Ok(None);
    }

    let config_content = fs::read_to_string(path)?;
    log::debug!("Successfully read config file.");

    let config: Config = toml::from_str(&config_content)
        .map_err(|e| anyhow::anyhow!("Configuration file at '{}' is invalid: {}", path, e))?;
    log::debug!("Successfully parsed configuration.");

    Ok(Some(config))
}

/// Append a new [[server]] entry to config.toml, preserving existing comments and formatting.
pub fn add_server(config_path: &PathBuf, server: &Server) -> Result<(), anyhow::Error> {
    let content = std::fs::read_to_string(config_path)?;
    let mut doc: DocumentMut = content.parse()
        .map_err(|e| anyhow::anyhow!("Failed to parse config.toml: {}", e))?;

    // Build the new entry table
    let mut entry = toml_edit::Table::new();
    entry["name"] = value(server.name.as_str());
    entry["address"] = value(server.address.as_str());
    entry["target_cluster_ip"] = value(server.target_cluster_ip.as_str());
    if let Some(ref u) = server.user {
        entry["user"] = value(u.as_str());
    }
    if let Some(ref fp) = server.file_path {
        entry["file_path"] = value(fp.as_str());
    }
    if let Some(ref fn_) = server.file_name {
        entry["file_name"] = value(fn_.as_str());
    }
    if let Some(ref ctx) = server.context_name {
        entry["context_name"] = value(ctx.as_str());
    }
    if let Some(ref id) = server.identity_file {
        entry["identity_file"] = value(id.as_str());
    }

    // Get or create the [[server]] array of tables
    if doc.get("server").is_none() {
        doc["server"] = Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
    }
    doc["server"]
        .as_array_of_tables_mut()
        .ok_or_else(|| anyhow::anyhow!("'server' key is not an array of tables"))?
        .push(entry);

    // Write atomically
    let tmp = config_path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string())
        .map_err(|e| anyhow::anyhow!("Couldn't save config.toml — check file permissions at {}: {}", config_path.display(), e))?;
    std::fs::rename(&tmp, config_path)?;
    Ok(())
}

/// Remove all [[server]] entries with the given name from config.toml.
pub fn remove_server(config_path: &PathBuf, name: &str) -> Result<(), anyhow::Error> {
    let content = std::fs::read_to_string(config_path)?;
    let mut doc: DocumentMut = content.parse()
        .map_err(|e| anyhow::anyhow!("Failed to parse config.toml: {}", e))?;

    if let Some(servers) = doc["server"].as_array_of_tables_mut() {
        let len_before = servers.len();
        // toml_edit 0.25 ArrayOfTables doesn't have retain; rebuild by removing matching indices
        let indices_to_remove: Vec<usize> = (0..servers.len())
            .filter(|&i| servers.get(i).and_then(|t| t["name"].as_str()) == Some(name))
            .collect();
        // Remove in reverse order to keep indices valid
        for &i in indices_to_remove.iter().rev() {
            servers.remove(i);
        }
        let len_after = servers.len();
        if len_before == len_after {
            log::warn!("remove_server: no server named '{}' found in config", name);
        }
    }

    let tmp = config_path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string())
        .map_err(|e| anyhow::anyhow!("Couldn't save config.toml — check file permissions at {}: {}", config_path.display(), e))?;
    std::fs::rename(&tmp, config_path)?;
    Ok(())
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_temp_config(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().expect("temp file");
        f.write_all(content.as_bytes()).expect("write");
        f
    }

    fn make_server(name: &str) -> Server {
        Server {
            name: name.to_string(),
            address: "192.168.1.10".to_string(),
            target_cluster_ip: "10.0.0.1".to_string(),
            user: Some("admin".to_string()),
            file_path: None,
            file_name: None,
            context_name: None,
            identity_file: None,
        }
    }

    #[test]
    fn test_add_server_appends_entry() {
        let initial = r#"
local_output_dir = "/tmp/kube"

[[server]]
name = "existing"
address = "1.2.3.4"
target_cluster_ip = "10.0.0.1"
"#;
        let f = write_temp_config(initial);
        let path = f.path().to_path_buf();

        add_server(&path, &make_server("new-server")).expect("add_server should succeed");

        let result = load_config(path.to_str().unwrap()).expect("load should succeed");
        assert_eq!(result.servers.len(), 2);
        assert_eq!(result.servers[1].name, "new-server");
        // Original entry preserved
        assert_eq!(result.servers[0].name, "existing");
    }

    #[test]
    fn test_add_server_preserves_comments() {
        let initial = "# This is my config\nlocal_output_dir = \"/tmp/kube\"\n";
        let f = write_temp_config(initial);
        let path = f.path().to_path_buf();

        add_server(&path, &make_server("s1")).expect("add should succeed");

        let content = std::fs::read_to_string(&path).expect("read");
        assert!(content.contains("# This is my config"), "comment should be preserved");
    }

    #[test]
    fn test_remove_server_removes_correct_entry() {
        let initial = r#"
local_output_dir = "/tmp/kube"

[[server]]
name = "keep-me"
address = "1.2.3.4"
target_cluster_ip = "10.0.0.1"

[[server]]
name = "delete-me"
address = "5.6.7.8"
target_cluster_ip = "10.0.0.2"
"#;
        let f = write_temp_config(initial);
        let path = f.path().to_path_buf();

        remove_server(&path, "delete-me").expect("remove should succeed");

        let result = load_config(path.to_str().unwrap()).expect("load should succeed");
        assert_eq!(result.servers.len(), 1);
        assert_eq!(result.servers[0].name, "keep-me");
    }
}
