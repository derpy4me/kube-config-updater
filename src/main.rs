use clap::{Parser, Subcommand};
use flexi_logger::{FileSpec, Logger, WriteMode};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

mod config;
mod credentials;
mod kube;
mod ssh;

use config::{Config, Server};
use kube::process_kubeconfig_file;
use ssh::fetch_remote_file;

#[derive(Subcommand, Debug)]
enum Commands {
    /// Manage SSH credentials stored in the OS keyring
    Credential {
        #[command(subcommand)]
        action: CredentialAction,
    },
}

#[derive(Subcommand, Debug)]
enum CredentialAction {
    /// Store a credential for a server (prompts if --password is omitted)
    Set {
        #[arg(long, group = "target")]
        server: Option<String>,
        #[arg(long, group = "target")]
        default: bool,
        #[arg(long)]
        password: Option<String>,
    },
    /// Remove a stored credential
    Delete {
        #[arg(long, group = "target")]
        server: Option<String>,
        #[arg(long, group = "target")]
        default: bool,
    },
    /// Show which servers have a stored credential (never shows passwords)
    List,
}

enum SkipReason {
    CertValid(chrono::DateTime<chrono::Utc>),
    KeyringUnavailable,
}

enum ServerResult {
    Fetched,
    Skipped(SkipReason),
}

/// Command-line arguments for the kube_config_updater application.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Path to the configuration file.
    /// Defaults to $HOME/.kube_config_updater/config.toml
    #[arg(short, long)]
    config_path: Option<PathBuf>,

    /// If provided, logs will be written to a file in this directory.
    /// Otherwise, logs are written to stdout.
    #[arg(short, long)]
    log_dir: Option<PathBuf>,

    /// A list of specific server names to process.
    /// If not provided, all servers in the config will be processed.
    #[arg(short, long)]
    servers: Vec<String>,

    /// If set, the application will run in dry-run mode,
    /// printing actions instead of executing them.
    #[arg(long)]
    dry_run: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

fn process_server(server: &Server, config: &Config, dry_run: bool) -> Result<ServerResult, anyhow::Error> {
    let user = server.user(config)?;
    let remote_path_str = server.file_path(config)?;
    let identity_file = server.identity_file(config);

    let mut local_path = PathBuf::from(&config.local_output_dir);
    local_path.push(&server.name);

    // Step 1: Check local cert expiry — skip SSH if cert is still valid
    match kube::check_local_cert_expiry(&local_path) {
        kube::CertStatus::Valid(expiry) => {
            log::debug!("[{}] Cert valid until {}, skipping", server.name, expiry);
            return Ok(ServerResult::Skipped(SkipReason::CertValid(expiry)));
        }
        kube::CertStatus::Expired => {
            log::info!("[{}] Cert expired, fetching...", server.name);
        }
        kube::CertStatus::Unknown => {
            log::info!("[{}] Cert status unknown (no cache), fetching...", server.name);
        }
    }

    // Step 2: Look up credential from keyring
    let password: Option<String> = match credentials::get_credential(&server.name) {
        credentials::CredentialResult::Found(pw) => Some(pw),
        credentials::CredentialResult::NotFound => None,
        credentials::CredentialResult::Unavailable(reason) => {
            log::warn!(
                "[{}] Keyring unavailable ({}). Skipping. Run 'credential set' or log in to unlock keyring.",
                server.name,
                reason
            );
            return Ok(ServerResult::Skipped(SkipReason::KeyringUnavailable));
        }
    };

    // Step 3: Fetch the remote kubeconfig
    let contents = fetch_remote_file(
        &server.name,
        &server.address,
        user,
        &remote_path_str,
        identity_file,
        password.as_deref(),
    )?;

    // Step 4: Hash the contents
    let mut hasher = Sha256::new();
    hasher.update(&contents);
    let source_hash = format!("{:x}", hasher.finalize());
    log::debug!("[{}] Source file SHA256: {}", server.name, source_hash);

    // Step 5: Write local file
    if dry_run {
        log::info!("[{}] DRY-RUN: Would write config to {:?}", server.name, local_path);
    } else {
        fs::write(&local_path, &contents)?;
        log::info!("[{}] Config written to {:?}", server.name, local_path);
    }

    // Step 6: Process kubeconfig (update cluster IP, context name, add metadata)
    process_kubeconfig_file(
        &local_path,
        &server.target_cluster_ip,
        &source_hash,
        &server.context_name,
        &server.name,
        dry_run,
    )?;

    // Step 7: Merge into ~/.kube/config
    kube::merge_into_main_kubeconfig(&local_path, &server.name, dry_run)?;

    Ok(ServerResult::Fetched)
}

