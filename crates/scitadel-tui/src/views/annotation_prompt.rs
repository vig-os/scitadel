//! In-TUI annotation create / edit / reply / delete prompt state.
//!
//! State machine + a small `draw_overlay` renderer. The state machine
//! is pure (no I/O); the app drives it from key events in
//! `Overlay::PaperDetail` and asks `submit()` / `confirm()` for the
//! next side effect.
//!
//! Iter 3b of #49 (#92): the n / e / d / r keybindings on the paper
//! detail overlay. Visual-mode char-range selection and full $EDITOR
//! integration are out of scope; the note buffer is edited inline.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

/// Which buffer accepts keystrokes when a Create prompt is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateStage {
    Quote,
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnotationPrompt {
    Create {
        stage: CreateStage,
        quote_buf: String,
        note_buf: String,
    },
    Edit {
        annotation_id: String,
        note_buf: String,
    },
    Reply {
        parent_id: String,
        note_buf: String,
    },
    DeleteConfirm {
        annotation_id: String,
    },
}

/// What happens when the user submits the prompt. The app translates
/// each variant into the matching repository call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptCommit {
    /// Open a new prompt (e.g. Create stage transition Quote → Note).
    AdvanceStage(AnnotationPrompt),
    /// Submit the result; caller dispatches to the DB.
    Submit(PromptSubmission),
    /// User cancelled or nothing to do.
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptSubmission {
    Create { quote: String, note: String },
    Edit { annotation_id: String, note: String },
    Reply { parent_id: String, note: String },
    Delete { annotation_id: String },
}

impl AnnotationPrompt {
    /// Start a Create prompt at the Quote stage.
    #[must_use]
    pub fn create() -> Self {
        Self::Create {
            stage: CreateStage::Quote,
            quote_buf: String::new(),
            note_buf: String::new(),
        }
    }

    /// Start an Edit prompt pre-filled with the existing note body.
    #[must_use]
    pub fn edit(annotation_id: impl Into<String>, current_note: impl Into<String>) -> Self {
        Self::Edit {
            annotation_id: annotation_id.into(),
            note_buf: current_note.into(),
        }
    }

    /// Start a Reply prompt against a parent annotation.
    #[must_use]
    pub fn reply(parent_id: impl Into<String>) -> Self {
        Self::Reply {
            parent_id: parent_id.into(),
            note_buf: String::new(),
        }
    }

    /// Start a Delete-confirmation prompt for an annotation.
    #[must_use]
    pub fn delete_confirm(annotation_id: impl Into<String>) -> Self {
        Self::DeleteConfirm {
            annotation_id: annotation_id.into(),
        }
    }

    /// Append a character to the active buffer (whichever the prompt
    /// is currently editing). Confirm prompts ignore character input
    /// other than y/n/Esc; the app handles those at the key layer.
    pub fn push_char(&mut self, ch: char) {
        match self {
            Self::Create {
                stage, quote_buf, ..
            } if *stage == CreateStage::Quote => quote_buf.push(ch),
            Self::Create { note_buf, .. }
            | Self::Edit { note_buf, .. }
            | Self::Reply { note_buf, .. } => {
                note_buf.push(ch);
            }
            Self::DeleteConfirm { .. } => {}
        }
    }

    /// Pop the last character from the active buffer.
    pub fn backspace(&mut self) {
        match self {
            Self::Create {
                stage, quote_buf, ..
            } if *stage == CreateStage::Quote => {
                quote_buf.pop();
            }
            Self::Create { note_buf, .. }
            | Self::Edit { note_buf, .. }
            | Self::Reply { note_buf, .. } => {
                note_buf.pop();
            }
            Self::DeleteConfirm { .. } => {}
        }
    }

