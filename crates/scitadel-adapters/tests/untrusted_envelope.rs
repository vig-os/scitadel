//! #287's second acceptance box, at the boundary it names: a title that came out
//! of a document's bytes must be neutralised before anything renders it.
//!
//! The two payloads here are **extracted by the real extractors** rather than
//! typed in, so the test cannot pass on a fixture that the production parser
//! would never produce:
//!
//! - a PDF's `/Title` in the UTF-16BE hex string encoding, which is what Word
//!   writes for a non-ASCII title and the only `/Title` encoding that can hold a
//!   literal backslash — the literal-string decoder drops one;
//! - a served landing page's `<meta name="citation_title">`, whose escape
//!   sequences arrive as `&#x1b;` numeric entities, which the page's own decoder
//!   expands to real ESC bytes.
//!
//! Both then go through the whole write path — a `Paper` row stored and read back
//! — so what is asserted is what a consumer of `PaperRepository` sees.
//!
//! The two render paths themselves are asserted in their own crates, against
//! these same payloads: the TUI's rendered buffer in `scitadel-tui`'s reader view,
//! and `read_paper`'s return in `scitadel-mcp`.

use scitadel_adapters::identity;
use scitadel_core::models::{Paper, PaperId};
use scitadel_core::ports::PaperRepository;
use scitadel_core::untrusted::{Provenance, UntrustedText};
use scitadel_db::sqlite::{Database, SqlitePaperRepository};

/// `ESC [31m` (colour), `ESC [2J` (erase display) and an OSC 8 hyperlink whose
/// payload is a URL — the three shapes a terminal actually honours, from the
/// one that can repaint a screen to the one that makes a title clickable.
const HOSTILE_TITLE: &str = "Real Title\u{1b}[31m\u{1b}[2J\u{1b}]8;;https://attacker.example/\u{1b}\\click\u{1b}]8;;\u{1b}\\";

/// The `/Title` a converter wrote for that string.
fn pdf_bytes() -> Vec<u8> {
    let hex: String = HOSTILE_TITLE.chars().fold(String::new(), |mut acc, c| {
        use std::fmt::Write as _;
        let _ = write!(acc, "{:04X}", c as u32);
        acc
    });
    format!("%PDF-1.7\n/Title <FEFF{hex}> /Author(x)\n%%EOF\n").into_bytes()
}

/// The landing page a publisher served, with the escapes as numeric entities.
fn html_bytes() -> Vec<u8> {
    r#"<html><head><meta name="citation_title" content="Real Title&#x1b;[31m&#x1b;[2J&#x1b;]8;;https://attacker.example/&#x1b;\click"></head></html>"#
        .to_string()
        .into_bytes()
}

fn assert_neutralised(source: &str, raw: &str) {
    assert!(
        raw.contains('\u{1b}'),
        "{source}: the extracted title really carries escapes, so the assertions below \
         cross the boundary rather than passing on a no-op"
    );

    let text = UntrustedText::publisher_supplied(raw);
    let rendered = text.rendered();

    assert!(
        !rendered.contains('\u{1b}'),
        "{source}: no escape byte may survive: {rendered:?}"
    );
    assert!(
        !rendered.chars().any(char::is_control),
        "{source}: no control character may survive: {rendered:?}"
    );
    for (label, needle) in [
        ("CSI", "31m"),
        ("erase-display", "[2J"),
        ("OSC 8", "https://attacker.example/"),
    ] {
        assert!(
            !rendered.contains(needle),
            "{source}: the {label} payload must not survive: {rendered:?}"
        );
    }
    assert_eq!(
        rendered, "Real Title click",
        "{source}: the words survive, so a reader can still weigh the document"
    );
    assert!(
        text.needs_neutralising(),
        "{source}: and the caller can tell that it changed something"
    );

    // Provenance is recoverable at the call site — #287's third box. `Ours`
    // would be a lie for a string that came out of the bytes.
    assert!(!text.provenance().is_trusted());
    assert_eq!(text.provenance(), Provenance::PublisherSupplied);
    assert_eq!(text.provenance().label(), "from the document (untrusted)");
}

/// Both real extractions, neutralised.
#[test]
fn a_documents_own_title_is_neutralised_when_it_is_rendered() {
    let pdf = identity::pdf_title(&pdf_bytes()).expect("a PDF states a title");
    assert_neutralised("pdf_title", &pdf);

    let html = identity::html_title(&html_bytes()).expect("a page states a title");
    assert_neutralised("html_title", &html);
}

/// The whole path a consumer takes: extracted title, stored on a `Paper`, read
/// back out of the database. The bytes survive, and the rendering is neutral.
#[test]
fn a_documents_title_survives_the_database_round_trip_and_renders_neutral() {
    let raw = identity::pdf_title(&pdf_bytes()).expect("a PDF states a title");

    let db = Database::open_in_memory().expect("open db");
    db.migrate().expect("migrate");
    let repo = SqlitePaperRepository::new(db.clone());

    let mut paper = Paper::new(&raw);
    paper.id = PaperId::from("p-hostile");
    paper.authors = vec![UntrustedText::publisher_supplied("Attacker, A.")];
    paper.title = UntrustedText::publisher_supplied(raw.clone());
    repo.save(&paper).expect("save paper");

    let back = repo.get("p-hostile").expect("get").expect("present");
    assert_eq!(
        back.title.as_str(),
        raw,
        "the stored title keeps its bytes: neutralisation is a rendering concern, \
         not a storage one"
    );
    assert_eq!(back.title.rendered(), "Real Title click");
    assert_eq!(back.authors[0].rendered(), "Attacker, A.");

    // What the column actually records, and what it does not: `papers.title`
    // names scitadel's record of the work, so the read path labels it `Ours`.
    // That is honest for a title that arrived from a metadata feed, and it is a
    // weaker claim than it looks for one that was written from a document —
    // nothing in the row says which. The document's own claim is the
    // `resolved_title` column, where `resolved_title_text()` labels it
    // `PublisherSupplied` instead, so the question "is this ours or the
    // publisher's?" has an answer there and not here.
    //
    // Neither label is a licence to render unescaped: both go through
    // `rendered()`, which is the part that is not optional.
    assert!(back.title.provenance().is_trusted());
    assert_eq!(back.title.rendered(), "Real Title click");
    assert!(
        UntrustedText::ours(raw.as_str()).provenance().is_trusted(),
        "and an `Ours` string is still neutralised on the way out"
    );
}
