use std::sync::mpsc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::tui::app::{AppEvent, AppState, AuthMethod, View, WizardState, WizardStep};
use super::{centered_rect, render_dim_background};

pub fn render(frame: &mut Frame, app: &mut AppState, wizard: &WizardState) {
    let area = frame.area();
    render_dim_background(frame, area);

    let popup_area = centered_rect(
        area.width.saturating_sub(4).min(70),
        area.height.saturating_sub(4).min(22),
        area,
    );
    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Add Server ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    // Split inner area into rows
    let rows = Layout::vertical([
        Constraint::Length(1), // row 0: step indicator
        Constraint::Length(1), // row 1: separator
        Constraint::Fill(1),   // row 2: content
        Constraint::Length(2), // row 3: error area
        Constraint::Length(1), // row 4: footer
    ])
    .split(inner);

    // Row 0: step indicator
    render_step_indicator(frame, wizard, rows[0]);

    // Row 1: separator
    let sep = Paragraph::new("  ────────────────────────────────────────────────────────");
    frame.render_widget(sep, rows[1]);

    // Row 2: content
    if wizard.step == WizardStep::Auth {
        render_auth_content(frame, app, wizard, rows[2]);
    } else {
        render_text_input_content(frame, wizard, rows[2]);
    }

    // Row 3: error area (always 2 rows)
    render_error_area(frame, app, wizard, rows[3]);

    // Row 4: footer
    render_footer(frame, wizard, rows[4]);

    // Help overlay (rendered on top of everything)
    if wizard.help_open {
        render_help_popup(frame, wizard);
    }
}

fn render_step_indicator(frame: &mut Frame, wizard: &WizardState, area: ratatui::layout::Rect) {
    let current_idx = wizard.step.index();
    let total = 8usize;

    let label = format!("  Step {} of {} — {}   ", current_idx + 1, total, wizard.step.label());

    let dots: String = (0..total)
        .map(|i| {
            if i < current_idx {
                '●'
            } else if i == current_idx {
                '◉'
            } else {
                '○'
            }
        })
        .collect();

    let dots_len = dots.chars().count() as u16;

    let cols = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(dots_len),
    ])
    .split(area);

    let left = Paragraph::new(label);
    frame.render_widget(left, cols[0]);

    let right = Paragraph::new(dots);
    frame.render_widget(right, cols[1]);
}

fn render_text_input_content(frame: &mut Frame, wizard: &WizardState, area: ratatui::layout::Rect) {
    let (field_label, value, hint) = match &wizard.step {
        WizardStep::Name => ("Server name", wizard.name.as_str(), "Unique identifier (no spaces)"),
        WizardStep::Address => ("SSH host/IP", wizard.address.as_str(), "e.g. 192.168.1.10 or myserver.local"),
        WizardStep::User => ("SSH user", wizard.user.as_str(), "Leave blank to use config default"),
        WizardStep::FilePath => (
            "Remote file path",
            wizard.file_path.as_str(),
            "e.g. /etc/rancher/k3s/k3s.yaml  (blank = k3s default)",
        ),
        WizardStep::FileName => (
            "Local filename",
            wizard.file_name.as_str(),
            "e.g. myserver.yaml  (blank = {name}.yaml)",
        ),
        WizardStep::TargetClusterIp => (
            "Cluster IP to write",
            wizard.target_cluster_ip.as_str(),
            "IP written into the kubeconfig context",
        ),
        WizardStep::ContextName => (
            "Context name",
            wizard.context_name.as_str(),
            "Leave blank to use server name",
        ),
        WizardStep::Auth => unreachable!("Auth step handled separately"),
    };

    let content_rows = Layout::vertical([
        Constraint::Length(1), // field label
        Constraint::Length(1), // input line
        Constraint::Length(1), // blank
        Constraint::Fill(1),   // hint
    ])
    .split(area);

    let label_line = Paragraph::new(format!("  {}:", field_label));
    frame.render_widget(label_line, content_rows[0]);

    let input_line = Paragraph::new(format!("  > {}│", value));
    frame.render_widget(input_line, content_rows[1]);

    let hint_line = Paragraph::new(format!("  {}", hint)).wrap(Wrap { trim: true });
    frame.render_widget(hint_line, content_rows[3]);
}

