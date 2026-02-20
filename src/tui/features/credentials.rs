use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Frame,
};

use crate::tui::app::{AppState, View};
use super::{centered_rect, render_dim_background};

pub fn render_menu(frame: &mut Frame, app: &mut AppState, server_name: &str) {
    render_dim_background(frame, frame.area());

    let area = centered_rect(40, 7, frame.area());

    frame.render_widget(Clear, area);

    let title = format!(" Credentials: {} ", server_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().fg(Color::White));

    let lines = vec![
        Line::from(""),
        Line::from(vec![Span::raw("   [s] Set password")]),
        Line::from(vec![Span::raw("   [d] Delete credential")]),
        Line::from(vec![Span::raw("   [Esc] Cancel")]),
        Line::from(""),
    ];

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
    let _ = app;
}

pub fn render_input(frame: &mut Frame, app: &mut AppState, server_name: &str) {
    render_dim_background(frame, frame.area());

    let area = centered_rect(50, 7, frame.area());

    frame.render_widget(Clear, area);

    let title = format!(" Set Password: {} ", server_name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().fg(Color::White));

    let masked = app.credential_input.masked_display();
    let password_line = format!("   > {}│", masked);

    let lines = vec![
        Line::from(""),
        Line::from(vec![Span::raw("   Password:")]),
        Line::from(vec![Span::raw(password_line)]),
        Line::from(""),
        Line::from(vec![Span::raw("   Enter: save   Esc: cancel")]),
    ];

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

pub fn handle_key_menu(app: &mut AppState, name: String, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('s') | KeyCode::Char('S') => {
            app.credential_input.clear();
            app.view = View::CredentialInput(name);
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            match crate::credentials::delete_credential(&name) {
                Ok(()) => {
                    app.notification = Some((
                        format!("Credential deleted for '{}'", name),
                        std::time::Instant::now(),
                    ));
                }
                Err(e) => {
                    let msg = format!("Couldn't delete credential: {}", e);
                    app.view = View::Error {
                        message: msg,
                        underlying: Box::new(View::Dashboard),
                    };
                    return false;
                }
            }
            app.view = View::Dashboard;
        }
        KeyCode::Esc => {
            app.view = View::Dashboard;
        }
        _ => {}
    }
    false
}

pub fn handle_key_input(app: &mut AppState, name: String, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.credential_input.push(c);
        }
        KeyCode::Backspace => {
            app.credential_input.pop();
        }
        KeyCode::Enter => {
            let password = app.credential_input.value.clone();
            app.credential_input.clear();
            match crate::credentials::set_credential(&name, &password) {
                Ok(()) => {
                    app.notification = Some((
                        format!("Credential saved for '{}'", name),
                        std::time::Instant::now(),
                    ));
                    app.view = View::Dashboard;
                }
                Err(e) => {
                    let msg = format!("Couldn't save credential: {}", e);
                    app.view = View::Error {
                        message: msg,
                        underlying: Box::new(View::Dashboard),
                    };
                }
            }
        }
        KeyCode::Esc => {
            app.credential_input.clear();
            app.view = View::CredentialMenu(name);
        }
        _ => {}
    }
    false
}
