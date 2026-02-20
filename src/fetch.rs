use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

pub(crate) enum SkipReason {
    CertValid(chrono::DateTime<chrono::Utc>),
    KeyringUnavailable,
}

pub(crate) enum ServerResult {
    Fetched,
    Skipped(SkipReason),
}

pub(crate) fn process_server(
    server: &crate::config::Server,
    config: &crate::config::Config,
    dry_run: bool,
    force: bool,
) -> Result<ServerResult, anyhow::Error> {
    let user = server.user(config)?;
    let remote_path_str = server.file_path(config)?;
    let identity_file = server.identity_file(config);

    let mut local_path = PathBuf::from(&config.local_output_dir);
    local_path.push(&server.name);

    // Step 1: Check local cert expiry — skip SSH if cert is still valid (unless force)
    if !force {
        match crate::kube::check_local_cert_expiry(&local_path) {
            crate::kube::CertStatus::Valid(expiry) => {
                log::debug!("[{}] Cert valid until {}, skipping", server.name, expiry);
                return Ok(ServerResult::Skipped(SkipReason::CertValid(expiry)));
            }
            crate::kube::CertStatus::Expired(_) => {
                log::info!("[{}] Cert expired, fetching...", server.name);
            }
            crate::kube::CertStatus::Unknown => {
                log::info!("[{}] Cert status unknown (no cache), fetching...", server.name);
            }
        }
    }

    // Step 2: Look up credential from keyring
    let password: Option<String> = match crate::credentials::get_credential(&server.name) {
        crate::credentials::CredentialResult::Found(pw) => Some(pw),
        crate::credentials::CredentialResult::NotFound => None,
        crate::credentials::CredentialResult::Unavailable(reason) => {
            log::warn!(
                "[{}] Keyring unavailable ({}). Skipping. Run 'credential set' or log in to unlock keyring.",
                server.name,
                reason
            );
            return Ok(ServerResult::Skipped(SkipReason::KeyringUnavailable));
        }
    };

    // Step 3: Fetch the remote kubeconfig
    let contents = crate::ssh::fetch_remote_file(
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
    crate::kube::process_kubeconfig_file(
        &local_path,
        &server.target_cluster_ip,
        &source_hash,
        &server.context_name,
        &server.name,
        dry_run,
    )?;

    // Step 7: Merge into ~/.kube/config
    crate::kube::merge_into_main_kubeconfig(&local_path, &server.name, dry_run)?;

    Ok(ServerResult::Fetched)
}

/// Iterates through and processes all servers defined in the configuration.
///
/// It ensures the output directory exists and then processes each server in parallel,
/// logging successes and failures.
pub(crate) fn process_servers(
    config: &crate::config::Config,
    servers_to_process: &[String],
    dry_run: bool,
) -> Result<(), anyhow::Error> {
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
            let result = process_server(server, config, dry_run, false);
            bar.inc(1);
            (server, result)
        })
        .collect();

    bar.finish_and_clear();

    let mut fetched: u32 = 0;
    let mut skipped_cert_valid: u32 = 0;
    let mut skipped_no_cred: u32 = 0;
    let mut failed: u32 = 0;

    // Load existing state so entries for servers not in this run are preserved
    let mut state_entries = crate::state::read_state().unwrap_or_default();

    for (server, result) in &results {
        let server_state = match result {
            Ok(ServerResult::Fetched) => {
                fetched += 1;
                log::info!("[{}] Successfully fetched and merged.", server.name);
                crate::state::ServerRunState {
                    status: crate::state::RunStatus::Fetched,
                    last_updated: Some(chrono::Utc::now()),
                    error: None,
                }
            }
            Ok(ServerResult::Skipped(SkipReason::CertValid(expiry))) => {
                skipped_cert_valid += 1;
                log::debug!("[{}] Cert valid until {}, skipping", server.name, expiry);
                crate::state::ServerRunState {
                    status: crate::state::RunStatus::Skipped,
                    last_updated: Some(chrono::Utc::now()),
                    error: None,
                }
            }
            Ok(ServerResult::Skipped(SkipReason::KeyringUnavailable)) => {
                skipped_no_cred += 1;
                crate::state::ServerRunState {
                    status: crate::state::RunStatus::NoCredential,
                    last_updated: Some(chrono::Utc::now()),
                    error: None,
                }
            }
            Err(e) => {
                failed += 1;
                log::error!("[{}] FAILED: {}", server.name, e);
                let e_str = format!("{:#}", e);
                let status = if e_str.to_lowercase().contains("authentication failed")
                    || e_str.to_lowercase().contains("auth rejected")
                {
                    crate::state::RunStatus::AuthRejected
                } else {
                    crate::state::RunStatus::Failed
                };
                crate::state::ServerRunState {
                    status,
                    last_updated: Some(chrono::Utc::now()),
                    error: Some(e_str),
                }
            }
        };
        state_entries.insert(server.name.clone(), server_state);
    }

    // Only emit a summary when something notable happened
    // Total silence when all certs are valid — safe for cron
    if fetched > 0 || failed > 0 || skipped_no_cred > 0 {
        log::info!(
            "Done. fetched={} skipped_cert_valid={} skipped_no_cred={} failed={}",
            fetched, skipped_cert_valid, skipped_no_cred, failed
        );
    }

    // Write state file for TUI to consume (non-fatal)
    if let Err(e) = crate::state::write_state(&state_entries) {
        log::warn!("Could not write state file: {}", e);
    }

    Ok(())
}
