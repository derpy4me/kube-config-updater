use clap::{Parser, Subcommand};
use flexi_logger::{FileSpec, Logger, WriteMode};
use std::fs;
use std::path::PathBuf;

mod config;
mod credentials;
mod fetch;
mod kube;
mod ssh;
mod state;
pub mod tui;


#[derive(Subcommand, Debug)]
enum Commands {
    /// Manage SSH credentials stored in the OS keyring
    Credential {
        #[command(subcommand)]
        action: CredentialAction,
    },
    /// Launch the interactive TUI dashboard
    Tui,
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
    let is_tui = matches!(cli.command, Some(Commands::Tui));
    let has_log_dir = cli.log_dir.is_some();
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

    // In TUI mode without an explicit log dir, suppress all log output before
    // any log::info! calls. BufferAndFlush would otherwise flush buffered messages
    // into the alternate screen after ratatui::init(), corrupting the display.
    if is_tui && !has_log_dir {
        log::set_max_level(log::LevelFilter::Off);
    }

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
    if let Some(parent) = config_path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent)?;
        log::info!("Created configuration directory at: {}", parent.display());
    }

    // On first run, write a minimal default config so the app starts cleanly
    if !config_path.exists() {
        let output_dir = dirs::home_dir()
            .map(|mut p| { p.push(".kube"); p.to_string_lossy().into_owned() })
            .unwrap_or_else(|| "/tmp/kube".to_string());
        let template = format!(
            "# kube_config_updater configuration\n\
             # Run 'kube_config_updater tui' and press 'a' to add a server.\n\n\
             local_output_dir = \"{}\"\n",
            output_dir
        );
        fs::write(&config_path, &template)?;
        log::info!("Created default configuration file at: {}", config_path.display());
    }

    let config = config::load_config(config_path.to_str().unwrap_or_default())?;
    log::info!("Found {} servers in config", config.servers.len());

    match cli.command {
        Some(Commands::Credential { action }) => {
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
                    println!("{:<30} CREDENTIAL", "SERVER");
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
        Some(Commands::Tui) => {
            tui::run_tui(config, config_path, cli.dry_run)?;
            return Ok(());
        }
        None => {}
    }

    fetch::process_servers(&config, &cli.servers, cli.dry_run)?;

    Ok(())
}

#[cfg(test)]
mod tests;
