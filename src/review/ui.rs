//! The terminal side of `review`.
//!
//! Kept apart from the decision logic so the rules about what gets redacted
//! can be tested without a terminal to draw into.

use std::io::Stdout;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span as TextSpan};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use super::Document;
use crate::config::Config;
use crate::vault::Vault;

/// What the user asked for on leaving a document.
enum Outcome {
    /// Write this document and move to the next.
    Write,
    /// Leave this document alone and move to the next.
    Skip,
    /// Stop, writing nothing further.
    Quit,
}

/// Reviews each file in turn, writing those the user confirms.
///
/// # Errors
///
/// Returns an error if the terminal cannot be prepared, a file cannot be
/// read, or a confirmed document cannot be written.
pub fn run(
    files: &[PathBuf],
    config: &Config,
    vault: &mut Vault,
    output_dir: Option<&Path>,
) -> Result<()> {
    let mut written = Vec::new();

    for path in files {
        let mut document = Document::open(path, config)?;

        if document.decisions.is_empty() {
            eprintln!("{}: nothing detected, skipped", path.display());
            continue;
        }

        let mut terminal = start().context("preparing the terminal")?;
        let outcome = review_one(&mut terminal, &mut document);
        stop(&mut terminal).context("restoring the terminal")?;

        match outcome? {
            Outcome::Write => {
                let destination = document.write(vault, output_dir)?;
                written.push(destination);
            }
            Outcome::Skip => eprintln!("{}: skipped, nothing written", path.display()),
            Outcome::Quit => {
                eprintln!("stopped; {} file(s) written", written.len());
                return Ok(());
            }
        }
    }

    for path in &written {
        eprintln!("wrote {}", path.display());
    }
    Ok(())
}

type Screen = Terminal<CrosstermBackend<Stdout>>;

fn start() -> Result<Screen> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

/// Puts the terminal back, whatever happened.
///
/// Leaving raw mode on would make the user's shell unusable, so this runs
/// even when the review loop failed.
fn stop(terminal: &mut Screen) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn review_one(terminal: &mut Screen, document: &mut Document) -> Result<Outcome> {
    let mut selected = 0usize;
    let mut state = ListState::default();

    loop {
        state.select(Some(selected));
        terminal.draw(|frame| draw(frame, document, &mut state, selected))?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if let Some(outcome) = apply_key(key.code, document, &mut selected) {
            return Ok(outcome);
        }
    }
}

/// Applies a keypress, returning an outcome when the user is done.
///
/// Pure, so the rules about what each key does can be tested without a
/// terminal.
fn apply_key(code: KeyCode, document: &mut Document, selected: &mut usize) -> Option<Outcome> {
    let last = document.decisions.len().saturating_sub(1);
    match code {
        KeyCode::Char('j') | KeyCode::Down => *selected = (*selected + 1).min(last),
        KeyCode::Char('k') | KeyCode::Up => *selected = selected.saturating_sub(1),
        KeyCode::Char('g') | KeyCode::Home => *selected = 0,
        KeyCode::Char('G') | KeyCode::End => *selected = last,
        KeyCode::Char(' ') | KeyCode::Enter => {
            if let Some(decision) = document.decisions.get_mut(*selected) {
                decision.accepted = !decision.accepted;
            }
        }
        KeyCode::Char('a') => {
            for decision in &mut document.decisions {
                decision.accepted = true;
            }
        }
        KeyCode::Char('n') => {
            for decision in &mut document.decisions {
                decision.accepted = false;
            }
        }
        KeyCode::Char('w') => return Some(Outcome::Write),
        KeyCode::Char('s') => return Some(Outcome::Skip),
        KeyCode::Char('q') | KeyCode::Esc => return Some(Outcome::Quit),
        _ => {}
    }
    None
}

fn draw(frame: &mut ratatui::Frame, document: &Document, state: &mut ListState, selected: usize) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(5),
            Constraint::Length(3),
        ])
        .split(frame.area());

    draw_header(frame, areas[0], document);
    draw_list(frame, areas[1], document, state);
    draw_context(frame, areas[2], document, selected);
    draw_keys(frame, areas[3]);
}