fn render_auth_content(
    frame: &mut Frame,
    app: &AppState,
    wizard: &WizardState,
    area: ratatui::layout::Rect,
) {
    let rows = Layout::vertical([
        Constraint::Length(1), // "Authentication method:"
        Constraint::Length(1), // [P] Password  [I] Identity file
        Constraint::Length(1), // blank
        Constraint::Length(1), // sub-label
        Constraint::Length(1), // sub-input
        Constraint::Fill(1),   // test status
    ])
    .split(area);

    let auth_label = Paragraph::new("  Authentication method:");
    frame.render_widget(auth_label, rows[0]);

    // Highlight selected method
    let pw_style = if wizard.auth_method == AuthMethod::Password {
        Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default()
    };
    let id_style = if wizard.auth_method == AuthMethod::IdentityFile {
        Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default()
    };

    let method_line = Line::from(vec![
        Span::raw("  "),
        Span::styled("[P] Password", pw_style),
        Span::raw("    "),
        Span::styled("[I] Identity file", id_style),
        Span::raw("   "),
    ]);
    frame.render_widget(Paragraph::new(method_line), rows[1]);

    // Sub-fields depending on method
    match wizard.auth_method {
        AuthMethod::Password => {
            let sub_label = Paragraph::new(if wizard.auth_input_focused {
                "  Password: [Enter to test, Esc to cancel]"
            } else {
                "  Password:"
            });
            frame.render_widget(sub_label, rows[3]);

            let masked = wizard.password_input.masked_display();
            let input_style = if wizard.auth_input_focused {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let input_line = Paragraph::new(format!("  > {}│", masked)).style(input_style);
            frame.render_widget(input_line, rows[4]);
        }
        AuthMethod::IdentityFile => {
            let sub_label = Paragraph::new(if wizard.auth_input_focused {
                "  Identity file path: [Enter to test, Esc to cancel]"
            } else {
                "  Identity file path:"
            });
            frame.render_widget(sub_label, rows[3]);

            let input_style = if wizard.auth_input_focused {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let input_line = Paragraph::new(format!("  > {}│", wizard.identity_file_input))
                .style(input_style);
            frame.render_widget(input_line, rows[4]);
        }
    }

    // Test status line
    let test_status = if wizard.testing {
        let spinner_frame = app.spinner.current();
        format!("  {} Testing...", spinner_frame)
    } else if wizard.test_passed {
        "  ✓ Connected".to_string()
    } else {
        String::new()
    };

    let test_style = if wizard.test_passed && !wizard.testing {
        if app.use_color {
            Style::default().fg(Color::Green)
        } else {
            Style::default()
        }
    } else {
        Style::default()
    };

    let test_line = Paragraph::new(test_status).style(test_style);
    frame.render_widget(test_line, rows[5]);
}

fn render_error_area(
    frame: &mut Frame,
    app: &AppState,
    wizard: &WizardState,
    area: ratatui::layout::Rect,
) {
    let err_rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);

    if let Some(ref err_msg) = wizard.error {
        let style = if app.use_color {
            Style::default().fg(Color::Red)
        } else {
            Style::default()
        };
        let err_line = Paragraph::new(format!("  {}", err_msg)).style(style).wrap(Wrap { trim: true });
        frame.render_widget(err_line, err_rows[0]);
    }
    // second row stays blank
    let _ = err_rows[1];
}

fn render_footer(frame: &mut Frame, wizard: &WizardState, area: ratatui::layout::Rect) {
    let hints = if wizard.step == WizardStep::Auth {
        if wizard.auth_input_focused {
            "  Enter: test  Esc: cancel  Backspace: delete"
        } else {
            "  Enter:type  t:test  s:save (after test)  Esc:back  ?:help"
        }
    } else {
        "  Enter: next  Esc: back  q: cancel  ?:help"
    };
    let footer = Paragraph::new(hints);
    frame.render_widget(footer, area);
}

pub fn handle_key(
    app: &mut AppState,
    key: KeyEvent,
    tx: &mpsc::Sender<AppEvent>,
) -> bool {
    let ws = match &app.view {
        View::Wizard(ws) => ws.clone(),
        _ => return false,
    };

    // Help popup: ? toggles, Esc closes; consumes all other keys while open.
    // Not intercepted when typing into the credential input (? is a valid password char).
    if !ws.auth_input_focused {
        if key.code == KeyCode::Char('?') {
            let mut ws = ws;
            ws.help_open = !ws.help_open;
            app.view = View::Wizard(ws);
            return false;
        }
        if ws.help_open {
            if key.code == KeyCode::Esc {
                let mut ws = ws;
                ws.help_open = false;
                app.view = View::Wizard(ws);
            }
            return false;
        }
    }

    if ws.step == WizardStep::Auth {
        let mut ws = ws;
        if ws.auth_input_focused {
            match key.code {
                KeyCode::Esc => {
                    ws.auth_input_focused = false;
                    app.view = View::Wizard(ws);
                }
                KeyCode::Enter => {
                    ws.auth_input_focused = false;
                    if !ws.testing {
                        ws.testing = true;
                        ws.test_passed = false;
                        ws.error = None;
                        let ws_snap = ws.clone();
                        let default_user = app.config.default_user.clone();
                        app.in_progress.insert("__wizard__".to_string());
                        app.view = View::Wizard(ws);
                        spawn_wizard_test(ws_snap, default_user, tx.clone());
                    } else {
                        app.view = View::Wizard(ws);
                    }
                }
                KeyCode::Backspace => {
                    match ws.auth_method {
                        AuthMethod::Password => { ws.password_input.pop(); }
                        AuthMethod::IdentityFile => { ws.identity_file_input.pop(); }
                    }
                    app.view = View::Wizard(ws);
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    match ws.auth_method {
                        AuthMethod::Password => { ws.password_input.push(c); }
                        AuthMethod::IdentityFile => { ws.identity_file_input.push(c); }
                    }
                    app.view = View::Wizard(ws);
                }
                _ => {}
            }
        } else {
            match key.code {
                KeyCode::Char('p') | KeyCode::Char('P') => {
                    ws.auth_method = AuthMethod::Password;
                    app.view = View::Wizard(ws);
                }
                KeyCode::Char('i') | KeyCode::Char('I') => {
                    ws.auth_method = AuthMethod::IdentityFile;
                    app.view = View::Wizard(ws);
                }
                KeyCode::Enter => {
                    ws.auth_input_focused = true;
                    app.view = View::Wizard(ws);
                }
                KeyCode::Char('t') | KeyCode::Char('T') => {
                    if !ws.testing {
                        ws.testing = true;
                        ws.test_passed = false;
                        ws.error = None;
                        let ws_snap = ws.clone();
                        let default_user = app.config.default_user.clone();
                        app.in_progress.insert("__wizard__".to_string());
                        app.view = View::Wizard(ws);
                        spawn_wizard_test(ws_snap, default_user, tx.clone());
                    }
                }
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    if ws.test_passed {
                        let ws_snap = ws.clone();
                        wizard_save(app, &ws_snap);
                    }
                }
                KeyCode::Esc => {
                    if let Some(prev) = ws.step.prev() {
                        ws.step = prev;
                        app.view = View::Wizard(ws);
                    }
                }
                _ => {}
            }
        }
    } else {
        match key.code {
            KeyCode::Char('q') => { app.view = View::Dashboard; }
            KeyCode::Esc => {
                let mut ws = ws;
                match ws.step.prev() {
                    Some(prev) => { ws.step = prev; app.view = View::Wizard(ws); }
                    None => { app.view = View::Dashboard; } // cancel at Name step
                }
            }
            KeyCode::Enter => {
                let mut ws = ws;
                if let Some(err) = wizard_validate_current(&ws, &app.config) {
                    ws.error = Some(err);
                    app.view = View::Wizard(ws);
                } else if let Some(next) = ws.step.next() {
                    ws.error = None;
                    ws.step = next;
                    app.view = View::Wizard(ws);
                }
            }
            KeyCode::Backspace => {
                let mut ws = ws;
                match ws.step {
                    WizardStep::Name => ws.name.pop(),
                    WizardStep::Address => ws.address.pop(),
                    WizardStep::User => ws.user.pop(),
                    WizardStep::FilePath => ws.file_path.pop(),
                    WizardStep::FileName => ws.file_name.pop(),
                    WizardStep::TargetClusterIp => ws.target_cluster_ip.pop(),
                    WizardStep::ContextName => ws.context_name.pop(),
                    WizardStep::Auth => None,
                };
                app.view = View::Wizard(ws);
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let mut ws = ws;
                match ws.step {
                    WizardStep::Name => ws.name.push(c),
                    WizardStep::Address => ws.address.push(c),
                    WizardStep::User => ws.user.push(c),
                    WizardStep::FilePath => ws.file_path.push(c),
                    WizardStep::FileName => ws.file_name.push(c),
                    WizardStep::TargetClusterIp => ws.target_cluster_ip.push(c),
                    WizardStep::ContextName => ws.context_name.push(c),
                    WizardStep::Auth => {}
                }
                app.view = View::Wizard(ws);
            }
            _ => {}
        }
    }
    false
}

