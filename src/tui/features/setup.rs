use std::sync::mpsc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Style},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::tui::app::{AppEvent, AppState, SetupStep, SetupWizardState, View, WizardState};
use super::centered_rect;

pub fn render(frame: &mut Frame, app: &AppState, wizard: &SetupWizardState) {
    let area = frame.area();
    let popup_area = centered_rect(
        area.width.saturating_sub(4).min(72),
        area.height.saturating_sub(4).min(16),
        area,
    );
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
        frame.render_widget(Paragraph::new(format!("  {}", err)).style(style).wrap(Wrap { trim: true }), rows[3]);
    }

    let hints = match wizard.step {
        SetupStep::OutputDir => "  Enter:next  Ctrl+C:quit  (required)",
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
        Paragraph::new(format!("  {}", hint)).wrap(Wrap { trim: true }),
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

/// Escape a string value for inclusion in a TOML basic (double-quoted) string.
fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn build_config_toml(ws: &SetupWizardState) -> String {
    let mut toml = format!("local_output_dir = \"{}\"\n", toml_escape(ws.output_dir.trim()));
    if !ws.default_user.trim().is_empty() {
        toml.push_str(&format!("default_user = \"{}\"\n", toml_escape(ws.default_user.trim())));
    }
    if !ws.default_file_path.trim().is_empty() {
        toml.push_str(&format!("default_file_path = \"{}\"\n", toml_escape(ws.default_file_path.trim())));
    }
    if !ws.default_file_name.trim().is_empty() {
        toml.push_str(&format!("default_file_name = \"{}\"\n", toml_escape(ws.default_file_name.trim())));
    }
    toml
}

fn setup_write(app: &mut AppState, ws: &SetupWizardState) {
    let toml = build_config_toml(ws);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{SetupStep, SetupWizardState};

    // ── Step navigation ───────────────────────────────────────────────────────

    #[test]
    fn test_setup_step_forward_sequence() {
        assert_eq!(SetupStep::OutputDir.next(), Some(SetupStep::DefaultUser));
        assert_eq!(SetupStep::DefaultUser.next(), Some(SetupStep::DefaultFilePath));
        assert_eq!(SetupStep::DefaultFilePath.next(), Some(SetupStep::DefaultFileName));
        assert_eq!(SetupStep::DefaultFileName.next(), None);
    }

    #[test]
    fn test_setup_step_backward_sequence() {
        assert_eq!(SetupStep::OutputDir.prev(), None);
        assert_eq!(SetupStep::DefaultUser.prev(), Some(SetupStep::OutputDir));
        assert_eq!(SetupStep::DefaultFilePath.prev(), Some(SetupStep::DefaultUser));
        assert_eq!(SetupStep::DefaultFileName.prev(), Some(SetupStep::DefaultFilePath));
    }

    #[test]
    fn test_setup_step_index_matches_order() {
        assert_eq!(SetupStep::OutputDir.index(), 0);
        assert_eq!(SetupStep::DefaultUser.index(), 1);
        assert_eq!(SetupStep::DefaultFilePath.index(), 2);
        assert_eq!(SetupStep::DefaultFileName.index(), 3);
    }

    // ── Validation ────────────────────────────────────────────────────────────

    #[test]
    fn test_validate_rejects_empty_output_dir() {
        let ws = SetupWizardState::default(); // output_dir is ""
        assert!(validate(&ws).is_some(), "empty output_dir must be rejected");
    }

    #[test]
    fn test_validate_rejects_whitespace_only_output_dir() {
        let ws = SetupWizardState { output_dir: "   ".to_string(), ..Default::default() };
        assert!(validate(&ws).is_some());
    }

    #[test]
    fn test_validate_accepts_non_empty_output_dir() {
        let ws = SetupWizardState { output_dir: "/home/user/.kube".to_string(), ..Default::default() };
        assert!(validate(&ws).is_none());
    }

    #[test]
    fn test_validate_optional_steps_always_pass_when_blank() {
        let base = SetupWizardState {
            output_dir: "/tmp/kube".to_string(),
            ..Default::default()
        };

        for step in [SetupStep::DefaultUser, SetupStep::DefaultFilePath, SetupStep::DefaultFileName] {
            let ws = SetupWizardState { step, ..base.clone() };
            assert!(validate(&ws).is_none(), "blank optional step should pass validation");
        }
    }

    // ── TOML generation ───────────────────────────────────────────────────────

    #[test]
    fn test_build_config_toml_minimal() {
        let ws = SetupWizardState {
            output_dir: "/home/user/.kube".to_string(),
            ..Default::default()
        };
        let toml = build_config_toml(&ws);
        assert!(toml.contains("local_output_dir = \"/home/user/.kube\""));
        assert!(!toml.contains("default_user"));
        assert!(!toml.contains("default_file_path"));
        assert!(!toml.contains("default_file_name"));
    }

    #[test]
    fn test_build_config_toml_with_all_defaults() {
        let ws = SetupWizardState {
            output_dir: "/home/user/.kube".to_string(),
            default_user: "ubuntu".to_string(),
            default_file_path: "/etc/rancher/k3s".to_string(),
            default_file_name: "k3s.yaml".to_string(),
            ..Default::default()
        };
        let toml = build_config_toml(&ws);
        assert!(toml.contains("local_output_dir = \"/home/user/.kube\""));
        assert!(toml.contains("default_user = \"ubuntu\""));
        assert!(toml.contains("default_file_path = \"/etc/rancher/k3s\""));
        assert!(toml.contains("default_file_name = \"k3s.yaml\""));
    }

    #[test]
    fn test_build_config_toml_trims_whitespace() {
        let ws = SetupWizardState {
            output_dir: "  /tmp/kube  ".to_string(),
            default_user: "  ubuntu  ".to_string(),
            ..Default::default()
        };
        let toml = build_config_toml(&ws);
        assert!(toml.contains("\"/tmp/kube\""), "output_dir should be trimmed");
        assert!(toml.contains("\"ubuntu\""), "default_user should be trimmed");
    }

    #[test]
    fn test_toml_escape_handles_double_quote() {
        assert_eq!(toml_escape(r#"path/with "quotes""#), r#"path/with \"quotes\""#);
    }

    #[test]
    fn test_toml_escape_handles_backslash() {
        assert_eq!(toml_escape(r"C:\Users\foo"), r"C:\\Users\\foo");
    }

    #[test]
    fn test_toml_escape_clean_path_unchanged() {
        assert_eq!(toml_escape("/home/user/.kube"), "/home/user/.kube");
    }

    #[test]
    fn test_build_config_toml_path_with_quotes_round_trips() {
        use tempfile::NamedTempFile;
        // Paths with double-quotes were the exact bug seen in the screenshot
        let ws = SetupWizardState {
            output_dir: r#"/home/user/my "special" dir"#.to_string(),
            ..Default::default()
        };
        let tmp = NamedTempFile::new().expect("temp file");
        std::fs::write(tmp.path(), build_config_toml(&ws)).expect("write");
        let config = crate::config::load_config(tmp.path().to_str().unwrap())
            .expect("path with quotes must survive round-trip");
        assert_eq!(config.local_output_dir, r#"/home/user/my "special" dir"#);
    }

    #[test]
    fn test_build_config_toml_path_with_backslash_round_trips() {
        use tempfile::NamedTempFile;
        let ws = SetupWizardState {
            output_dir: r"C:\Users\foo\.kube".to_string(),
            ..Default::default()
        };
        let tmp = NamedTempFile::new().expect("temp file");
        std::fs::write(tmp.path(), build_config_toml(&ws)).expect("write");
        let config = crate::config::load_config(tmp.path().to_str().unwrap())
            .expect("path with backslashes must survive round-trip");
        assert_eq!(config.local_output_dir, r"C:\Users\foo\.kube");
    }

    #[test]
    fn test_build_config_toml_round_trips_through_load_config() {
        use tempfile::NamedTempFile;

        let ws = SetupWizardState {
            output_dir: "/tmp/kube".to_string(),
            default_user: "ubuntu".to_string(),
            default_file_path: "/etc/rancher/k3s".to_string(),
            default_file_name: "k3s.yaml".to_string(),
            ..Default::default()
        };

        let tmp = NamedTempFile::new().expect("temp file");
        std::fs::write(tmp.path(), build_config_toml(&ws)).expect("write");

        let config = crate::config::load_config(tmp.path().to_str().unwrap())
            .expect("setup wizard TOML should parse cleanly");

        assert_eq!(config.local_output_dir, "/tmp/kube");
        assert_eq!(config.default_user.as_deref(), Some("ubuntu"));
        assert_eq!(config.default_file_path.as_deref(), Some("/etc/rancher/k3s"));
        assert_eq!(config.default_file_name.as_deref(), Some("k3s.yaml"));
        assert!(config.servers.is_empty(), "fresh config must have no servers");
    }

    #[test]
    fn test_build_config_toml_minimal_round_trips() {
        use tempfile::NamedTempFile;

        let ws = SetupWizardState {
            output_dir: "/tmp/kube".to_string(),
            ..Default::default()
        };

        let tmp = NamedTempFile::new().expect("temp file");
        std::fs::write(tmp.path(), build_config_toml(&ws)).expect("write");

        let config = crate::config::load_config(tmp.path().to_str().unwrap())
            .expect("minimal setup TOML must parse");

        assert_eq!(config.local_output_dir, "/tmp/kube");
        assert!(config.default_user.is_none());
        assert!(config.servers.is_empty());
    }
}
