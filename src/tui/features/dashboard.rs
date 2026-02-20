use std::sync::mpsc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Cell, Clear, Paragraph, Row, Table, Wrap},
    Frame,
};

use crate::tui::app::{AppEvent, AppState, View, WizardState};
use super::{cert_color, cert_expires_display, centered_rect, status_color, status_display};

pub fn render(frame: &mut Frame, app: &mut AppState) {
    let area = frame.area();

    // Enforce minimum terminal size
    if area.width < 80 || area.height < 10 {
        let msg = format!(
            "Terminal too small ({}x{}) - minimum 80x10",
            area.width, area.height
        );
        frame.render_widget(Paragraph::new(msg).alignment(Alignment::Center), area);
        return;
    }

    // 3-row vertical layout: title | table | status bar
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .split(area);

    render_title_bar(frame, app, chunks[0]);
    render_server_table(frame, app, chunks[1]);
    render_status_bar(frame, app, chunks[2]);
}

fn render_title_bar(frame: &mut Frame, app: &AppState, area: ratatui::layout::Rect) {
    let dry_run_indicator = if app.dry_run {
        Span::styled(
            " [DRY-RUN] ",
            Style::default()
                .fg(if app.use_color { Color::Yellow } else { Color::Reset })
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw("")
    };

    let title_chunks = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(if app.dry_run { 11 } else { 0 }),
    ])
    .split(area);

    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            " kube_config_updater ",
            Style::default().add_modifier(Modifier::BOLD),
        )])),
        title_chunks[0],
    );

    if app.dry_run {
        frame.render_widget(
            Paragraph::new(Line::from(vec![dry_run_indicator])).alignment(Alignment::Right),
            title_chunks[1],
        );
    }
}

fn render_server_table(frame: &mut Frame, app: &mut AppState, area: ratatui::layout::Rect) {
    let rows: Vec<Row> = app
        .config
        .servers
        .iter()
        .map(|server| {
            let state = app.server_states.get(&server.name);
            let is_in_progress = app.in_progress.contains(&server.name);
            let is_flashing = app.flash_rows.get(&server.name).copied().unwrap_or(0) > 0;

            // STATUS column
            let (status_text, status_style) = if is_in_progress {
                (
                    format!("{} Fetching...", app.spinner.current()),
                    Style::default().fg(if app.use_color { Color::Cyan } else { Color::Reset }),
                )
            } else {
                let text = match state {
                    Some(s) => status_display(&s.status).to_string(),
                    None => "· Not run today".to_string(),
                };
                let style = match state {
                    Some(s) => status_color(&s.status, app.use_color),
                    None => Style::default().add_modifier(Modifier::DIM),
                };
                (text, style)
            };

            // CERT EXPIRES column — read directly from the cached kubeconfig file
            let (cert_str, cert_style) = {
                let expires = app.cert_cache.get(&server.name).and_then(|v| v.as_ref());
                (cert_expires_display(expires), cert_color(expires, app.use_color))
            };

            // NAME column — bold if row recently updated (flash)
            let name_style = if is_flashing {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(server.name.clone()).style(name_style),
                Cell::from(cert_str).style(cert_style),
                Cell::from(status_text).style(status_style),
            ])
        })
        .collect();

    let widths = [
        Constraint::Fill(1),     // NAME
        Constraint::Length(13),  // CERT EXPIRES (YYYY-MM-DD + padding)
        Constraint::Length(20),  // STATUS (fits "⚠ No credential" + spinner)
    ];

    let highlight_style = if app.use_color {
        Style::default()
            .bg(Color::Blue)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    };

    let table = Table::new(rows, widths)
        .header(
            Row::new(["NAME", "CERT EXPIRES", "STATUS"])
                .style(Style::default().add_modifier(Modifier::DIM | Modifier::BOLD)),
        )
        .row_highlight_style(highlight_style)
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(table, area, &mut app.table_state);
}

fn render_status_bar(frame: &mut Frame, app: &AppState, area: ratatui::layout::Rect) {
    // Show notification for 3s, then fall back to key hints
    let content = if let Some((msg, _)) = &app.notification {
        Line::from(vec![Span::styled(
            format!(" {} ", msg),
            Style::default().fg(if app.use_color { Color::Cyan } else { Color::Reset }),
        )])
    } else {
        let total = app.config.servers.len();
        let counter = match app.table_state.selected() {
            Some(sel) => format!(" {}/{} ", sel + 1, total),
            None => format!(" –/{} ", total),
        };

        let hints = " f:force-fetch  F:force-all  c:cred  a:add  D:del  d:dry-run  e:edit  ?:help  q:quit ";

        Line::from(vec![
            Span::styled(hints, Style::default().add_modifier(Modifier::DIM)),
            Span::styled(counter, Style::default().add_modifier(Modifier::DIM)),
        ])
    };

    frame.render_widget(Paragraph::new(content), area);
}

/// Error overlay — displays an error message over the dimmed dashboard.
pub fn render_error_overlay(frame: &mut Frame, message: &str) {
    let area = frame.area();
    let popup_width = (message.len() as u16 + 6).max(40).min(area.width.saturating_sub(4)).min(70);
    let popup_area = centered_rect(popup_width, 7, area);

    frame.render_widget(Clear, popup_area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" Error ")
        .title_style(Style::default().fg(Color::Red));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let content = format!("{}\n\nPress any key to dismiss.", message);
    frame.render_widget(
        Paragraph::new(content)
            .style(Style::default().fg(Color::Red))
            .wrap(Wrap { trim: true }),
        inner,
    );
}

