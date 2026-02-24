use std::sync::mpsc;
use std::time::Duration;

use crate::config::Config;
use crate::state;

pub mod app;
pub mod features;

use app::{AppEvent, AppState, ProbeState, SetupWizardState, View};

pub fn run_tui(config: Config, config_path: std::path::PathBuf, dry_run: bool) -> anyhow::Result<()> {
    let server_states = state::read_state().unwrap_or_default();
    let mut app = AppState::new(config, config_path, server_states, dry_run);
    app.refresh_cert_cache();
    if !app.config.servers.is_empty() {
        app.table_state.select(Some(0));
    }
    run_app(app)
}

pub fn run_tui_setup(config_path: std::path::PathBuf, dry_run: bool) -> anyhow::Result<()> {
    let empty_config = crate::config::Config {
        default_user: None,
        default_file_path: None,
        default_file_name: None,
        default_identity_file: None,
        local_output_dir: String::new(),
        servers: vec![],
    };

    let initial_output_dir = dirs::home_dir()
        .map(|mut p| { p.push(".kube"); p.to_string_lossy().into_owned() })
        .unwrap_or_else(|| String::from("/tmp/kube"));

    let setup = SetupWizardState { output_dir: initial_output_dir, ..Default::default() };

    let mut app = AppState::new(empty_config, config_path, std::collections::HashMap::new(), dry_run);
    app.view = View::SetupWizard(setup);

    run_app(app)
}

fn run_app(mut app: AppState) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel::<AppEvent>();

    let tx_events = tx.clone();
    std::thread::spawn(move || {
        loop {
            match crossterm::event::read() {
                Ok(crossterm::event::Event::Key(k)) => {
                    if tx_events.send(AppEvent::Key(k)).is_err() { break; }
                }
                Ok(crossterm::event::Event::Resize(w, h)) => {
                    if tx_events.send(AppEvent::Resize(w, h)).is_err() { break; }
                }
                _ => {}
            }
        }
    });

    let tx_tick = tx.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(100));
            if tx_tick.send(AppEvent::Tick).is_err() { break; }
        }
    });

    let tx_watcher = tx.clone();
    std::thread::spawn(move || {
        let path = std::path::Path::new(state::STATE_FILE);
        let mut last_mtime: Option<std::time::SystemTime> = None;
        loop {
            std::thread::sleep(Duration::from_secs(2));
            if let Ok(meta) = std::fs::metadata(path)
                && let Ok(mtime) = meta.modified()
                && last_mtime.map(|m| m != mtime).unwrap_or(true)
            {
                last_mtime = Some(mtime);
                if tx_watcher.send(AppEvent::StateFileChanged).is_err() { break; }
            }
        }
    });

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, &rx, &tx);
    ratatui::restore();
    result
}

/// Record pre-fetch cert state, mark server as in-progress, and spawn a forced fetch.
/// Centralises the three-step setup that every fetch-triggering key handler needs.
pub(crate) fn start_fetch(
    app: &mut AppState,
    server: crate::config::Server,
    tx: &mpsc::Sender<AppEvent>,
) {
    let name = server.name.clone();
    app.pre_fetch_expiry.insert(name.clone(), app.cert_cache.get(&name).copied().flatten());
    app.in_progress.insert(name);
    spawn_fetch(server, app.config.clone(), app.dry_run, true, tx.clone());
}

pub(crate) fn spawn_fetch(
    server: crate::config::Server,
    config: crate::config::Config,
    dry_run: bool,
    force: bool,
    tx: mpsc::Sender<AppEvent>,
) {
    std::thread::spawn(move || {
        let result = crate::fetch::process_server(&server, &config, dry_run, force)
            .map(|_| ())
            .map_err(|e| friendly_error(&e));
        tx.send(AppEvent::FetchComplete {
            server_name: server.name,
            result,
        })
        .ok();
    });
}

