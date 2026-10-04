//! ADR-007 §1 "Artefact rules": **"SI only counts when its magic bytes match
//! its declared format."**
//!
//! > The raid review found 7 of 27 "SI" files on disk were HTML landing pages.
//!
//! That is a measured failure rate of 26% in a corpus scitadel is about to
//! ingest, and it is silent: a 200 kB `<!doctype html>` saved as
//! `Supporting_Information_S1.pdf` satisfies every check the schema has. It
//! hashes, it stores, it gets an `artefacts` row with `format = 'pdf'`, and
//! `coverage` will then report the work as having its SI — so raid's extraction
//! pipeline reads an HTML page and produces an empty table with no error anywhere.
//!
//! ## Why sniffing bytes and not trusting the extension
//!
//! The extension is the *claim*; the first bytes are the *evidence*. This module
//! compares them, and the comparison is the whole of ADR-007's sentence.
//!
//! ## The conservative direction
//!
//! Refusing a real file costs a person one re-drop. Accepting a landing page
//! costs a silently wrong dataset. So the module refuses on **positive
//! evidence** and nothing else:
//!
//! - **`Mismatch`** only when the content is *recognisably* something else. A
//!   `.pdf` whose bytes start `<!doctype html>` is a mismatch; a `.pdf` whose
//!   bytes are nothing this module recognises is [`MagicVerdict::Unknown`], which
//!   is not a refusal. Unrecognised is not evidence — the same reasoning
//!   `identity::Verdict::Unverified` rests on, and #260/#261 are what guessing in
//!   the other direction costs.
//! - **An HTML page is refused under every non-HTML declared format**, including
//!   the unbounded ones. This is the measured case, and it is the one rule here
//!   that fires without a format table entry — which matters because SI is by
//!   definition unbounded (`Supplementary Data.dat` is a real thing) and a table
//!   alone would let exactly the 7-of-27 failure through.
//! - **An empty file is refused** under every format: there is no artefact in
//!   zero bytes, and storing one would make `coverage` claim we hold it.
//!
//! ## What a caller records
//!
//! A [`MagicVerdict::Mismatch`] is the reason
//! `acquisition_attempts.outcome = 'bad_magic'` exists. Migration 013 declares
//! that outcome and, like `too_large` and `publisher`, it had no writer anywhere
//! in the workspace — so this module plus its callers in [`crate::scan`] are what
//! make it mean something. `bad_magic` records the attempt; **no artefact row is
//! created**, because the rule is that SI only *counts* when the bytes match.
//!
//! [`crate::scan`]: crate::scan

/// How the first bytes of a file read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Magic {
    /// `%PDF-` — the ISO 32000 header, which is a *fixed* five-byte signature
    /// and not a heuristic.
    Pdf,
    /// A markup document: `<!doctype html>`, `<html`, `<svg`, `<?xml`.
    Markup,
    /// A ZIP container, which is also what every OOXML format (`.xlsx`,
    /// `.docx`, `.ods`) is.
    Zip,
    /// An OLE2 compound file — legacy `.xls`, and the only table serialisation
    /// in the vocabulary that is not a ZIP.
    Ole2,
    /// A JSON document, judged by its first significant byte.
    Json,
    /// Delimited or plain text that decodes as UTF-8 and holds no NUL.
    Text,
    /// An image, in any of the formats `artefacts.kind = 'figure'` accepts.
    Image,
    /// PostScript, which is what `.eps` is.
    PostScript,
    /// Nothing in this module recognises it.
    Unknown,
    /// Zero bytes.
    Empty,
}

impl Magic {
    /// The word used in a refusal message, so the reason a file was not filed
    /// says what it *is* rather than only what it was claimed to be.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Pdf => "a PDF",
            Self::Markup => "a markup document (HTML or XML)",
            Self::Zip => "a ZIP container",
            Self::Ole2 => "an OLE2 compound document (legacy .xls)",
            Self::Json => "a JSON document",
            Self::Text => "UTF-8 text",
            Self::Image => "an image",
            Self::PostScript => "PostScript",
            Self::Unknown => "not a format this build recognises",
            Self::Empty => "empty (zero bytes)",
        }
    }
}