/// Iterates through and processes all servers defined in the configuration.
///
/// It ensures the output directory exists and then processes each server in parallel,
/// logging successes and failures.
fn process_servers(config: &Config, servers_to_process: &[String], dry_run: bool) -> Result<(), anyhow::Error> {
    fs::create_dir_all(&config.local_output_dir)?;
    log::info!("Using output directory: {}", &config.local_output_dir);

    let servers: Vec<_> = if servers_to_process.is_empty() {
        config.servers.iter().collect()
    } else {
        config
            .servers
            .iter()
            .filter(|s| servers_to_process.contains(&s.name))
            .collect()
    };

    if servers.is_empty() {
        log::warn!("No servers found to process. Check your --servers flag or config file.");
        return Ok(());
    }

    let bar = ProgressBar::new(servers.len() as u64);
    bar.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")?
            .progress_chars("#>-"),
    );

    let results: Vec<_> = servers
        .par_iter()
        .map(|&server| {
            let result = process_server(server, config, dry_run);
            bar.inc(1);
            (server, result)
        })
        .collect();

    bar.finish_and_clear();

    let mut fetched: u32 = 0;
    let mut skipped_cert_valid: u32 = 0;
    let mut skipped_no_cred: u32 = 0;
    let mut failed: u32 = 0;

    for (server, result) in results {
        match result {
            Ok(ServerResult::Fetched) => {
                fetched += 1;
                log::info!("[{}] Successfully fetched and merged.", server.name);
            }
            Ok(ServerResult::Skipped(SkipReason::CertValid(expiry))) => {
                skipped_cert_valid += 1;
                log::debug!("[{}] Cert valid until {}, skipping", server.name, expiry);
            }
            Ok(ServerResult::Skipped(SkipReason::KeyringUnavailable)) => {
                skipped_no_cred += 1;
                // Warning already logged inside process_server
            }
            Err(e) => {
                failed += 1;
                log::error!("[{}] FAILED: {}", server.name, e);
            }
        }
    }

    // Only emit a summary when something notable happened
    // Total silence when all certs are valid — safe for cron
    if fetched > 0 || failed > 0 || skipped_no_cred > 0 {
        log::info!(
            "Done. fetched={} skipped_cert_valid={} skipped_no_cred={} failed={}",
            fetched, skipped_cert_valid, skipped_no_cred, failed
        );
    }
    Ok(())
}

/// The main entry point of the application.
///
/// This function is responsible for:
/// - Parsing command-line arguments.
/// - Setting up the logger (either to stdout or a file).
/// - Determining the configuration file path.
/// - Loading the configuration.
/// - Initiating the server processing.
fn main() -> Result<(), anyhow::Error> {
    let cli = Cli::parse();

    // --- Logger Setup ---
    let mut logger = Logger::try_with_str("info")?;
    if let Some(log_dir) = cli.log_dir {
        // If a log directory is provided, log to a file.
        fs::create_dir_all(&log_dir).map_err(|e| {
            anyhow::anyhow!(
                "Failed to create log directory at '{}': {}. Check permissions.",
                log_dir.display(),
                e
            )
        })?;
        logger = logger.log_to_file(FileSpec::default().directory(&log_dir));
    } else {
        // Otherwise, log to stdout.
        logger = logger.log_to_stdout();
    }
    let _logger_handler = logger.write_mode(WriteMode::BufferAndFlush).start()?;

    let config_path = cli.config_path.unwrap_or_else(|| {
        dirs::home_dir()
            .map(|mut path| {
                path.push(".kube_config_updater");
                path.push("config.toml");
                path
            })
            .unwrap_or_else(|| PathBuf::from("config.toml"))
    });

    // Ensure the parent directory for the config file exists
    if let Some(parent) = config_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
            log::info!("Created configuration directory at: {}", parent.display());
        }
    }

    let config = config::load_config(config_path.to_str().unwrap_or_default())?;
    log::info!("Found {} servers in config", config.servers.len());

    if let Some(Commands::Credential { action }) = cli.command {
        match action {
            CredentialAction::Set { server, default, password } => {
                let account = if default {
                    credentials::DEFAULT_ACCOUNT.to_string()
                } else {
                    server.ok_or_else(|| anyhow::anyhow!("Specify --server <name> or --default"))?
                };
                let pw = match password {
                    Some(p) => p,
                    None => rpassword::prompt_password("Password: ")
                        .map_err(|e| anyhow::anyhow!("Failed to read password: {}", e))?,
                };
                credentials::set_credential(&account, &pw)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                println!("Credential stored for '{}'.", account);
            }
            CredentialAction::Delete { server, default } => {
                let account = if default {
                    credentials::DEFAULT_ACCOUNT.to_string()
                } else {
                    server.ok_or_else(|| anyhow::anyhow!("Specify --server <name> or --default"))?
                };
                credentials::delete_credential(&account)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                println!("Credential deleted for '{}'.", account);
            }
            CredentialAction::List => {
                let server_names: Vec<&str> = config.servers.iter().map(|s| s.name.as_str()).collect();
                let results = credentials::check_credentials(&server_names);
                println!("{:<30} {}", "SERVER", "CREDENTIAL");
                println!("{}", "-".repeat(40));
                let default_results = credentials::check_credentials(&[credentials::DEFAULT_ACCOUNT]);
                if let Some((_, default_result)) = default_results.first() {
                    let status = if matches!(default_result, credentials::CredentialResult::Found(_)) { "[SET]" } else { "[NOT SET]" };
                    println!("{:<30} {}", "_default", status);
                }
                for (name, result) in &results {
                    let status = if matches!(result, credentials::CredentialResult::Found(_)) { "[SET]" } else { "[NOT SET]" };
                    println!("{:<30} {}", name, status);
                }
            }
        }
        return Ok(());
    }

    process_servers(&config, &cli.servers, cli.dry_run)?;

    Ok(())
}

#[cfg(test)]
mod tests;
