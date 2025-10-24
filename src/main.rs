use clap::Parser;
use flexi_logger::{FileSpec, Logger, WriteMode};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

mod config;
mod kube;
mod ssh;

use config::{Config, Server};
use kube::process_kubeconfig_file;
use ssh::fetch_remote_file;

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
}

/// Processes a single server configuration.
///
/// This function fetches the remote kubeconfig file, calculates its hash,
/// writes it to a local file, and then processes it to update cluster information.
fn process_server(server: &Server, config: &Config, dry_run: bool) -> Result<(), anyhow::Error> {
    let user = server.user(config)?;
    let remote_path_str = server.file_path(config)?;
    let identity_file = server.identity_file(config);

    let contents = fetch_remote_file(
        &server.name,
        &server.address,
        user,
        &remote_path_str,
        identity_file,
    )?;

    let mut hasher = Sha256::new();
    hasher.update(&contents);
    let source_hash = format!("{:x}", hasher.finalize());
    log::debug!("[{}] Calculated Source file SHA256: {}", server.name, source_hash);

    let mut local_path = PathBuf::from(&config.local_output_dir);
    local_path.push(server.name.clone());

    if dry_run {
        log::info!("[{}] DRY-RUN: Would write config to {:?}", server.name, local_path);
    } else {
        fs::write(&local_path, contents)?;
        log::info!("[{}] Config successfully written: {:?}", server.name, local_path);
    }

    process_kubeconfig_file(
        &local_path,
        &server.target_cluster_ip,
        &source_hash,
        &server.context_name,
        dry_run,
    )?;

    Ok(())
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

    bar.finish_with_message("Finished processing all servers.");

    let mut successes = 0;
    let mut failures = 0;

    for (server, result) in results {
        match result {
            Ok(_) => {
                log::info!("[{}] Successfully processed.", server.name);
                successes += 1;
            }
            Err(e) => {
                log::error!("[{}] FAILED to process: {}", server.name, e);
                failures += 1;
            }
        }
    }

    log::info!("Finished. Successes: {}, Failures: {}", successes, failures);
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

    process_servers(&config, &cli.servers, cli.dry_run)?;

    Ok(())
}

#[cfg(test)]
mod tests;