/// Delete confirmation overlay.
pub fn render_delete_confirm(frame: &mut Frame, _app: &AppState, server_name: &str) {
    let area = frame.area();
    let popup_width = (server_name.len() as u16 + 22).max(40).min(area.width - 4);
    let popup_area = centered_rect(popup_width, 5, area);

    frame.render_widget(Clear, popup_area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" Delete Server ");

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let msg = format!("  Delete \"{}\"? [y/N]", server_name);
    frame.render_widget(
        Paragraph::new(Line::from(msg)).alignment(Alignment::Center),
        inner,
    );
}

pub fn handle_key(
    app: &mut AppState,
    key: KeyEvent,
    tx: &mpsc::Sender<AppEvent>,
    terminal: &mut ratatui::DefaultTerminal,
) -> bool {
    let selected_name: Option<String> = app
        .table_state
        .selected()
        .and_then(|i| app.config.servers.get(i))
        .map(|s| s.name.clone());

    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Char('j') | KeyCode::Down => {
            app.table_state.select_next();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.table_state.select_previous();
        }
        KeyCode::Char('g') => {
            app.table_state.select_first();
        }
        KeyCode::Char('G') => {
            app.table_state.select_last();
        }
        KeyCode::Enter => {
            if let Some(name) = selected_name {
                app.view = View::Detail(name);
            }
        }
        KeyCode::Char('f') => {
            if let Some(name) = selected_name
                && !app.in_progress.contains(&name)
                && let Some(server) = app.config.servers.iter().find(|s| s.name == name).cloned()
            {
                app.pre_fetch_expiry.insert(
                    name.clone(),
                    app.cert_cache.get(&name).copied().flatten(),
                );
                app.in_progress.insert(name);
                crate::tui::spawn_fetch(server, app.config.clone(), app.dry_run, true, tx.clone());
            }
        }
        KeyCode::Char('F') => {
            for server in app.config.servers.clone() {
                if !app.in_progress.contains(&server.name) {
                    app.pre_fetch_expiry.insert(
                        server.name.clone(),
                        app.cert_cache.get(&server.name).copied().flatten(),
                    );
                    app.in_progress.insert(server.name.clone());
                    crate::tui::spawn_fetch(server, app.config.clone(), app.dry_run, true, tx.clone());
                }
            }
        }
        KeyCode::Char('c') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(name) = selected_name {
                app.view = View::CredentialMenu(name);
            }
        }
        KeyCode::Char('d') => {
            app.dry_run = !app.dry_run;
            let msg = if app.dry_run { "Dry-run ON" } else { "Dry-run OFF" };
            app.notification = Some((msg.to_string(), std::time::Instant::now()));
        }
        KeyCode::Char('a') => {
            app.view = View::Wizard(WizardState::new());
        }
        KeyCode::Char('D') => {
            if let Some(name) = selected_name {
                app.view = View::DeleteConfirm(name);
            }
        }
        KeyCode::Char('?') => {
            app.prior_view = Some(Box::new(View::Dashboard));
            app.view = View::Help;
        }
        KeyCode::Char('e') => {
            open_editor(terminal, app);
        }
        _ => {}
    }
    false
}

pub fn handle_key_delete_confirm(app: &mut AppState, name: String, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('y') => {
            perform_delete(app, &name);
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            app.view = View::Dashboard;
        }
        _ => {}
    }
    false
}

fn perform_delete(app: &mut AppState, server_name: &str) {
    // Remove from config.toml
    if let Err(e) = crate::config::remove_server(&app.config_path, server_name) {
        let msg = format!("Couldn't delete server: {}", e);
        app.view = View::Error {
            message: msg,
            underlying: Box::new(View::Dashboard),
        };
        return;
    }

    // Delete the cached local file if it exists
    let mut local_path = std::path::PathBuf::from(&app.config.local_output_dir);
    local_path.push(server_name);
    let _ = std::fs::remove_file(&local_path); // non-fatal

    // Remove from in-memory state
    app.config.servers.retain(|s| s.name != server_name);
    app.server_states.remove(server_name);
    app.cert_cache.remove(server_name);
    app.in_progress.remove(server_name);
    app.flash_rows.remove(server_name);

    // Clamp selection
    let total = app.config.servers.len();
    if total == 0 {
        app.table_state = ratatui::widgets::TableState::default();
    } else if let Some(sel) = app.table_state.selected()
        && sel >= total
    {
        app.table_state.select_last();
    }

    app.notification = Some((
        format!("Deleted server: {}", server_name),
        std::time::Instant::now(),
    ));
    app.view = View::Dashboard;
}

fn open_editor(terminal: &mut ratatui::DefaultTerminal, app: &mut AppState) {
    ratatui::restore();

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let _ = std::process::Command::new(&editor)
        .arg(&app.config_path)
        .status();

    // Reinit terminal and overwrite the handle in place
    *terminal = ratatui::init();

    // Reload config
    let path_str = app.config_path.to_string_lossy().to_string();
    match crate::config::load_config(&path_str) {
        Ok(new_config) => {
            app.config = new_config;
            app.refresh_cert_cache();
            app.notification = Some(("Config reloaded".to_string(), std::time::Instant::now()));
        }
        Err(e) => {
            app.view = View::Error {
                message: format!("config.toml could not be read after edit: {}", e),
                underlying: Box::new(View::Dashboard),
            };
        }
    }
}
