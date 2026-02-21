pub mod credentials;
pub mod dashboard;
pub mod detail;
pub mod help;
pub mod setup;
pub mod wizard;

use ratatui::{
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::Block,
    Frame,
};

use crate::state::RunStatus;

// ─── Layout Helpers ───────────────────────────────────────────────────────────

/// Return a rect of the given size centered within `area`.
pub fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .split(area)[0];
    Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .split(vertical)[0]
}

// ─── Color Helpers ────────────────────────────────────────────────────────────

/// Returns the style for a server's run status.
pub fn status_color(status: &RunStatus, use_color: bool) -> Style {
    if !use_color {
        return Style::default();
    }
    match status {
        RunStatus::Fetched => Style::default().fg(Color::Green),
        RunStatus::Skipped => Style::default().fg(Color::DarkGray),
        RunStatus::NoCredential | RunStatus::AuthRejected => Style::default().fg(Color::Yellow),
        RunStatus::Failed => Style::default().fg(Color::Red),
    }
}

/// Returns the style for a cert expiry date.
pub fn cert_color(expires_at: Option<&chrono::DateTime<chrono::Utc>>, use_color: bool) -> Style {
    if !use_color {
        return Style::default();
    }
    match expires_at {
        None => Style::default().fg(Color::Red),
        Some(exp) => {
            let days = (*exp - chrono::Utc::now()).num_days();
            if days > 30 {
                Style::default().fg(Color::Green)
            } else if days > 0 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Red)
            }
        }
    }
}

// ─── Overlay Dimming ─────────────────────────────────────────────────────────

/// Renders a dim block over the full area to visually push background content back.
/// Call this before rendering any overlay widget.
pub fn render_dim_background(frame: &mut Frame, area: Rect) {
    let block = Block::default().style(Style::default().add_modifier(Modifier::DIM));
    frame.render_widget(block, area);
}

// ─── Status Display Text ─────────────────────────────────────────────────────

/// Returns the display string for a server's run status icon + label.
pub fn status_display(status: &RunStatus) -> &'static str {
    match status {
        RunStatus::Fetched => "✓ Fetched",
        RunStatus::Skipped => "— Skipped",
        RunStatus::NoCredential => "⚠ No credential",
        RunStatus::AuthRejected => "⚠ Auth rejected",
        RunStatus::Failed => "✗ Failed",
    }
}

/// Returns a formatted cert expiry string for display in the dashboard table.
pub fn cert_expires_display(expires_at: Option<&chrono::DateTime<chrono::Utc>>) -> String {
    match expires_at {
        None => "—".to_string(),
        Some(exp) => exp.format("%Y-%m-%d").to_string(),
    }
}