/// Build a fetch completion notification that shows whether the cert changed.
fn build_fetch_notification(
    server_name: &str,
    pre_expiry: Option<Option<chrono::DateTime<chrono::Utc>>>,
    new_expiry: Option<chrono::DateTime<chrono::Utc>>,
    success: bool,
) -> String {
    if !success {
        return format!("{}: fetch failed", server_name);
    }
    let now = chrono::Utc::now();
    match new_expiry {
        None => format!("{}: fetched", server_name),
        Some(exp) if exp <= now => {
            format!("{}: fetched — cert expired {} (renew on server)", server_name, exp.format("%Y-%m-%d"))
        }
        Some(exp) => {
            let prev_was_expired = pre_expiry.flatten().map(|pre| pre <= now).unwrap_or(false);
            if prev_was_expired {
                format!("{}: cert renewed → {}", server_name, exp.format("%Y-%m-%d"))
            } else {
                format!("{}: fetched, cert expires {}", server_name, exp.format("%Y-%m-%d"))
            }
        }
    }
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut AppState,
    rx: &mpsc::Receiver<AppEvent>,
    tx: &mpsc::Sender<AppEvent>,
) -> anyhow::Result<()> {
    loop {
        // Render
        terminal.draw(|frame| render_app(frame, app))?;

        // Process next event
        match rx.recv() {
            Ok(AppEvent::Key(key)) => {
                if handle_key(app, key, tx, terminal) {
                    break; // quit
                }
            }
            Ok(AppEvent::Resize(_, _)) => {
                // ratatui handles resize automatically on next draw
            }
            Ok(AppEvent::Tick) => {
                app.spinner.tick();
                app.flash_rows.retain(|_, v| { *v = v.saturating_sub(1); *v > 0 });
                if let Some((_, ts)) = &app.notification
                    && ts.elapsed() > Duration::from_secs(3)
                {
                    app.notification = None;
                }
                // Skip redraw if nothing needs animating
                let probe_active = app.probe.as_ref()
                    .map(|(_, s)| matches!(s, ProbeState::Probing))
                    .unwrap_or(false);
                if app.in_progress.is_empty()
                    && app.flash_rows.is_empty()
                    && app.notification.is_none()
                    && !probe_active
                {
                    continue;
                }
            }
            Ok(AppEvent::ProbeComplete { server_name, result }) => {
                let probe_state = match result {
                    Ok(expiry) => ProbeState::Done(expiry),
                    Err(msg) => ProbeState::Failed(msg),
                };
                app.probe = Some((server_name, probe_state));
            }
            Ok(AppEvent::FetchComplete { server_name, result }) => {
                app.in_progress.remove(&server_name);
                let run_state = match &result {
                    Ok(()) => state::ServerRunState {
                        status: state::RunStatus::Fetched,
                        last_updated: Some(chrono::Utc::now()),
                        error: None,
                    },
                    Err(msg) => {
                        let status = if crate::state::is_auth_error(msg) {
                            state::RunStatus::AuthRejected
                        } else {
                            state::RunStatus::Failed
                        };
                        state::ServerRunState {
                            status,
                            last_updated: Some(chrono::Utc::now()),
                            error: Some(msg.clone()),
                        }
                    }
                };
                // Refresh cert cache directly from the kube file
                let mut local_path = std::path::PathBuf::from(&app.config.local_output_dir);
                local_path.push(&server_name);
                let new_expiry = match crate::kube::check_local_cert_expiry(&local_path) {
                    crate::kube::CertStatus::Valid(exp) | crate::kube::CertStatus::Expired(exp) => Some(exp),
                    _ => None,
                };
                app.cert_cache.insert(server_name.clone(), new_expiry);
                // Build delta notification before consuming pre_fetch_expiry
                let pre = app.pre_fetch_expiry.remove(&server_name);
                let notif = build_fetch_notification(&server_name, pre, new_expiry, result.is_ok());
                app.flash_rows.insert(server_name.clone(), 3);
                app.server_states.insert(server_name.clone(), run_state.clone());
                app.notification = Some((notif, std::time::Instant::now()));
                if let Err(e) = state::update_server_state(&server_name, run_state) {
                    log::warn!("Could not write state file: {}", e);
                }
            }
            Ok(AppEvent::WizardTestComplete { result }) => {
                features::wizard::on_test_complete(app, result);
            }
            Ok(AppEvent::StateFileChanged) => {
                match state::read_state() {
                    Ok(new_states) => {
                        app.server_states = new_states;
                        app.refresh_cert_cache();
                        app.notification = Some(("State refreshed".to_string(), std::time::Instant::now()));
                    }
                    Err(_) => {
                        app.notification = Some(("State file unreadable — showing cached data".to_string(), std::time::Instant::now()));
                    }
                }
            }
            Err(_) => break, // channel closed
        }
    }
    Ok(())
}

