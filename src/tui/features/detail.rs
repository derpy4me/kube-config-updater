use std::sync::mpsc;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph},
    Frame,
};

use crate::credentials::CredentialResult;
use crate::tui::app::{AppEvent, AppState, ProbeState, View};
use super::{cert_color, cert_expires_display, status_color, status_display};

pub fn render(frame: &mut Frame, app: &mut AppState, server_name: &str) {
    let area = frame.area();

    let server = app.config.servers.iter().find(|s| s.name == server_name).cloned();

    let server = match server {
        Some(s) => s,
        None => {
            let msg = format!("Server not found: {}", server_name);
            frame.render_widget(
                Paragraph::new(msg).alignment(Alignment::Center),
                area,
            );
            return;
        }
    };

    let state = app.server_states.get(server_name).cloned();
    let cert_expires_at = app.cert_cache.get(server_name).and_then(|v| *v);
    let use_color = app.use_color;
    let config = app.config.clone();

    // Resolve optional fields with config defaults
    let user = server
        .user
        .as_deref()
        .or(config.default_user.as_deref())
        .unwrap_or("—")
        .to_string();

    let file_path = server
        .file_path
        .as_deref()
        .or(config.default_file_path.as_deref())
        .unwrap_or("—")
        .to_string();

    let file_name = server
        .file_name
        .as_deref()
        .or(config.default_file_name.as_deref())
        .unwrap_or("—")
        .to_string();

    let context_name = server
        .context_name
        .as_deref()
        .unwrap_or("—")
        .to_string();

    // Credential status
    let binding = [server_name];
    let cred_results = crate::credentials::check_credentials(&binding);
    let cred_stored = matches!(cred_results.first(), Some((_, CredentialResult::Found(_))));
    let cred_text = if cred_stored { "Stored" } else { "Not stored" };
    let cred_style = if !cred_stored && use_color {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    // Cert expiry — read from cert_cache (sourced from the kubeconfig file directly)
    let cert_value = match cert_expires_at {
        Some(ref exp) => exp.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        None => cert_expires_display(None),
    };
    let cert_style = cert_color(cert_expires_at.as_ref(), use_color);

    // Last updated
    let last_updated = state
        .as_ref()
        .and_then(|s| s.last_updated)
        .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "—".to_string());

    // Status
    let status_text;
    let status_style;
    match state.as_ref() {
        Some(s) => {
            status_text = status_display(&s.status).to_string();
            status_style = status_color(&s.status, use_color);
        }
        None => {
            status_text = "—".to_string();
            status_style = Style::default();
        }
    }

    // Error
    let error_text = state
        .as_ref()
        .and_then(|s| s.error.as_deref())
        .unwrap_or("—")
        .to_string();
    let has_error = state.as_ref().map(|s| s.error.is_some()).unwrap_or(false);
    let error_style = if has_error && use_color {
        Style::default().fg(Color::Red)
    } else {
        Style::default()
    };

    // Probe result for this server (if any)
    let probe_state = app.probe.as_ref().and_then(|(name, state)| {
        if name == server_name { Some(state.clone()) } else { None }
    });
    let spinner_char = app.spinner.current();

    // Separator line (fills available width, capped at content width)
    let sep = "─".repeat(area.width.saturating_sub(4) as usize);

    // Build lines
    let label_style = Style::default();

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("  Name:             ", label_style),
            Span::raw(server.name.clone()),
        ]),
        Line::from(vec![
            Span::styled("  Address:          ", label_style),
            Span::raw(server.address.clone()),
        ]),
        Line::from(vec![
            Span::styled("  SSH User:         ", label_style),
            Span::raw(user),
        ]),
        Line::from(vec![
            Span::styled("  Remote path:      ", label_style),
            Span::raw(file_path),
        ]),
        Line::from(vec![
            Span::styled("  Remote filename:  ", label_style),
            Span::raw(file_name),
        ]),
        Line::from(vec![
            Span::styled("  Cluster IP:       ", label_style),
            Span::raw(server.target_cluster_ip.clone()),
        ]),
        Line::from(vec![
            Span::styled("  Context name:     ", label_style),
            Span::raw(context_name),
        ]),
        Line::from(Span::raw(format!("  {}", sep))),
        Line::from(vec![
            Span::styled("  Cert expires:     ", label_style),
            Span::styled(cert_value, cert_style),
        ]),
        Line::from(vec![
            Span::styled("  Last updated:     ", label_style),
            Span::raw(last_updated),
        ]),
        Line::from(vec![
            Span::styled("  Credential:       ", label_style),
            Span::styled(cred_text, cred_style),
        ]),
        Line::from(vec![
            Span::styled("  Status:           ", label_style),
            Span::styled(status_text, status_style),
        ]),
        Line::from(vec![
            Span::styled("  Error:            ", label_style),
            Span::styled(error_text, error_style),
        ]),
    ];

    // ── Server cert probe section ────────────────────────────────────────────
    lines.push(Line::from(Span::raw(format!("  {}", sep))));
    match probe_state {
        None => {
            lines.push(Line::from(vec![
                Span::styled("  Server cert:      ", label_style),
                Span::styled("press p to probe", Style::default().add_modifier(Modifier::DIM)),
            ]));
        }
        Some(ProbeState::Probing) => {
            lines.push(Line::from(vec![
                Span::styled("  Server cert:      ", label_style),
                Span::raw(format!("{} Probing…", spinner_char)),
            ]));
        }
        Some(ProbeState::Done(server_expiry)) => {
            let now = chrono::Utc::now();
            let server_cert_str = match server_expiry {
                Some(exp) => exp.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
                None => "—".to_string(),
            };
            let server_cert_style = cert_color(server_expiry.as_ref(), use_color);

            // Comparison note: only highlight when there's a meaningful discrepancy
            let note = match (cert_expires_at, server_expiry) {
                (Some(local), Some(server)) if local == server && server <= now => {
                    " — cert expired on server (renew it there)"
                }
                (Some(local), Some(server)) if local != server && server > now => {
                    " — server has newer cert, run f to fetch"
                }
                _ => "",
            };

            lines.push(Line::from(vec![
                Span::styled("  Server cert:      ", label_style),
                Span::styled(server_cert_str, server_cert_style),
                Span::styled(note, Style::default().add_modifier(Modifier::DIM)),
            ]));
        }
        Some(ProbeState::Failed(err)) => {
            let err_style = if use_color {
                Style::default().fg(Color::Red)
            } else {
                Style::default()
            };
            lines.push(Line::from(vec![
                Span::styled("  Server cert:      ", label_style),
                Span::styled(format!("probe failed: {}", err), err_style),
            ]));
        }
    }

    // Outer layout: border block | content | footer
    let title = format!(" Server Detail: {} ", server_name);
    let outer_block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title);

    let inner_area = outer_block.inner(area);
    frame.render_widget(outer_block, area);

    // Split inner area: content (fill) | footer (1 row)
    let inner_chunks = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .split(inner_area);

    let content = Paragraph::new(lines);
    frame.render_widget(content, inner_chunks[0]);

    let footer = Paragraph::new(Line::from(vec![
        Span::raw("  f:force-fetch  p:probe  c:cred  Esc:back  ?:help"),
    ]));
    frame.render_widget(footer, inner_chunks[1]);
}