/// What [`check`] concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MagicVerdict {
    /// The bytes are what the format says they are. Nothing to record.
    Matches,
    /// Positive evidence they are not. The caller records `bad_magic` and files
    /// nothing.
    Mismatch {
        declared: String,
        found: Magic,
        reason: String,
    },
    /// No evidence either way: the declared format is not in the table, or the
    /// content is not recognisable. **Not a refusal.**
    Unknown { declared: String, reason: String },
}

impl MagicVerdict {
    /// Is this a refusal?
    #[must_use]
    pub fn is_mismatch(&self) -> bool {
        matches!(self, Self::Mismatch { .. })
    }

    /// The line to record in `acquisition_attempts.detail`.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Matches => None,
            Self::Mismatch { reason, .. } | Self::Unknown { reason, .. } => Some(reason),
        }
    }
}

/// Do these bytes match the format their file claims to be?
///
/// `declared` is the lowercased extension — the same value `file_extension`
/// produced and the same one stored in `artefacts.format`, so the check and the
/// row cannot disagree about what was claimed.
///
/// See the module docs for the conservative direction and for why `Unknown` is
/// not a refusal.
#[must_use]
pub fn check(declared: &str, bytes: &[u8]) -> MagicVerdict {
    let found = sniff(bytes);
    let declared = declared.to_ascii_lowercase();
    let owned = declared.clone();

    if found == Magic::Empty {
        return MagicVerdict::Mismatch {
            declared: declared.clone(),
            found,
            reason: format!(
                "the file is empty (0 bytes), so it cannot be a {declared} artefact whatever its \
                 name says"
            ),
        };
    }

    // Rule 1: a table entry, and the content is recognisably a different
    // family. `Unknown` content is deliberately not a mismatch.
    if let Some(expected) = expected_magic(&declared) {
        if found == expected {
            return MagicVerdict::Matches;
        }
        if found != Magic::Unknown {
            return mismatch(&declared, found, expected, EXTRA_NONE);
        }
        return MagicVerdict::Unknown {
            declared: declared.clone(),
            reason: format!(
                "declared as {declared}, whose first bytes are not a signature this build can \
                 check; a landing page would have been recognised, so this is not one"
            ),
        };
    }

    // Rule 2: no table entry (SI is unbounded) — but an HTML page is a landing
    // page under every name, which is the measured 7-of-27 failure.
    if found == Magic::Markup {
        return mismatch(
            &declared,
            found,
            Magic::Markup,
            "the file declares no checkable format, but its bytes are a markup document, and an \
             HTML page is not supplementary data under any name — this is the landing-page case \
             ADR-007 §1 records 7 times out of 27",
        );
    }

    MagicVerdict::Unknown {
        declared: owned,
        reason: format!(
            "{declared} is not a format this build can check by magic bytes (supplementary data is \
             unbounded), and the content is {}",
            found.describe()
        ),
    }
}

/// The magic a declared format implies, `None` for a format with no signature.
#[must_use]
pub fn expected_magic(declared: &str) -> Option<Magic> {
    Some(match declared {
        "pdf" => Magic::Pdf,
        "html" | "htm" | "xhtml" | "xml" | "nxml" | "jats" | "svg" => Magic::Markup,
        "zip" | "xlsx" | "docx" | "ods" | "pptx" => Magic::Zip,
        "xls" => Magic::Ole2,
        "json" => Magic::Json,
        "csv" | "tsv" | "txt" | "md" | "tex" | "bib" | "ris" => Magic::Text,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "tif" | "tiff" | "bmp" | "avif" => Magic::Image,
        "eps" | "ps" => Magic::PostScript,
        _ => return None,
    })
}