fn spawn_wizard_test(ws: WizardState, default_user: Option<String>, tx: mpsc::Sender<AppEvent>) {
    std::thread::spawn(move || {
        let result = do_wizard_connection_test(&ws, default_user);
        tx.send(AppEvent::FetchComplete {
            server_name: "__wizard__".to_string(),
            result: result.map_err(|e| crate::tui::friendly_error(&e)),
        })
        .ok();
    });
}

fn do_wizard_connection_test(ws: &WizardState, default_user: Option<String>) -> Result<(), anyhow::Error> {
    let user = if !ws.user.is_empty() {
        ws.user.clone()
    } else if let Some(ref u) = default_user {
        u.clone()
    } else {
        anyhow::bail!("SSH user is required — fill in step 3 or set a default_user in your config")
    };
    let file_path = if ws.file_path.is_empty() {
        "/etc/rancher/k3s/k3s.yaml".to_string()
    } else {
        ws.file_path.clone()
    };
    let password = if ws.auth_method == AuthMethod::Password && !ws.password_input.value.is_empty() {
        Some(ws.password_input.value.clone())
    } else {
        None
    };
    let identity = if ws.auth_method == AuthMethod::IdentityFile && !ws.identity_file_input.is_empty() {
        Some(ws.identity_file_input.clone())
    } else {
        None
    };
    crate::ssh::fetch_remote_file(
        &ws.name,
        &ws.address,
        &user,
        &file_path,
        identity.as_deref(),
        password.as_deref(),
    )
    .map(|_| ())
}