fn render_app(frame: &mut ratatui::Frame, app: &mut AppState) {
    // Extract a discriminant so we don't hold a borrow on app.view while calling
    // render functions that need &mut AppState.
    enum ViewKind {
        Dashboard,
        Detail(String),
        Wizard,
        SetupWizard,
        Help,
        ErrorView(String),
        CredentialMenu(String),
        CredentialInput(String),
        DeleteConfirm(String),
        KeyringFallbackConsent(String, String), // (server_name, keyring_error)
    }

    let kind = match &app.view {
        View::Dashboard => ViewKind::Dashboard,
        View::Detail(name) => ViewKind::Detail(name.clone()),
        View::Wizard(_) => ViewKind::Wizard,
        View::SetupWizard(_) => ViewKind::SetupWizard,
        View::Help => ViewKind::Help,
        View::Error { message } => ViewKind::ErrorView(message.clone()),
        View::CredentialMenu(name) => ViewKind::CredentialMenu(name.clone()),
        View::CredentialInput(name) => ViewKind::CredentialInput(name.clone()),
        View::DeleteConfirm(name) => ViewKind::DeleteConfirm(name.clone()),
        View::KeyringFallbackConsent { server_name, keyring_error, .. } => {
            ViewKind::KeyringFallbackConsent(server_name.clone(), keyring_error.clone())
        }
    };

    match kind {
        ViewKind::Dashboard => features::dashboard::render(frame, app),
        ViewKind::Detail(name) => features::detail::render(frame, app, &name),
        ViewKind::Wizard => {
            let ws = match &app.view {
                View::Wizard(ws) => ws.clone(),
                _ => unreachable!(),
            };
            features::wizard::render(frame, app, &ws);
        }
        ViewKind::SetupWizard => {
            let ws = match &app.view {
                View::SetupWizard(ws) => ws.clone(),
                _ => unreachable!(),
            };
            features::setup::render(frame, app, &ws);
        }
        ViewKind::Help => {
            features::dashboard::render(frame, app);
            features::help::render(frame, app);
        }
        ViewKind::ErrorView(message) => {
            features::dashboard::render(frame, app);
            features::render_dim_background(frame, frame.area());
            features::dashboard::render_error_overlay(frame, &message);
        }
        ViewKind::CredentialMenu(name) => {
            features::dashboard::render(frame, app);
            features::render_dim_background(frame, frame.area());
            features::credentials::render_menu(frame, app, &name);
        }
        ViewKind::CredentialInput(name) => {
            features::dashboard::render(frame, app);
            features::render_dim_background(frame, frame.area());
            features::credentials::render_input(frame, app, &name);
        }
        ViewKind::DeleteConfirm(name) => {
            features::dashboard::render(frame, app);
            features::render_dim_background(frame, frame.area());
            features::dashboard::render_delete_confirm(frame, app, &name);
        }
        ViewKind::KeyringFallbackConsent(server_name, keyring_error) => {
            features::dashboard::render(frame, app);
            features::keyring_fallback::render(frame, app, &server_name, &keyring_error);
        }
    }
}

fn handle_key(
    app: &mut AppState,
    key: crossterm::event::KeyEvent,
    tx: &mpsc::Sender<AppEvent>,
    terminal: &mut ratatui::DefaultTerminal,
) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};

    // Global: Ctrl+C and Ctrl+D always quit regardless of the active view.
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d'))
    {
        return true;
    }

    match &app.view {
        View::Dashboard => features::dashboard::handle_key(app, key, tx, terminal),
        View::Detail(name) => features::detail::handle_key(app, name.clone(), key, tx),
        View::DeleteConfirm(name) => features::dashboard::handle_key_delete_confirm(app, name.clone(), key),
        View::Help => { features::help::handle_key(app, key); false }
        View::Error { .. } => { app.view = View::Dashboard; false }
        View::CredentialMenu(name) => features::credentials::handle_key_menu(app, name.clone(), key),
        View::CredentialInput(name) => features::credentials::handle_key_input(app, name.clone(), key),
        View::Wizard(_) => features::wizard::handle_key(app, key, tx),
        View::SetupWizard(_) => features::setup::handle_key(app, key, tx),
        View::KeyringFallbackConsent { .. } => features::keyring_fallback::handle_key(app, key),
    }
}

/// Map an anyhow error to a human-readable, actionable message (NFR-7).
pub fn friendly_error(e: &anyhow::Error) -> String {
    let s = format!("{:#}", e);
    let lower = s.to_lowercase();
    if lower.contains("connection refused") || lower.contains("timed out") || lower.contains("no route") {
        return "Could not reach host — is it up and reachable from this machine?".to_string();
    }
    if lower.contains("authentication failed") || lower.contains("auth rejected") {
        return "Password rejected by server. Check credentials with 'c'.".to_string();
    }
    if lower.contains("sudo") || lower.contains("permission denied") {
        return "Connected but couldn't read the remote file — sudo may require a password or the path may be wrong.".to_string();
    }
    if lower.contains("yaml") || lower.contains("parse") {
        return "Remote file doesn't look like a kubeconfig — check the file path in config.".to_string();
    }
    if lower.contains("no clusters") {
        return "Kubeconfig has no cluster entries — expected standard k3s format.".to_string();
    }
    if lower.contains("keyring") || lower.contains("secret service") {
        return "OS keyring is locked or unavailable. Log in to unlock it, then retry.".to_string();
    }
    // Fallback: return original
    s
}

