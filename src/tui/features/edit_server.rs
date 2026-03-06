use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph},
};

use super::centered_rect;
use crate::tui::app::{AppState, EditServerState, View};

pub fn render(frame: &mut Frame, app: &AppState, state: &EditServerState) {
    let area = frame.area();
    let popup = centered_rect(area.width.saturating_sub(6).min(68), 16, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(format!(" Edit Server: {} ", state.server_name))
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(BorderType::Rounded);

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let rows = Layout::vertical([
        Constraint::Length(1), // header hint
        Constraint::Length(1), // separator
        Constraint::Length(7), // 7 fields
        Constraint::Length(1), // blank
        Constraint::Length(1), // error
        Constraint::Length(1), // footer
    ])
    .split(inner);

    frame.render_widget(
        Paragraph::new("  Use Tab/↑↓ to move between fields").style(Style::default().add_modifier(Modifier::DIM)),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new("  ──────────────────────────────────────────────────────────"),
        rows[1],
    );

    let field_rows = Layout::vertical([Constraint::Length(1); 7]).split(rows[2]);
    for (i, (label, value)) in EditServerState::LABELS.iter().zip(state.fields.iter()).enumerate() {
        let focused = i == state.field_idx;
        let label_text = format!("  {:<18}", format!("{}:", label));
        let value_display = if focused {
            format!("{}│", value)
        } else {
            value.clone()
        };
        let value_style = if focused && app.use_color {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else if focused {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(label_text),
                Span::styled(value_display, value_style),
            ])),
            field_rows[i],
        );
    }

    if let Some(ref err) = state.error {
        let style = if app.use_color {
            Style::default().fg(Color::Red)
        } else {
            Style::default()
        };
        frame.render_widget(Paragraph::new(format!("  {}", err)).style(style), rows[4]);
    }

    frame.render_widget(
        Paragraph::new("  Ctrl+S:save  Esc:cancel").style(Style::default().add_modifier(Modifier::DIM)),
        rows[5],
    );
}

pub fn handle_key(app: &mut AppState, key: KeyEvent) -> bool {
    let mut state = match &app.view {
        View::EditServer(s) => s.clone(),
        _ => return false,
    };

    let num_fields = EditServerState::LABELS.len();

    match key.code {
        KeyCode::Esc => {
            let server_name = state.server_name.clone();
            app.view = View::Detail(server_name);
        }
        KeyCode::Tab | KeyCode::Down => {
            state.field_idx = (state.field_idx + 1) % num_fields;
            app.view = View::EditServer(state);
        }
        KeyCode::BackTab | KeyCode::Up => {
            state.field_idx = state.field_idx.checked_sub(1).unwrap_or(num_fields - 1);
            app.view = View::EditServer(state);
        }
        KeyCode::Backspace => {
            state.fields[state.field_idx].pop();
            state.error = None;
            app.view = View::EditServer(state);
        }
        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            save(app, state);
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.fields[state.field_idx].push(c);
            state.error = None;
            app.view = View::EditServer(state);
        }
        _ => {}
    }
    false
}

fn save(app: &mut AppState, state: EditServerState) {
    if state.fields[0].trim().is_empty() {
        let mut s = state;
        s.field_idx = 0;
        s.error = Some("Address is required".to_string());
        app.view = View::EditServer(s);
        return;
    }
    if state.fields[1].trim().is_empty() {
        let mut s = state;
        s.field_idx = 1;
        s.error = Some("Cluster IP is required".to_string());
        app.view = View::EditServer(s);
        return;
    }

    let updated = state.to_server();
    match crate::config::update_server(&app.config_path, &updated) {
        Ok(()) => {
            if let Some(s) = app.config.servers.iter_mut().find(|s| s.name == updated.name) {
                *s = updated.clone();
            }
            app.notification = Some((
                format!("Saved changes to '{}'", updated.name),
                std::time::Instant::now(),
            ));
            app.view = View::Detail(updated.name);
        }
        Err(e) => {
            let mut s = state;
            s.error = Some(format!("Save failed: {}", e));
            app.view = View::EditServer(s);
        }
    }
}