/// No extra observation beyond the two formats: used where the mismatch speaks
/// for itself.
const EXTRA_NONE: &str = "the file's name and its contents are different formats";

/// A [`MagicVerdict::Mismatch`] with the reason ADR-007's own sentence wants:
/// what was claimed, what the bytes are, and — when there is something extra to
/// say — why that combination is a known failure rather than a curiosity.
fn mismatch(declared: &str, found: Magic, expected: Magic, extra: &str) -> MagicVerdict {
    MagicVerdict::Mismatch {
        declared: declared.to_string(),
        found,
        reason: format!(
            "declared as {declared} (which must be {}), but the bytes are {}: {extra}",
            expected.describe(),
            found.describe()
        ),
    }
}

/// What the first bytes of `bytes` say it is.
///
/// Only leading signatures and two cheap structural facts — is it valid UTF-8,
/// does it hold a NUL — so this runs on any file size without reading it whole:
/// callers hand it the head of the file, never the entire artefact.
#[must_use]
pub fn sniff(bytes: &[u8]) -> Magic {
    if bytes.is_empty() {
        return Magic::Empty;
    }
    if identity::is_pdf(bytes) {
        return Magic::Pdf;
    }
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        return Magic::Zip;
    }
    if bytes.starts_with(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1") {
        return Magic::Ole2;
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"\xFF\xD8\xFF")
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || bytes.starts_with(b"BM")
        || bytes.starts_with(b"II*\0")
        || bytes.starts_with(b"MM\0*")
        || (bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP")
    {
        return Magic::Image;
    }
    if bytes.starts_with(b"%!PS") {
        return Magic::PostScript;
    }
    if let Some(text) = leading_text(bytes) {
        // Markup before text, because an HTML or XML document is also valid
        // UTF-8 and would otherwise read as a CSV.
        let head = text.trim_start_matches('\u{feff}').trim_start();
        if head.starts_with("<!doctype") || head.starts_with("<html") || head.starts_with("<svg") {
            return Magic::Markup;
        }
        if head.starts_with("<?xml") {
            return Magic::Markup;
        }
        if head.starts_with('<')
            && head[1..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '!')
        {
            return Magic::Markup;
        }
        if head.starts_with('{') || head.starts_with('[') {
            return Magic::Json;
        }
        return Magic::Text;
    }
    Magic::Unknown
}

/// `bytes` as text, when they are text: valid UTF-8 with no NUL.
///
/// A NUL byte is what separates "a CSV file" from "a binary blob that happens
/// to decode" — without it a `.csv` of 200 kB of compressed-looking bytes would
/// pass, and a `.csv` is exactly the format raid's tables arrive in.
fn leading_text(bytes: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(bytes).ok()?;
    if text.as_bytes().contains(&0) {
        return None;
    }
    Some(text)
}

/// Read the head of a file — enough bytes for every signature above.
///
/// A fixed window rather than "the whole file", because a 250 MB SI must not be
/// read into memory to learn what it is. Every signature this module knows is
/// shorter than 32 bytes and every structural test (`UTF-8`, first significant
/// character) is decided by the head, so a longer window buys nothing.
///
/// # Errors
///
/// `DbError` if the file cannot be opened or read, which is the same refusal
/// every other blob helper makes.
pub fn read_head(path: &std::path::Path) -> Result<Vec<u8>, scitadel_db::error::DbError> {
    use std::io::Read;

    /// Wide enough for every signature in [`sniff`], and a whole number of
    /// UTF-8 boundaries' worth of slack so a multi-byte first character is not
    /// split (a split would make `from_utf8` fail and read as `Unknown`).
    const HEAD_BYTES: usize = 8 * 1024;

    let mut file =
        std::fs::File::open(path).map_err(|e| scitadel_db::sqlite::io_error("open", path, &e))?;
    let mut head = vec![0_u8; HEAD_BYTES];
    let read = file
        .read(&mut head)
        .map_err(|e| scitadel_db::sqlite::io_error("read", path, &e))?;
    head.truncate(read);
    Ok(head)
}

