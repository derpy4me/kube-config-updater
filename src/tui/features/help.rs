use crossterm::event::KeyEvent;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Frame,
};

use crate::tui::app::{AppState, View};
use super::{centered_rect, render_dim_background};

pub fn render(frame: &mut Frame, app: &mut AppState) {
    render_dim_background(frame, frame.area());

    let popup_height = (frame.area().height.saturating_sub(4)).min(36);
    let area = centered_rect(65, popup_height, frame.area());

    frame.render_widget(Clear, area);

    let bold = if app.use_color {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let dim = if app.use_color {
        Style::default().add_modifier(Modifier::DIM)
    } else {
        Style::default()
    };

    let mut lines: Vec<Line> = Vec::new();

    // ── Dashboard ──────────────────────────────────────────────────────────
    lines.push(Line::from(vec![Span::styled(" Dashboard", bold)]));
    for (keys, desc) in &[
        ("j / ↓      ", "Move down"),
        ("k / ↑      ", "Move up"),
        ("g          ", "Go to first"),
        ("G          ", "Go to last"),
        ("Enter      ", "Open detail view"),
        ("f          ", "Force fetch selected server"),
        ("F          ", "Force fetch all servers"),
        ("c          ", "Manage credentials"),
        ("a          ", "Add server (wizard)"),
        ("D          ", "Delete selected server"),
        ("d          ", "Toggle dry-run mode"),
        ("e          ", "Edit config in $EDITOR"),
        ("?          ", "Show this help"),
        ("q/^C/^D    ", "Quit"),
    ] {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::raw(*keys),
            Span::raw(*desc),
        ]));
    }

    lines.push(Line::raw(""));

    // ── Detail View ───────────────────────────────────────────────────────
    lines.push(Line::from(vec![Span::styled(" Detail View", bold)]));
    for (keys, desc) in &[
        ("Esc / q    ", "Back to dashboard"),
        ("f          ", "Force fetch this server"),
        ("p          ", "Probe server cert (read-only SSH check)"),
        ("c          ", "Manage credentials"),
        ("?          ", "Show this help"),
    ] {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::raw(*keys),
            Span::raw(*desc),
        ]));
    }

    lines.push(Line::raw(""));

    // ── Wizard ────────────────────────────────────────────────────────────
    lines.push(Line::from(vec![Span::styled(" Wizard", bold)]));
    for (keys, desc) in &[
        ("Enter      ", "Next step / confirm"),
        ("Esc        ", "Previous step / cancel"),
        ("q          ", "Cancel wizard"),
    ] {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::raw(*keys),
            Span::raw(*desc),
        ]));
    }

    lines.push(Line::raw(""));

    // ── Authentication Step ───────────────────────────────────────────────
    lines.push(Line::from(vec![Span::styled(" Authentication Step", bold)]));
    for (keys, desc) in &[
        ("p          ", "Password mode"),
        ("i          ", "Identity file mode"),
        ("t          ", "Test connection"),
        ("s          ", "Save server (after test passes)"),
    ] {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::raw(*keys),
            Span::raw(*desc),
        ]));
    }

    lines.push(Line::raw(""));

    // ── Any view ──────────────────────────────────────────────────────────
    lines.push(Line::from(vec![Span::styled(" Any view", bold)]));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::raw("?          "),
        Span::raw("Show this help"),
    ]));

    lines.push(Line::raw(""));

    // ── Footer ────────────────────────────────────────────────────────────
    lines.push(Line::from(vec![Span::styled(
        "  [press any key to dismiss]",
        dim,
    )]));

    let block = Block::default()
        .title("─ Help ─")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

pub fn handle_key(app: &mut AppState, _key: KeyEvent) {
    app.view = app
        .prior_view
        .take()
        .map(|b| *b)
        .unwrap_or(View::Dashboard);
}
