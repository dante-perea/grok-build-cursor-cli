//! Ratatui multi-pane render for the Cursor-like shell.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use crate::diff_review::ChangeDecision;
use crate::layout::FocusPane;
use crate::session::CursorSession;

/// Draw the full multi-pane Cursor shell into a frame.
pub fn draw_session(frame: &mut Frame, session: &CursorSession) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);

    draw_body(frame, session, chunks[0]);
    draw_status(frame, session, chunks[1]);
}

fn draw_body(frame: &mut Frame, session: &CursorSession, area: Rect) {
    let snap = session.layout.snapshot();
    let mut constraints = Vec::new();
    if snap.show_workspace {
        constraints.push(Constraint::Percentage(session.layout.splits.workspace_pct));
    }
    constraints.push(Constraint::Percentage(session.layout.splits.chat_pct));
    if snap.show_side {
        constraints.push(Constraint::Percentage(session.layout.splits.side_pct));
    }

    // Normalize if toggles hide columns — ratatui will stretch remaining.
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    let mut idx = 0;
    if snap.show_workspace {
        draw_workspace(frame, session, cols[idx]);
        idx += 1;
    }
    draw_chat_column(frame, session, cols[idx]);
    idx += 1;
    if snap.show_side {
        draw_side_column(frame, session, cols[idx]);
    }
}

fn pane_block(title: &str, focused: bool) -> Block<'static> {
    let style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(style)
        .title(title.to_string())
}

fn draw_workspace(frame: &mut Frame, session: &CursorSession, area: Rect) {
    let focused = session.layout.focus == FocusPane::Workspace;
    let block = pane_block(" Workspace ", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let items: Vec<ListItem> = session
        .workspace
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let name = f
                .path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("?");
            let prefix = if f.is_dir { "📁 " } else { "   " };
            let marker = if i == session.workspace.selected {
                "› "
            } else {
                "  "
            };
            ListItem::new(format!("{marker}{prefix}{name}"))
        })
        .collect();

    let list = List::new(items).highlight_style(Style::default().fg(Color::Yellow));
    frame.render_widget(list, inner);

    // Editor preview strip at bottom of workspace if a file is open.
    if let Some(path) = &session.workspace.open_path {
        if area.height > 8 {
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(inner);
            let preview_lines: Vec<Line> = session
                .workspace
                .buffer
                .lines()
                .take(split[1].height as usize)
                .map(|l| Line::from(l.to_string()))
                .collect();
            let title = format!(" Editor · {} ", path.display());
            let p = Paragraph::new(preview_lines)
                .block(Block::default().borders(Borders::TOP).title(title))
                .wrap(Wrap { trim: false });
            frame.render_widget(p, split[1]);
        }
    }
}

fn draw_chat_column(frame: &mut Frame, session: &CursorSession, area: Rect) {
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(5)])
        .split(area);

    // Chat transcript
    let chat_focus = session.layout.focus == FocusPane::Chat;
    let block = pane_block(" Agent Chat ", chat_focus);
    let inner = block.inner(split[0]);
    frame.render_widget(block, split[0]);

    let mut lines: Vec<Line> = Vec::new();
    for msg in &session.chat.messages {
        let (role, color) = match msg.role {
            crate::chat::ChatRole::User => ("You", Color::Green),
            crate::chat::ChatRole::Assistant => ("Grok", Color::Cyan),
            crate::chat::ChatRole::System => ("System", Color::Yellow),
        };
        let stream = if msg.streaming { " …" } else { "" };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{role}{stream}"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(""),
        ]));
        for line in msg.content.lines() {
            lines.push(Line::from(format!("  {line}")));
        }
        lines.push(Line::from(""));
    }
    let p = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(p, inner);

    // Composer
    let comp_focus = session.layout.focus == FocusPane::Composer;
    let title = if session.composer.turn_in_flight {
        " Composer (busy — Enter queues) "
    } else {
        " Composer "
    };
    let block = pane_block(title, comp_focus);
    let inner = block.inner(split[1]);
    frame.render_widget(block, split[1]);

    let text = if session.composer.draft.is_empty() {
        Line::from(Span::styled(
            session.composer.placeholder.clone(),
            Style::default().fg(Color::DarkGray),
        ))
    } else {
        Line::from(session.composer.draft.clone())
    };
    let p = Paragraph::new(text).wrap(Wrap { trim: false });
    frame.render_widget(p, inner);
}