fn draw_header(frame: &mut ratatui::Frame, area: Rect, document: &Document) {
    let accepted = document.accepted_count();
    let total = document.decisions.len();
    let title = format!(
        " {}  —  {accepted} of {total} will be redacted ",
        document.path.display()
    );
    frame.render_widget(
        Paragraph::new(title).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_list(frame: &mut ratatui::Frame, area: Rect, document: &Document, state: &mut ListState) {
    let items: Vec<ListItem> = document
        .decisions
        .iter()
        .map(|decision| {
            let (marker, style) = if decision.accepted {
                ("[x]", Style::default())
            } else {
                // Dimmed, because a rejected value stays in the output.
                ("[ ]", Style::default().fg(Color::DarkGray))
            };
            ListItem::new(Line::from(vec![
                TextSpan::styled(format!("{marker} "), style),
                TextSpan::styled(
                    format!("{:<9}", decision.span.kind.tag()),
                    style.add_modifier(Modifier::BOLD),
                ),
                TextSpan::styled(
                    format!("{:>5.0}%  ", decision.span.confidence * 100.0),
                    style,
                ),
                TextSpan::styled(decision.span.text.clone(), style),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" detections "))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    frame.render_stateful_widget(list, area, state);
}

fn draw_context(frame: &mut ratatui::Frame, area: Rect, document: &Document, selected: usize) {
    frame.render_widget(
        Paragraph::new(document.context(selected))
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title(" in context ")),
        area,
    );
}

fn draw_keys(frame: &mut ratatui::Frame, area: Rect) {
    frame.render_widget(
        Paragraph::new(
            "j/k move   space toggle   a accept all   n reject none   w write   s skip   q quit",
        )
        .block(Block::default().borders(Borders::ALL)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::Decision;
    use ratatui::backend::TestBackend;

    fn document(text: &str) -> Document {
        let decisions = crate::pipeline::detect(text, &Config::default())
            .expect("detecting")
            .into_iter()
            .map(|span| Decision {
                span,
                accepted: true,
            })
            .collect();
        Document {
            path: PathBuf::from("note.txt"),
            text: text.to_owned(),
            decisions,
        }
    }

    fn sample() -> Document {
        document("Call 06 12 34 56 78 or mail a@example.com.")
    }

    #[test]
    fn space_toggles_only_the_selected_detection() {
        let mut doc = sample();
        let mut selected = 0;
        apply_key(KeyCode::Char(' '), &mut doc, &mut selected);
        assert!(!doc.decisions[0].accepted);
        assert!(doc.decisions[1].accepted, "only the selection may change");
    }

    #[test]
    fn movement_stays_within_the_list() {
        let mut doc = sample();
        let mut selected = 0;
        apply_key(KeyCode::Char('k'), &mut doc, &mut selected);
        assert_eq!(selected, 0, "must not move above the first");
        for _ in 0..10 {
            apply_key(KeyCode::Char('j'), &mut doc, &mut selected);
        }
        assert_eq!(selected, 1, "must not move past the last");
    }

    #[test]
    fn accept_all_and_reject_all_cover_every_detection() {
        let mut doc = sample();
        let mut selected = 0;
        apply_key(KeyCode::Char('n'), &mut doc, &mut selected);
        assert_eq!(doc.accepted_count(), 0);
        apply_key(KeyCode::Char('a'), &mut doc, &mut selected);
        assert_eq!(doc.accepted_count(), doc.decisions.len());
    }

    #[test]
    fn write_skip_and_quit_end_the_review() {
        let mut doc = sample();
        let mut selected = 0;
        assert!(matches!(
            apply_key(KeyCode::Char('w'), &mut doc, &mut selected),
            Some(Outcome::Write)
        ));
        assert!(matches!(
            apply_key(KeyCode::Char('s'), &mut doc, &mut selected),
            Some(Outcome::Skip)
        ));
        assert!(matches!(
            apply_key(KeyCode::Char('q'), &mut doc, &mut selected),
            Some(Outcome::Quit)
        ));
        assert!(matches!(
            apply_key(KeyCode::Esc, &mut doc, &mut selected),
            Some(Outcome::Quit)
        ));
    }

    #[test]
    fn an_unbound_key_does_nothing() {
        let mut doc = sample();
        let mut selected = 0;
        let before = doc.accepted_count();
        assert!(apply_key(KeyCode::Char('z'), &mut doc, &mut selected).is_none());
        assert_eq!(doc.accepted_count(), before);
        assert_eq!(selected, 0);
    }

    #[test]
    fn keys_on_an_empty_document_do_not_panic() {
        let mut doc = document("nothing sensitive here");
        let mut selected = 0;
        for code in [
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Char('G'),
            KeyCode::Char(' '),
            KeyCode::Char('a'),
        ] {
            apply_key(code, &mut doc, &mut selected);
        }
        assert_eq!(selected, 0);
    }

    /// The screen must show what is at stake: the value, its kind, and
    /// whether it is going to be redacted.
    #[test]
    fn the_screen_shows_each_detection_and_its_state() {
        let doc = sample();
        let mut terminal = Terminal::new(TestBackend::new(90, 24)).expect("test terminal");
        let mut state = ListState::default();
        terminal
            .draw(|frame| draw(frame, &doc, &mut state, 0))
            .expect("drawing");

        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(rendered.contains("06 12 34 56 78"), "{rendered}");
        assert!(rendered.contains("PHONE"), "{rendered}");
        assert!(rendered.contains("[x]"), "accepted state must be visible");
        assert!(rendered.contains("2 of 2 will be redacted"), "{rendered}");
        assert!(rendered.contains("space toggle"), "keys must be shown");
    }

    #[test]
    fn a_rejected_detection_is_drawn_as_rejected() {
        let mut doc = sample();
        doc.decisions[0].accepted = false;
        let mut terminal = Terminal::new(TestBackend::new(90, 24)).expect("test terminal");
        let mut state = ListState::default();
        terminal
            .draw(|frame| draw(frame, &doc, &mut state, 0))
            .expect("drawing");

        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(rendered.contains("[ ]"), "rejected state must be visible");
        assert!(rendered.contains("1 of 2 will be redacted"), "{rendered}");
    }

    #[test]
    fn drawing_a_narrow_terminal_does_not_panic() {
        let doc = sample();
        let mut terminal = Terminal::new(TestBackend::new(20, 10)).expect("test terminal");
        let mut state = ListState::default();
        terminal
            .draw(|frame| draw(frame, &doc, &mut state, 0))
            .expect("drawing must survive a small screen");
    }
}
