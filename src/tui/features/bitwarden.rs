use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::centered_rect;
use crate::tui::app::{AppEvent, AppState, View};

pub fn render(frame: &mut ratatui::Frame, app: &AppState) {
    let error = match &app.view {
        View::BitwardenUnlock { error } => error.clone(),
        _ => None,
    };

    let area = centered_rect(60, 12, frame.area());
    let block = Block::default()
        .title(" Bitwarden Vault Unlock ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::vertical([
        Constraint::Length(1), // status
        Constraint::Length(1), // blank
        Constraint::Length(1), // prompt label
        Constraint::Length(1), // password input
        Constraint::Length(1), // blank
        Constraint::Length(2), // error or hint
    ])
    .split(inner);

    let status_text = if app.in_progress.contains(crate::tui::app::BITWARDEN_SENTINEL) {
        "Unlocking vault..."
    } else {
        "Enter your Bitwarden master password to unlock the vault."
    };
    frame.render_widget(
        Paragraph::new(status_text).style(Style::default().fg(Color::White)),
        rows[0],
    );

    frame.render_widget(
        Paragraph::new("Master Password:").style(Style::default().fg(Color::Gray)),
        rows[2],
    );

    let masked = app.credential_input.masked_display();
    frame.render_widget(
        Paragraph::new(masked).style(Style::default().fg(Color::Yellow)),
        rows[3],
    );

    if let Some(err) = error {
        frame.render_widget(
            Paragraph::new(err)
                .style(Style::default().fg(Color::Red))
                .wrap(Wrap { trim: false }),
            rows[5],
        );
    } else {
        frame.render_widget(
            Paragraph::new("[Enter] Unlock  [Esc] Skip (local servers only)")
                .style(Style::default().fg(Color::DarkGray)),
            rows[5],
        );
    }
}

pub fn handle_key(app: &mut AppState, key: KeyEvent, tx: &std::sync::mpsc::Sender<AppEvent>) -> bool {
    match key.code {
        KeyCode::Esc => {
            // Skip vault unlock — proceed with local servers only
            app.credential_input.clear();
            app.view = View::Dashboard;
            false
        }
        KeyCode::Enter => {
            if app.credential_input.value.is_empty() {
                return false;
            }
            // Spawn vault unlock + fetch on background thread
            let password = app.credential_input.value.clone();
            app.credential_input.clear();
            app.in_progress.insert(crate::tui::app::BITWARDEN_SENTINEL.to_string());

            let bw_config = app.config.bitwarden.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                let result = do_bitwarden_unlock(&password, bw_config.as_ref());
                let _ = tx.send(AppEvent::BitwardenComplete { result });
            });
            false
        }
        KeyCode::Backspace => {
            app.credential_input.pop();
            false
        }
        KeyCode::Char(c) => {
            app.credential_input.push(c);
            false
        }
        _ => false,
    }
}

fn do_bitwarden_unlock(
    password: &str,
    bw_config: Option<&crate::bitwarden::BitwardenConfig>,
) -> Result<(Vec<crate::bitwarden::VaultServer>, Vec<String>), String> {
    let bw_config = bw_config.ok_or("Bitwarden not configured")?;
    let mut cli = crate::bitwarden::BwCli::new().with_server_url(bw_config.server_url.as_deref());

    cli.unlock(password)?;

    let prefix = bw_config.item_prefix.as_deref().unwrap_or("k3s:");
    cli.fetch_servers(prefix, bw_config.collection.as_deref())
}

/// Called by the event loop when BitwardenComplete arrives.
pub fn on_complete(app: &mut AppState, result: Result<(Vec<crate::bitwarden::VaultServer>, Vec<String>), String>) {
    app.in_progress.remove(crate::tui::app::BITWARDEN_SENTINEL);

    match result {
        Ok((vault_servers, skipped)) => {
            let (merged, sources, passwords) = crate::bitwarden::merge_servers(&app.config.servers, vault_servers);
            app.config.servers = merged;
            app.server_sources = sources;
            app.vault_passwords = passwords;
            app.refresh_cert_cache();
            if !app.config.servers.is_empty() {
                app.table_state.select(Some(0));
            }
            let vault_count = app
                .server_sources
                .values()
                .filter(|s| **s == crate::bitwarden::ServerSource::Vault)
                .count();
            let msg = if skipped.is_empty() {
                format!("Vault unlocked — {} server(s) loaded", vault_count)
            } else {
                format!(
                    "Vault unlocked — {} server(s) loaded, {} skipped (missing fields: {})",
                    vault_count,
                    skipped.len(),
                    skipped.join("; ")
                )
            };
            app.notification = Some((msg, std::time::Instant::now()));
            app.view = View::Dashboard;
        }
        Err(msg) => {
            app.view = View::BitwardenUnlock { error: Some(msg) };
        }
    }
}