fn wizard_save(app: &mut AppState, ws: &WizardState) {
    let server = crate::config::Server {
        name: ws.name.clone(),
        address: ws.address.clone(),
        target_cluster_ip: ws.target_cluster_ip.clone(),
        user: if ws.user.is_empty() { None } else { Some(ws.user.clone()) },
        file_path: if ws.file_path.is_empty() { None } else { Some(ws.file_path.clone()) },
        file_name: if ws.file_name.is_empty() { None } else { Some(ws.file_name.clone()) },
        context_name: if ws.context_name.is_empty() { None } else { Some(ws.context_name.clone()) },
        identity_file: if ws.auth_method == AuthMethod::IdentityFile && !ws.identity_file_input.is_empty() {
            Some(ws.identity_file_input.clone())
        } else {
            None
        },
    };
    if let Err(e) = crate::config::add_server(&app.config_path, &server) {
        app.view = View::Error {
            message: format!("Couldn't save server: {}", e),
            underlying: Box::new(View::Dashboard),
        };
        return;
    }
    if ws.auth_method == AuthMethod::Password && !ws.password_input.value.is_empty() {
        if let Err(e) = crate::credentials::set_credential(&ws.name, &ws.password_input.value) {
            // Server was already written to disk; reload config so it appears in the dashboard.
            let path_str = app.config_path.to_string_lossy().to_string();
            if let Ok(new_config) = crate::config::load_config(&path_str) {
                app.config = new_config;
            }
            app.view = View::Error {
                message: format!(
                    "Server '{}' was saved but the password could not be stored in the keyring: {}. \
                     Set it from the dashboard with 'c'.",
                    ws.name, e
                ),
                underlying: Box::new(View::Dashboard),
            };
            return;
        }
    }
    let _ = crate::state::update_server_state(
        &ws.name,
        crate::state::ServerRunState {
            status: crate::state::RunStatus::Fetched,
            last_updated: Some(chrono::Utc::now()),
            error: None,
        },
    );
    let path_str = app.config_path.to_string_lossy().to_string();
    match crate::config::load_config(&path_str) {
        Ok(new_config) => { app.config = new_config; }
        Err(e) => {
            app.view = View::Error {
                message: format!("Server saved but config reload failed: {}", e),
                underlying: Box::new(View::Dashboard),
            };
            return;
        }
    }
    app.notification = Some((
        format!("Server '{}' added", ws.name),
        std::time::Instant::now(),
    ));
    app.view = View::Dashboard;
}

