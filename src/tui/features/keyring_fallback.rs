use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::tui::app::{AppState, View};
use super::{centered_rect, render_dim_background};

pub fn render(frame: &mut Frame, app: &AppState, server_name: &str, keyring_error: &str) {
    let area = frame.area();
    render_dim_background(frame, area);

    let popup_area = centered_rect(
        area.width.saturating_sub(4).min(68),
        area.height.saturating_sub(4).min(22),
        area,
    );
    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Credential Storage Fallback ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let rows = Layout::vertical([
        Constraint::Length(3), // error section (up to 3 wrapped lines)
        Constraint::Length(1), // blank
        Constraint::Length(3), // file path section (up to 3 wrapped lines)
        Constraint::Length(1), // blank
        Constraint::Fill(1),   // explanation
        Constraint::Length(1), // blank
        Constraint::Length(1), // key hints
    ])
    .split(inner);

    // Row 0: keyring error (truncated)
    let warn_style = if app.use_color {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    };
    let err_display = if keyring_error.len() > 120 {
        format!("{}…", &keyring_error[..120])
    } else {
        keyring_error.to_string()
    };
    let error_para = Paragraph::new(format!("  Keyring unavailable: {}", err_display))
        .style(warn_style)
        .wrap(Wrap { trim: true });
    frame.render_widget(error_para, rows[0]);

    // Row 2: fallback file path
    let file_path = crate::credentials::credential_file_path();
    let path_para = Paragraph::new(vec![
        Line::from(vec![
            Span::raw("  Fallback file: "),
            Span::styled(&file_path, Style::default().add_modifier(Modifier::BOLD)),
        ]),
        Line::from(format!("  Permissions: 0600  (only {} can read this file)", whoami())),
    ]);
    frame.render_widget(path_para.wrap(Wrap { trim: false }), rows[2]);

    // Row 4: explanation
    let explanation = vec![
        Line::from("  This is the same security model used by:"),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("~/.kube/config", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("   (kubectl credentials)"),
        ]),
        Line::from(vec![
            Span::raw("    "),
            Span::styled("~/.ssh/id_rsa", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("    (SSH private keys)"),
        ]),
        Line::from(""),
        Line::from("  To use the system keyring instead:"),
        Line::from("    install gnome-keyring or keepassxc (Secret Service plugin)"),
        Line::from(format!("    and store credential for '{}' with  c", server_name)),
    ];
    frame.render_widget(Paragraph::new(explanation).wrap(Wrap { trim: true }), rows[4]);

    // Row 6: key hints
    let hint_style = Style::default().add_modifier(Modifier::BOLD);
    let hints = Line::from(vec![
        Span::raw("  "),
        Span::styled("[y]", hint_style),
        Span::raw(" Store to file    "),
        Span::styled("[n]", hint_style),
        Span::raw(" Cancel — do not store"),
    ]);
    frame.render_widget(Paragraph::new(hints), rows[6]);
}

pub fn handle_key(app: &mut AppState, key: KeyEvent) -> bool {
    let (server_name, password) = match &app.view {
        View::KeyringFallbackConsent { server_name, password, .. } => {
            (server_name.clone(), password.clone())
        }
        _ => return false,
    };

    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            match crate::credentials::set_credential_file(&server_name, &password) {
                Ok(()) => {
                    app.notification = Some((
                        format!("Credential for '{}' stored in file (0600)", server_name),
                        std::time::Instant::now(),
                    ));
                }
                Err(e) => {
                    app.view = View::Error {
                        message: format!("Could not write credentials file: {}", e),
                        underlying: Box::new(View::Dashboard),
                    };
                    return false;
                }
            }
            app.view = View::Dashboard;
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            app.notification = Some((
                format!("Credential not stored for '{}'. Use 'c' to add later.", server_name),
                std::time::Instant::now(),
            ));
            app.view = View::Dashboard;
        }
        _ => {}
    }
    false
}

/// Returns the current Unix username for display in the consent dialog.
fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "you".to_string())
}