fn draw_side_column(frame: &mut Frame, session: &CursorSession, area: Rect) {
    let show_diff = session.layout.show_diff_review;
    let constraints = if show_diff {
        vec![Constraint::Percentage(50), Constraint::Percentage(50)]
    } else {
        vec![Constraint::Percentage(100)]
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    // Activity
    let act_focus = session.layout.focus == FocusPane::Activity;
    let block = pane_block(" Activity ", act_focus);
    let inner = block.inner(rows[0]);
    frame.render_widget(block, rows[0]);

    let items: Vec<ListItem> = session
        .activity
        .entries
        .iter()
        .rev()
        .take(inner.height as usize)
        .map(|e| {
            let icon = match e.status {
                crate::activity::ActivityStatus::Running => "●",
                crate::activity::ActivityStatus::Completed => "✓",
                crate::activity::ActivityStatus::Failed => "✗",
                crate::activity::ActivityStatus::Pending => "○",
                crate::activity::ActivityStatus::Cancelled => "–",
            };
            let tool = e
                .tool_name
                .as_ref()
                .map(|t| format!(" [{t}]"))
                .unwrap_or_default();
            ListItem::new(format!("{icon} {}{tool}", e.title))
        })
        .collect();
    frame.render_widget(List::new(items), inner);

    if show_diff && rows.len() > 1 {
        draw_diff_review(frame, session, rows[1]);
    }
}

fn draw_diff_review(frame: &mut Frame, session: &CursorSession, area: Rect) {
    let focused = session.layout.focus == FocusPane::DiffReview;
    let pending = session.diffs.pending_count();
    let title = format!(" Diff Review ({pending} pending) ");
    let block = pane_block(&title, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if session.diffs.items.is_empty() {
        let p = Paragraph::new("No proposed changes yet.")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(p, inner);
        return;
    }

    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(inner);

    let items: Vec<ListItem> = session
        .diffs
        .items
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mark = if i == session.diffs.selected {
                "›"
            } else {
                " "
            };
            let dec = match c.decision {
                ChangeDecision::Pending => "·",
                ChangeDecision::Accepted => "✓",
                ChangeDecision::Rejected => "✗",
            };
            ListItem::new(format!(
                "{mark} {dec} {} ({})",
                c.path.display(),
                c.summary
            ))
        })
        .collect();
    frame.render_widget(List::new(items), split[0]);

    if let Some(item) = session.diffs.selected_item() {
        let preview = item.inspect_preview();
        let p = Paragraph::new(preview)
            .block(Block::default().borders(Borders::TOP).title(" Inspect "))
            .wrap(Wrap { trim: false });
        frame.render_widget(p, split[1]);
    }
}

fn draw_status(frame: &mut Frame, session: &CursorSession, area: Rect) {
    let help = " Tab:focus │ Enter:submit │ a/r:accept/reject │ Ctrl+C:quit ";
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", session.status_line),
            Style::default().fg(Color::Black).bg(Color::Cyan),
        ),
        Span::raw(help),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// Headless layout dump for verification (no alternate screen).
pub fn dump_layout_text(session: &CursorSession) -> String {
    let mut out = session.layout_snapshot().dump();
    out.push_str("\n--- panes content summary ---\n");
    out.push_str(&format!(
        "workspace_files: {}\n",
        session.workspace.files.len()
    ));
    out.push_str(&format!(
        "chat_messages: {}\n",
        session.chat.messages.len()
    ));
    out.push_str(&format!(
        "composer_draft_len: {}\n",
        session.composer.draft.len()
    ));
    out.push_str(&format!(
        "activity_entries: {}\n",
        session.activity.entries.len()
    ));
    out.push_str(&format!(
        "diff_items: {} (pending {})\n",
        session.diffs.items.len(),
        session.diffs.pending_count()
    ));
    out.push_str(&format!("agent_busy: {}\n", session.agent_busy));
    out.push_str(&format!("status: {}\n", session.status_line));
    out
}