fn wizard_validate_current(
    ws: &WizardState,
    config: &crate::config::Config,
) -> Option<String> {
    match &ws.step {
        WizardStep::Name => {
            if ws.name.is_empty() { return Some("Name cannot be empty".to_string()); }
            if ws.name.contains(' ') { return Some("Name cannot contain spaces".to_string()); }
            if config.servers.iter().any(|s| s.name == ws.name) {
                return Some(format!("A server named '{}' already exists", ws.name));
            }
            None
        }
        WizardStep::Address => {
            if ws.address.is_empty() { return Some("Address cannot be empty".to_string()); }
            None
        }
        WizardStep::TargetClusterIp => {
            if ws.target_cluster_ip.is_empty() {
                return Some("Target cluster IP cannot be empty".to_string());
            }
            let parts: Vec<&str> = ws.target_cluster_ip.split('.').collect();
            if parts.len() != 4 || !parts.iter().all(|p| p.parse::<u8>().is_ok()) {
                return Some("Invalid IP address (expected x.x.x.x)".to_string());
            }
            None
        }
        _ => None,
    }
}

// ─── Help Popup ───────────────────────────────────────────────────────────────

fn render_help_popup(frame: &mut Frame, wizard: &WizardState) {
    let area = frame.area();
    let popup_area = centered_rect(
        area.width.saturating_sub(4).min(62),
        area.height.saturating_sub(4).min(17),
        area,
    );
    frame.render_widget(Clear, popup_area);

    let title = format!(" ? Help — {} ", wizard.step.label());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let rows = Layout::vertical([
        Constraint::Fill(1),   // content
        Constraint::Length(1), // close hint
    ])
    .split(inner);

    let lines = step_help_lines(&wizard.step);
    frame.render_widget(Paragraph::new(lines), rows[0]);

    let close_hint = Paragraph::new("  ? or Esc to close")
        .style(Style::default().add_modifier(Modifier::DIM));
    frame.render_widget(close_hint, rows[1]);
}

