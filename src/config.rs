use serde::Deserialize;
use std::fs;
use std::path::Path;

/// Represents the main application configuration, loaded from a TOML file.
#[derive(Deserialize, Debug)]
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
#[derive(Deserialize, Debug)]
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
    log::debug!("Attempting to load configuration from '{}'...", path);

    if !Path::new(path).exists() {
        anyhow::bail!("Configuration file not found at '{}'. Please create it.", path);
    }

    let config_content = fs::read_to_string(path)?;
    log::debug!("Successfully read config file.");

    let config: Config = toml::from_str(&config_content)?;
    log::debug!("Successfully parsed configuration.");

    Ok(config)
}