    /// Handle Enter. Either advances the stage of a Create prompt,
    /// returns a Submit with whatever the user typed, or cancels if
    /// the buffer is empty (so an empty Enter doesn't write a blank
    /// note).
    #[must_use]
    pub fn submit(&self) -> PromptCommit {
        match self {
            Self::Create {
                stage: CreateStage::Quote,
                quote_buf,
                note_buf,
            } if !quote_buf.trim().is_empty() => PromptCommit::AdvanceStage(Self::Create {
                stage: CreateStage::Note,
                quote_buf: quote_buf.clone(),
                note_buf: note_buf.clone(),
            }),
            Self::Create {
                stage: CreateStage::Note,
                quote_buf,
                note_buf,
            } if !note_buf.trim().is_empty() && !quote_buf.trim().is_empty() => {
                PromptCommit::Submit(PromptSubmission::Create {
                    quote: quote_buf.clone(),
                    note: note_buf.clone(),
                })
            }
            Self::Edit {
                annotation_id,
                note_buf,
            } if !note_buf.trim().is_empty() => PromptCommit::Submit(PromptSubmission::Edit {
                annotation_id: annotation_id.clone(),
                note: note_buf.clone(),
            }),
            Self::Reply {
                parent_id,
                note_buf,
            } if !note_buf.trim().is_empty() => PromptCommit::Submit(PromptSubmission::Reply {
                parent_id: parent_id.clone(),
                note: note_buf.clone(),
            }),
            // Empty buffer (or DeleteConfirm) — Enter is a no-op cancel.
            _ => PromptCommit::Cancel,
        }
    }

    /// Handle the y / n keys for a delete confirmation. Returns
    /// `Some(Submit)` on `y`, `Some(Cancel)` on `n`, `None` otherwise
    /// so the app can ignore irrelevant keys.
    #[must_use]
    pub fn confirm(&self, ch: char) -> Option<PromptCommit> {
        match self {
            Self::DeleteConfirm { annotation_id } => match ch {
                'y' | 'Y' => Some(PromptCommit::Submit(PromptSubmission::Delete {
                    annotation_id: annotation_id.clone(),
                })),
                'n' | 'N' => Some(PromptCommit::Cancel),
                _ => None,
            },
            _ => None,
        }
    }

    /// Short label for the title bar of the prompt overlay.
    #[must_use]
    pub fn title(&self) -> &'static str {
        match self {
            Self::Create {
                stage: CreateStage::Quote,
                ..
            } => " New Annotation — quote ",
            Self::Create {
                stage: CreateStage::Note,
                ..
            } => " New Annotation — note ",
            Self::Edit { .. } => " Edit Annotation ",
            Self::Reply { .. } => " Reply ",
            Self::DeleteConfirm { .. } => " Delete annotation? ",
        }
    }

    /// The user-facing text the prompt currently shows in its body.
    /// For confirms, this is the y/N hint; otherwise the active buffer.
    #[must_use]
    pub fn body(&self) -> &str {
        match self {
            Self::Create {
                stage: CreateStage::Quote,
                quote_buf,
                ..
            } => quote_buf,
            Self::Create {
                stage: CreateStage::Note,
                note_buf,
                ..
            }
            | Self::Edit { note_buf, .. }
            | Self::Reply { note_buf, .. } => note_buf,
            Self::DeleteConfirm { .. } => "press y to delete, n/Esc to cancel",
        }
    }
}

/// Centred modal renderer. Draws on top of the parent area; the
/// caller is responsible for painting the background view first so
/// the modal layers cleanly.
pub fn draw_overlay(frame: &mut Frame, area: Rect, prompt: &AnnotationPrompt) {
    let modal = centered_rect(area, 70, 30);
    frame.render_widget(Clear, modal);

    // Stack: title block / quote summary (Create stage Note only) / body / hint.
    let inner = Block::default()
        .title(prompt.title())
        .borders(Borders::ALL)
        .style(Style::default().fg(crate::theme::theme().emphasis));
    let inner_area = modal;
    frame.render_widget(inner, inner_area);

    // Body area = inside the border, with 1-cell padding.
    let body_area = Rect {
        x: inner_area.x + 2,
        y: inner_area.y + 1,
        width: inner_area.width.saturating_sub(4),
        height: inner_area.height.saturating_sub(2),
    };

    let mut lines: Vec<Line<'_>> = Vec::new();
    if let AnnotationPrompt::Create {
        stage: CreateStage::Note,
        quote_buf,
        ..
    } = prompt
    {
        lines.push(Line::from(vec![
            Span::styled("quote: ", Style::default().fg(crate::theme::theme().muted)),
            Span::styled(format!("\"{quote_buf}\""), Style::default().fg(crate::theme::theme().quote)),
        ]));
        lines.push(Line::from(""));
    }

    if let AnnotationPrompt::DeleteConfirm { annotation_id } = prompt {
        lines.push(Line::from(vec![
            Span::raw("Delete annotation "),
            Span::styled(
                annotation_id.clone(),
                Style::default().fg(crate::theme::theme().danger).add_modifier(Modifier::BOLD),
            ),
            Span::raw("?"),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(prompt.body()));
    } else {
        let prompt_label = match prompt {
            AnnotationPrompt::Create {
                stage: CreateStage::Quote,
                ..
            } => "quoted passage",
            AnnotationPrompt::Create {
                stage: CreateStage::Note,
                ..
            }
            | AnnotationPrompt::Edit { .. } => "note body",
            AnnotationPrompt::Reply { .. } => "reply",
            AnnotationPrompt::DeleteConfirm { .. } => "",
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{prompt_label}: "),
                Style::default().fg(crate::theme::theme().muted),
            ),
            Span::raw(prompt.body().to_string()),
            // Trailing block-cursor hint so the user can see the caret.
            Span::styled("█", Style::default().fg(crate::theme::theme().emphasis)),
        ]));
    }

    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(para, body_area);
}