pub fn handle_key(
    app: &mut AppState,
    name: String,
    key: KeyEvent,
    tx: &mpsc::Sender<AppEvent>,
) -> bool {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.probe = None;
            app.view = View::Dashboard;
        }
        KeyCode::Char('f') => {
            if !app.in_progress.contains(&name)
                && let Some(server) = app.config.servers.iter().find(|s| s.name == name).cloned()
            {
                app.pre_fetch_expiry.insert(
                    name.clone(),
                    app.cert_cache.get(&name).copied().flatten(),
                );
                app.in_progress.insert(name.clone());
                crate::tui::spawn_fetch(server, app.config.clone(), app.dry_run, true, tx.clone());
            }
        }
        KeyCode::Char('p') => {
            let already_probing = app.probe.as_ref()
                .map(|(n, s)| n == &name && matches!(s, ProbeState::Probing))
                .unwrap_or(false);
            if !already_probing
                && let Some(server) = app.config.servers.iter().find(|s| s.name == name).cloned()
            {
                app.probe = Some((name.clone(), ProbeState::Probing));
                spawn_probe(server, app.config.clone(), tx.clone());
            }
        }
        KeyCode::Char('c') => {
            app.view = View::CredentialMenu(name);
        }
        KeyCode::Char('?') => {
            app.prior_view = Some(Box::new(View::Detail(name)));
            app.view = View::Help;
        }
        _ => {}
    }
    false
}

fn spawn_probe(
    server: crate::config::Server,
    config: crate::config::Config,
    tx: mpsc::Sender<AppEvent>,
) {
    std::thread::spawn(move || {
        let result = do_probe(&server, &config).map_err(|e| crate::tui::friendly_error(&e));
        tx.send(AppEvent::ProbeComplete {
            server_name: server.name,
            result,
        })
        .ok();
    });
}

fn do_probe(
    server: &crate::config::Server,
    config: &crate::config::Config,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, anyhow::Error> {
    let user = server.user(config)?;
    let remote_path_str = server.file_path(config)?;
    let identity_file = server.identity_file(config);
    let password = match crate::credentials::get_credential(&server.name) {
        crate::credentials::CredentialResult::Found(pw) => Some(pw),
        _ => None,
    };
    let contents = crate::ssh::fetch_remote_file(
        &server.name,
        &server.address,
        user,
        &remote_path_str,
        identity_file,
        password.as_deref(),
    )?;
    Ok(crate::kube::parse_cert_expiry_from_bytes(&contents))
}