fn step_help_lines(step: &WizardStep) -> Vec<Line<'static>> {
    fn h(s: &'static str) -> Line<'static> {
        Line::from(Span::styled(s, Style::default().add_modifier(Modifier::BOLD)))
    }
    fn t(s: &'static str) -> Line<'static> {
        Line::from(s)
    }
    let b = Line::from("");

    match step {
        WizardStep::Name => vec![
            b.clone(),
            h("  Purpose"),
            t("    The internal identifier used throughout"),
            t("    the app. Shown in the dashboard, local"),
            t("    filenames, and notifications."),
            b.clone(),
            h("  What to enter"),
            t("    A short label with no spaces."),
            t("    e.g.  k3s-home  or  prod-cluster"),
            b.clone(),
            h("  Tip"),
            t("    Must be unique. Cannot be changed after"),
            t("    saving without editing the config file."),
        ],
        WizardStep::Address => vec![
            b.clone(),
            h("  Purpose"),
            t("    The SSH host this tool connects to when"),
            t("    fetching your kubeconfig. Must be"),
            t("    reachable on port 22 from this machine."),
            b.clone(),
            h("  What to enter"),
            t("    An IP address or hostname."),
            t("    e.g.  192.168.1.10  or  k3s.local"),
        ],
        WizardStep::User => vec![
            b.clone(),
            h("  Purpose"),
            t("    The Unix user the tool will SSH as"),
            t("    on this server."),
            b.clone(),
            h("  What to enter"),
            t("    e.g.  root  or any user with file access."),
            t("    Leave blank to use the config default."),
            b.clone(),
            h("  Tip"),
            t("    For password auth, the user needs read"),
            t("    access to the remote file or sudo rights."),
        ],
        WizardStep::FilePath => vec![
            b.clone(),
            h("  Purpose"),
            t("    The absolute path to the kubeconfig"),
            t("    file on the remote server."),
            b.clone(),
            h("  What to enter"),
            t("    Leave blank for the k3s default:"),
            t("      /etc/rancher/k3s/k3s.yaml"),
            t("    For RKE2:"),
            t("      /etc/rancher/rke2/rke2.yaml"),
        ],
        WizardStep::FileName => vec![
            b.clone(),
            h("  Purpose"),
            t("    The filename used when saving this"),
            t("    server's kubeconfig in your local"),
            t("    output directory."),
            b.clone(),
            h("  What to enter"),
            t("    e.g.  mycluster.yaml"),
            t("    Leave blank to default to {name}.yaml"),
            t("    (where {name} is from step 1)."),
        ],
        WizardStep::TargetClusterIp => vec![
            b.clone(),
            h("  Purpose"),
            t("    k3s kubeconfigs often list 127.0.0.1 as"),
            t("    the cluster address. This IP replaces it"),
            t("    so kubectl works from your machine."),
            b.clone(),
            h("  What to enter"),
            t("    The server's IP reachable from here."),
            t("    e.g.  192.168.1.10  (often same as step 2)"),
            b.clone(),
            h("  Tip"),
            t("    Never use 127.0.0.1 — that routes"),
            t("    kubectl back to your local machine."),
        ],
        WizardStep::ContextName => vec![
            b.clone(),
            h("  Purpose"),
            t("    The Kubernetes context name written into"),
            t("    your kubeconfig. Used by kubectl, k9s,"),
            t("    and Lens to identify this cluster."),
            b.clone(),
            h("  What to enter"),
            t("    e.g.  home-cluster"),
            t("    Leave blank to use the server name"),
            t("    from step 1."),
        ],
        WizardStep::Auth => vec![
            b.clone(),
            h("  Purpose"),
            t("    How this tool authenticates via SSH"),
            t("    to fetch your kubeconfig."),
            b.clone(),
            h("  Password"),
            t("    Enter your SSH password. The tool uses"),
            t("    sudo -S to read the remote file."),
            b.clone(),
            h("  Identity file"),
            t("    Path to your SSH private key."),
            t("    e.g.  ~/.ssh/id_rsa"),
            t("    The key must be authorized on the server."),
        ],
    }
}
