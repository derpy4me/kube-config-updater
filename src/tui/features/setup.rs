use std::sync::mpsc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Style},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Frame,
};

use crate::tui::app::{AppEvent, AppState, SetupStep, SetupWizardState, View, WizardState};
use super::centered_rect;

pub fn render(frame: &mut Frame, app: &AppState, wizard: &SetupWizardState) {
    let popup_area = centered_rect(72, 16, frame.area());
    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Initial Setup ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let rows = Layout::vertical([
        Constraint::Length(1), // step indicator
        Constraint::Length(1), // separator
        Constraint::Fill(1),   // content
        Constraint::Length(1), // error row
        Constraint::Length(1), // footer
    ])
    .split(inner);

    render_step_indicator(frame, wizard, rows[0]);

    let sep = Paragraph::new("  ────────────────────────────────────────────────────────");
    frame.render_widget(sep, rows[1]);

    render_content(frame, wizard, rows[2]);

    if let Some(ref err) = wizard.error {
        let style = if app.use_color {
            Style::default().fg(Color::Red)
        } else {
            Style::default()
        };
        frame.render_widget(Paragraph::new(format!("  {}", err)).style(style), rows[3]);
    }

    let hints = match wizard.step {
        SetupStep::OutputDir => "  Enter:next  (required)",
        _ => "  Enter:next  Esc:back  (leave blank for no default)",
    };
    frame.render_widget(Paragraph::new(hints), rows[4]);
}

fn render_step_indicator(frame: &mut Frame, wizard: &SetupWizardState, area: ratatui::layout::Rect) {
    let current_idx = wizard.step.index();
    let total = 4usize;

    let label = format!("  Step {} of {} — {}   ", current_idx + 1, total, wizard.step.label());

    let dots: String = (0..total)
        .map(|i| {
            if i < current_idx { '●' } else if i == current_idx { '◉' } else { '○' }
        })
        .collect();

    let dots_len = dots.chars().count() as u16;
    let cols = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(dots_len),
    ])
    .split(area);

    frame.render_widget(Paragraph::new(label), cols[0]);
    frame.render_widget(Paragraph::new(dots), cols[1]);
}

fn render_content(frame: &mut Frame, wizard: &SetupWizardState, area: ratatui::layout::Rect) {
    let (field_label, value, hint) = match wizard.step {
        SetupStep::OutputDir => (
            "Local output directory",
            wizard.output_dir.as_str(),
            "Directory where fetched kubeconfigs are written  (e.g. /home/user/.kube)",
        ),
        SetupStep::DefaultUser => (
            "Default SSH user",
            wizard.default_user.as_str(),
            "SSH user for all servers unless overridden  (common: ubuntu, root)",
        ),
        SetupStep::DefaultFilePath => (
            "Default remote file path",
            wizard.default_file_path.as_str(),
            "Remote directory unless overridden  (k3s default: /etc/rancher/k3s)",
        ),
        SetupStep::DefaultFileName => (
            "Default remote file name",
            wizard.default_file_name.as_str(),
            "Remote filename unless overridden  (k3s default: k3s.yaml)",
        ),
    };

    let content_rows = Layout::vertical([
        Constraint::Length(1), // field label
        Constraint::Length(1), // input line
        Constraint::Length(1), // blank
        Constraint::Fill(1),   // hint
    ])
    .split(area);

    frame.render_widget(
        Paragraph::new(format!("  {}:", field_label)),
        content_rows[0],
    );
    frame.render_widget(
        Paragraph::new(format!("  > {}│", value)),
        content_rows[1],
    );
    frame.render_widget(
        Paragraph::new(format!("  {}", hint)),
        content_rows[3],
    );
}

pub fn handle_key(
    app: &mut AppState,
    key: KeyEvent,
    _tx: &mpsc::Sender<AppEvent>,
) -> bool {
    let ws = match &app.view {
        View::SetupWizard(ws) => ws.clone(),
        _ => return false,
    };

    match key.code {
        KeyCode::Esc => {
            match ws.step.prev() {
                Some(prev) => {
                    let mut ws = ws;
                    ws.step = prev;
                    ws.error = None;
                    app.view = View::SetupWizard(ws);
                }
                None => {
                    // First step — can't go back; no config exists yet
                    let mut ws = ws;
                    ws.error = Some("No config file exists yet — complete setup to continue.".to_string());
                    app.view = View::SetupWizard(ws);
                }
            }
        }
        KeyCode::Enter => {
            let mut ws = ws;
            if let Some(err) = validate(&ws) {
                ws.error = Some(err);
                app.view = View::SetupWizard(ws);
            } else if let Some(next) = ws.step.next() {
                ws.error = None;
                ws.step = next;
                app.view = View::SetupWizard(ws);
            } else {
                // Final step: write config and transition to the add-server wizard
                setup_write(app, &ws);
            }
        }
        KeyCode::Backspace => {
            let mut ws = ws;
            match ws.step {
                SetupStep::OutputDir => { ws.output_dir.pop(); }
                SetupStep::DefaultUser => { ws.default_user.pop(); }
                SetupStep::DefaultFilePath => { ws.default_file_path.pop(); }
                SetupStep::DefaultFileName => { ws.default_file_name.pop(); }
            };
            ws.error = None;
            app.view = View::SetupWizard(ws);
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let mut ws = ws;
            match ws.step {
                SetupStep::OutputDir => ws.output_dir.push(c),
                SetupStep::DefaultUser => ws.default_user.push(c),
                SetupStep::DefaultFilePath => ws.default_file_path.push(c),
                SetupStep::DefaultFileName => ws.default_file_name.push(c),
            }
            ws.error = None;
            app.view = View::SetupWizard(ws);
        }
        _ => {}
    }
    false
}

fn validate(ws: &SetupWizardState) -> Option<String> {
    if ws.step == SetupStep::OutputDir && ws.output_dir.trim().is_empty() {
        return Some("Output directory is required".to_string());
    }
    None
}

fn setup_write(app: &mut AppState, ws: &SetupWizardState) {
    let output_dir = ws.output_dir.trim();

    let mut toml = format!("local_output_dir = \"{}\"\n", output_dir);
    if !ws.default_user.trim().is_empty() {
        toml.push_str(&format!("default_user = \"{}\"\n", ws.default_user.trim()));
    }
    if !ws.default_file_path.trim().is_empty() {
        toml.push_str(&format!("default_file_path = \"{}\"\n", ws.default_file_path.trim()));
    }
    if !ws.default_file_name.trim().is_empty() {
        toml.push_str(&format!("default_file_name = \"{}\"\n", ws.default_file_name.trim()));
    }

    if let Err(e) = std::fs::write(&app.config_path, &toml) {
        let mut ws_err = ws.clone();
        ws_err.error = Some(format!("Could not write config: {}", e));
        app.view = View::SetupWizard(ws_err);
        return;
    }

    match crate::config::load_config(app.config_path.to_str().unwrap_or_default()) {
        Ok(config) => {
            app.config = config;
            app.view = View::Wizard(WizardState::new());
        }
        Err(e) => {
            let mut ws_err = ws.clone();
            ws_err.error = Some(format!("Config written but failed to load: {}", e));
            app.view = View::SetupWizard(ws_err);
        }
    }
}