use crate::identity;

#[cfg(test)]
mod tests {
    use super::*;

    /// A publisher's "your download is starting" page, which is what 7 of raid's
    /// 27 "SI" files actually were.
    const LANDING_PAGE: &[u8] =
        b"<!DOCTYPE html>\n<html lang=\"en\"><head><title>Preparing your download</title></head>\
          \n<body><p>Your file will be available shortly.</p></body></html>\n";

    #[test]
    fn a_pdf_saved_as_a_pdf_is_accepted() {
        assert_eq!(
            check("pdf", b"%PDF-1.7\nbody\n%%EOF\n"),
            MagicVerdict::Matches
        );
        assert_eq!(
            check("PDF", b"%PDF-1.7\n"),
            MagicVerdict::Matches,
            "case-insensitive"
        );
    }

    #[test]
    fn an_html_page_saved_as_an_si_pdf_is_refused() {
        let verdict = check("pdf", LANDING_PAGE);
        assert!(verdict.is_mismatch(), "{verdict:?}");
        let reason = verdict.reason().expect("a reason");
        assert!(reason.contains("declared as pdf"), "{reason}");
        assert!(
            reason.contains("markup document"),
            "the reason says what the bytes actually are: {reason}"
        );
    }

    /// The measured case under a name no table entry could catch: SI is
    /// unbounded, so `Supporting_Data.dat` holding a landing page must still be
    /// refused.
    #[test]
    fn an_html_page_saved_under_an_uncheckable_si_name_is_refused() {
        let verdict = check("dat", LANDING_PAGE);
        assert!(verdict.is_mismatch(), "{verdict:?}");
        assert!(
            verdict
                .reason()
                .is_some_and(|r| r.contains("landing-page case")),
            "{}",
            verdict.reason().unwrap_or_default()
        );
    }

    #[test]
    fn an_empty_file_is_refused_under_every_format() {
        for format in ["pdf", "si", "dat", "csv"] {
            let verdict = check(format, b"");
            assert!(verdict.is_mismatch(), "{format}: {verdict:?}");
        }
    }

    /// Content no signature covers is **not** evidence of anything, so it is not
    /// a refusal. Getting this backwards is how a legitimate-but-odd supplement
    /// ends up permanently unfetchable.
    #[test]
    fn unrecognised_content_is_unknown_not_a_mismatch() {
        let verdict = check("pdf", b"\x00\x01\x02\x03 unknown bytes");
        assert!(!verdict.is_mismatch(), "{verdict:?}");
        assert!(
            matches!(verdict, MagicVerdict::Unknown { .. }),
            "{verdict:?}"
        );
    }