fn centered_rect(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_prompt_advances_quote_to_note_then_submits() {
        let mut p = AnnotationPrompt::create();
        for c in "hello".chars() {
            p.push_char(c);
        }
        // Enter on Quote with non-empty buf advances to Note stage.
        let next = p.submit();
        let advanced = match next {
            PromptCommit::AdvanceStage(a) => a,
            other => panic!("expected AdvanceStage, got {other:?}"),
        };
        match &advanced {
            AnnotationPrompt::Create {
                stage: CreateStage::Note,
                quote_buf,
                ..
            } => assert_eq!(quote_buf, "hello"),
            other => panic!("expected Create at Note, got {other:?}"),
        }

        let mut p = advanced;
        for c in "my note".chars() {
            p.push_char(c);
        }
        let submission = match p.submit() {
            PromptCommit::Submit(s) => s,
            other => panic!("expected Submit, got {other:?}"),
        };
        assert_eq!(
            submission,
            PromptSubmission::Create {
                quote: "hello".into(),
                note: "my note".into()
            }
        );
    }

    #[test]
    fn enter_with_empty_buffer_cancels_instead_of_submitting() {
        let p = AnnotationPrompt::create();
        assert_eq!(p.submit(), PromptCommit::Cancel);

        let p = AnnotationPrompt::reply("parent-id");
        assert_eq!(p.submit(), PromptCommit::Cancel);
    }

    #[test]
    fn edit_pre_fills_and_submits() {
        let mut p = AnnotationPrompt::edit("a-1", "old note");
        // Tweak the buffer.
        p.backspace();
        for c in " body".chars() {
            p.push_char(c);
        }
        let submission = match p.submit() {
            PromptCommit::Submit(s) => s,
            other => panic!("expected Submit, got {other:?}"),
        };
        assert_eq!(
            submission,
            PromptSubmission::Edit {
                annotation_id: "a-1".into(),
                note: "old not body".into()
            }
        );
    }

    #[test]
    fn reply_submits_with_parent() {
        let mut p = AnnotationPrompt::reply("root-id");
        for c in "agreed".chars() {
            p.push_char(c);
        }
        assert_eq!(
            p.submit(),
            PromptCommit::Submit(PromptSubmission::Reply {
                parent_id: "root-id".into(),
                note: "agreed".into()
            })
        );
    }

    #[test]
    fn delete_confirm_only_responds_to_y_n() {
        let p = AnnotationPrompt::delete_confirm("a-1");
        assert_eq!(
            p.confirm('y'),
            Some(PromptCommit::Submit(PromptSubmission::Delete {
                annotation_id: "a-1".into()
            }))
        );
        assert_eq!(p.confirm('n'), Some(PromptCommit::Cancel));
        assert_eq!(p.confirm('Y'), p.confirm('y'));
        assert_eq!(p.confirm('x'), None);
    }

    #[test]
    fn delete_confirm_ignores_text_input() {
        let mut p = AnnotationPrompt::delete_confirm("a-1");
        p.push_char('q'); // should be a no-op
        p.backspace();
        assert_eq!(p, AnnotationPrompt::delete_confirm("a-1"));
    }

    #[test]
    fn whitespace_only_buffer_is_treated_as_empty() {
        let mut p = AnnotationPrompt::create();
        p.push_char(' ');
        p.push_char(' ');
        assert_eq!(p.submit(), PromptCommit::Cancel);
    }
}
