use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;

use scitadel_core::models::{Paper, ResearchQuestion, Search};

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

pub fn draw_questions(
    frame: &mut Frame,
    area: Rect,
    questions: &[ResearchQuestion],
    selected: usize,
) {
    let mut items = vec![ListItem::new(Line::from("  All"))];

    for q in questions {
        let label = format!("  {} {}", q.id.short(), truncate(&q.text, 14));
        items.push(ListItem::new(Line::from(label)));
    }

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Questions ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

pub fn draw_searches(
    frame: &mut Frame,
    area: Rect,
    searches: &[Search],
    selected: usize,
    breadcrumb: &str,
) {
    let title = format!(" Searches ({breadcrumb}) ");
    let mut items = vec![ListItem::new(Line::from("  All"))];

    for s in searches {
        let label = format!("  {} {}", s.id.short(), truncate(&s.query, 14));
        items.push(ListItem::new(Line::from(label)));
    }

    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

pub fn draw_papers(
    frame: &mut Frame,
    area: Rect,
    papers: &[Paper],
    scores: &[Option<f64>],
    selected: usize,
    breadcrumb: &str,
) {
    let title = format!(" Papers ({breadcrumb}) ");

    let items: Vec<ListItem<'_>> = papers
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let score_str = scores
                .get(i)
                .and_then(|s| *s)
                .map_or_else(|| "--".to_string(), |s| format!("{s:.2}"));
            let label = format!("  {} {}", score_str, truncate(&p.title, 16));
            ListItem::new(Line::from(label))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}