    /// Every signature in the table agrees with itself — the table is what the
    /// whole check rests on, so a wrong entry would refuse real files.
    #[test]
    fn every_signature_matches_its_own_format() {
        let cases: &[(&str, &[u8])] = &[
            ("pdf", b"%PDF-1.4"),
            ("html", b"<!DOCTYPE html><html></html>"),
            ("xml", b"<?xml version=\"1.0\"?><article/>"),
            (
                "jats",
                b"<!DOCTYPE article PUBLIC \"-//JATS//DTD\"><article/>",
            ),
            ("svg", b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"),
            ("zip", b"PK\x03\x04\x14\x00\x00\x00\x00\x00"),
            ("xlsx", b"PK\x03\x04\x14\x00\x00\x00\x00\x00"),
            ("xls", b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1rest"),
            ("json", b"  {\"rows\": []}"),
            ("csv", b"table,kd\n1,2.3\n"),
            ("txt", b"plain words\n"),
            ("png", b"\x89PNG\r\n\x1a\nrest"),
            ("jpg", b"\xFF\xD8\xFF\xE0rest"),
            ("gif", b"GIF89arest"),
            ("webp", b"RIFF\x00\x00\x00\x00WEBPrest"),
            ("tif", b"II*\x00rest"),
            ("bmp", b"BMrest"),
            ("eps", b"%!PS-Adobe-3.0 EPSF-3.0"),
        ];
        for (format, bytes) in cases {
            assert_eq!(
                check(format, bytes),
                MagicVerdict::Matches,
                "{format} with bytes {bytes:?}"
            );
        }
    }

    /// Cross-format refusals for the table entries, so a mislabelled archive or
    /// image cannot pass as something else.
    #[test]
    fn a_known_signature_under_the_wrong_format_is_refused() {
        let cases: &[(&str, &[u8])] = &[
            (
                "xlsx",
                b"<!DOCTYPE html><html>not really a spreadsheet</html>",
            ),
            ("json", b"k,v\n1,2\n"),
            ("png", b"%PDF-1.7\n"),
            ("zip", b"%PDF-1.7\n"),
            ("csv", b"\x89PNG\r\n\x1a\n"),
            ("pdf", b"{\"rows\": []}"),
            ("html", b"%PDF-1.7\n"),
        ];
        for (format, bytes) in cases {
            assert!(
                check(format, bytes).is_mismatch(),
                "{format} holding {bytes:?} must be refused"
            );
        }
    }

    /// A BOM and leading whitespace must not hide a markup document, which is
    /// how a Word-exported supplementary file starts.
    #[test]
    fn a_bom_and_leading_whitespace_do_not_hide_markup() {
        assert_eq!(
            sniff("\u{feff}\n   <!DOCTYPE html><html/>".as_bytes()),
            Magic::Markup
        );
        assert_eq!(sniff(b"<?xml version=\"1.0\"?>\n<article/>"), Magic::Markup);
    }

    /// A multi-byte character straddling the head window's edge must not turn a
    /// real CSV into a refusal.
    ///
    /// This is the shape that would actually bite: `read_head` returns a byte
    /// count, and a two-byte Greek letter whose first byte is the last one read
    /// leaves a truncated sequence at the end of the window. `from_utf8` fails on
    /// the *whole* buffer, so the naive reading is "unrecognised bytes" — and an
    /// `unknown` verdict that were a *mismatch* would refuse every UTF-8 file
    /// whose name happened to be a byte-length boundary away from clean.
    #[test]
    fn a_character_split_by_the_head_window_is_not_a_false_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("table.csv");
        // 8191 ASCII bytes, then the first byte of a two-byte character: the
        // window ends between the character's halves.
        let mut body = vec![b'x'; 8 * 1024 - 1];
        body.extend_from_slice(&"κ".as_bytes()[..1]);
        body.extend_from_slice(b",1,2\n");
        std::fs::write(&path, &body).unwrap();

        let head = read_head(&path).unwrap();
        assert_eq!(head.len(), 8 * 1024, "the window ends mid-character");
        assert_eq!(sniff(&head), Magic::Unknown, "precondition");
        assert!(
            !check("csv", &head).is_mismatch(),
            "unrecognisable bytes are not evidence, so nothing is refused"
        );
        assert_eq!(
            check("csv", &head),
            MagicVerdict::Unknown {
                declared: "csv".to_string(),
                reason: check("csv", &head).reason().unwrap_or_default().to_string(),
            },
            "and the reason names the declared format"
        );
    }

    /// The head window is bounded, so a 250 MB SI is never read whole to learn
    /// what it is.
    #[test]
    fn only_the_head_of_a_file_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.pdf");
        // A valid PDF header followed by far more than the window.
        let mut body = b"%PDF-1.7\n".to_vec();
        body.extend(std::iter::repeat_n(b'x', 64 * 1024));
        std::fs::write(&path, &body).unwrap();
        let head = read_head(&path).unwrap();
        assert!(head.len() <= 8 * 1024, "the head is bounded");
        assert_eq!(check("pdf", &head), MagicVerdict::Matches);
        assert!(read_head(&dir.path().join("gone.pdf")).is_err());
    }
}
