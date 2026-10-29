//! `search` — find literal text or regular expressions inside a PDF.
//!
//! ## Coordinate conventions
//!
//! Every `bbox` reported in `details.hits[]` describes the matched text in
//! **PDF points** (1/72 inch) with the origin at the **top-left corner of the
//! page**, x increasing to the right and y increasing downwards. Pdfium works
//! in the PDF-native bottom-left system, so the value emitted here is the
//! vertical flip `y = rect_space_height - rect.top()`; `width`/`height` are the
//! rectangle's extents. All four numbers are rounded to two decimals.
//!
//! `rect_space_height` is the height of the space pdfium reports text rects
//! in, which is the page's *unrotated* media box. A page carrying `/Rotate 90`
//! reports `width()`/`height()` transposed relative to that space, so the
//! height is taken from `width()` for quarter turns instead of being used
//! directly; otherwise the flip above would produce a negative `y`.
//!
//! When no geometry can be established the `bbox` is `null` rather than a
//! guess — a hit is never given invented coordinates.
//!
//! ## Search strategies
//!
//! 1. **pdfium (preferred).** The page's text layer is used directly, so each
//!    hit carries an exact bounding box. Literal patterns go through pdfium's
//!    own `FPDFText_FindStart` search, which is literal-only by design;
//!    regular expressions are evaluated over the page's plain text and the
//!    matched character range is mapped back onto pdfium character indices to
//!    recover a box.
//! 2. **plain text (fallback).** When pdfium cannot be bound the text is
//!    extracted with `pdf-extract`, the pattern is matched per page, and
//!    `bbox` is `null` with `geometry_available: false`. The command stays
//!    usable on machines without a pdfium library.
//!
//! Both strategies share one compiled [`Regex`]: a literal pattern is
//! `regex::escape`d first, so literal and regex searches follow the same code
//! path and a literal can never be silently reinterpreted.

use std::collections::BTreeSet;

use pdfium_render::prelude::*;
use regex::Regex;
use serde_json::{json, Value};

use crate::error::PdfError;
use crate::io;
use crate::output::CommandResult;

/// Arguments for [`run`], assembled by `main.rs`.
pub struct SearchArgs<'a> {
    pub input: &'a str,
    pub pattern: &'a str,
    pub regex: bool,
    pub ignore_case: bool,
    pub pages: Option<&'a str>,
    pub context: usize,
    pub count_only: bool,
    pub max_results: usize,
    pub password: Option<&'a str>,
    pub pdfium_path: Option<&'a str>,
}

/// A single search hit, before it is rendered as JSON.
struct Hit {
    page: u32,
    text: String,
    bbox: Option<Bbox>,
    context: String,
}

/// A rectangle in PDF points, origin top-left.
pub struct Bbox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Bbox {
    fn to_json(&self) -> Value {
        json!({
            "x": self.x,
            "y": self.y,
            "width": self.width,
            "height": self.height,
        })
    }
}

/// One located occurrence of a pattern, for callers that need coordinates
/// rather than a report (e.g. `redact`).
pub struct Located {
    pub page: u32,
    pub text: String,
    pub bbox: Bbox,
}

/// Locate every occurrence of `pattern` on the selected pages, returning the
/// coordinates of each hit in the CLI's top-left-origin convention.
///
/// This is the geometry half of `search`, exposed so `redact` can reuse the
/// same pdfium measurement and coordinate flip instead of duplicating it.
/// pdfium is mandatory here: without it there are no coordinates, and a caller
/// that draws boxes must not guess. Hits whose rectangle cannot be established
/// are dropped rather than given invented positions.
pub fn locate(
    input: &str,
    pattern: &str,
    regex: bool,
    ignore_case: bool,
    pages: Option<&str>,
    password: Option<&str>,
    pdfium_path: Option<&str>,
) -> Result<Vec<Located>, PdfError> {
    if pattern.is_empty() {
        return Err(PdfError::MissingArg {
            arg: "--find".to_string(),
            example:
                "pdf-cli redact --input in.pdf --output out.pdf --find \"Acme\" --replace \"Beta\""
                    .into(),
        });
    }

    let compiled = build_regex(pattern, regex, ignore_case)?;
    let bytes = io::read_bytes(input)?;

    let doc = io::load_doc_bytes(&bytes, password)?;
    let total_pages = doc.get_pages().len() as u32;
    drop(doc);

    let page_list = io::parse_page_range(pages, total_pages)?;

    let pdfium = io::bind_pdfium(pdfium_path)?;
    let pdf_doc = pdfium
        .load_pdf_from_byte_slice(&bytes, password)
        .map_err(pdfium_error)?;

    let mut out = Vec::new();
    for &page_num in &page_list {
        let page = pdf_doc
            .pages()
            .get((page_num - 1) as u16)
            .map_err(pdfium_error)?;
        let frame = PageFrame::new(&page);
        let text = page.text().map_err(pdfium_error)?;

        let (scanned, indices, byte_starts) = scan_page(&text);
        if scanned.trim().is_empty() {
            continue;
        }

        let raw = if regex {
            regex_matches(&compiled, &scanned, &indices, &byte_starts, &text, &frame)
        } else {
            literal_matches(
                &text,
                pattern,
                &compiled,
                ignore_case,
                &scanned,
                &indices,
                &byte_starts,
                &frame,
            )
        };

        for (_, _, hit_text, bbox) in raw {
            if let Some(bbox) = bbox {
                out.push(Located {
                    page: page_num,
                    text: hit_text,
                    bbox,
                });
            }
        }
    }

    Ok(out)
}

/// Accumulates hits while honouring `--count-only` and `--max-results`.
struct Sink {
    count_only: bool,
    /// `Some(n)` when at most `n` hits should be reported.
    limit: Option<usize>,
    hits: Vec<Hit>,
    total: usize,
    truncated: bool,
    matched_pages: BTreeSet<u32>,
    pages_searched: u32,
    /// `true` when at least one searched page carried extractable text.
    had_text: bool,
}

impl Sink {
    fn new(count_only: bool, max_results: usize) -> Self {
        Sink {
            count_only,
            limit: if max_results > 0 {
                Some(max_results)
            } else {
                None
            },
            hits: Vec::new(),
            total: 0,
            truncated: false,
            matched_pages: BTreeSet::new(),
            pages_searched: 0,
            had_text: false,
        }
    }

    /// Records one hit. Returns `true` when the caller should stop searching.
    fn push(&mut self, hit: Hit) -> bool {
        self.matched_pages.insert(hit.page);

        if self.count_only {
            // Counting is cheap: always report the real total, so a
            // `--max-results` cap never distorts a count.
            self.total += 1;
            return false;
        }

        if let Some(limit) = self.limit {
            if self.hits.len() >= limit {
                self.truncated = true;
                return true;
            }
        }

        self.total += 1;
        self.hits.push(hit);
        false
    }
}

/// Run the `search` command.
///
/// Reads the PDF once (from a path or stdin), resolves the page selection
/// through [`io::parse_page_range`], then reports every match as
/// `{page, text, bbox, context}` plus a one-sentence summary in `output`.
/// `dry_run` writes nothing (search never writes) but is echoed in `details`.
pub fn run(args: SearchArgs<'_>, dry_run: bool) -> Result<CommandResult, PdfError> {
    if args.pattern.is_empty() {
        return Err(PdfError::MissingArg {
            arg: "--pattern".to_string(),
            example: "pdf-cli search --input doc.pdf --pattern \"invoice\"".into(),
        });
    }

    let regex = build_regex(args.pattern, args.regex, args.ignore_case)?;

    // Read once so that stdin (`--input -`) is not consumed twice.
    let bytes = io::read_bytes(args.input)?;

    let doc = io::load_doc_bytes(&bytes, args.password).map_err(|e| match e {
        PdfError::WrongPassword { .. } | PdfError::PasswordRequired => e,
        other => other,
    })?;
    let total_pages = doc.get_pages().len() as u32;
    drop(doc);

    let pages = io::parse_page_range(args.pages, total_pages)?;

    let mut sink = Sink::new(args.count_only, args.max_results);
    let mut geometry_available = false;

    match io::bind_pdfium(args.pdfium_path) {
        Ok(pdfium) => match pdfium.load_pdf_from_byte_slice(&bytes, args.password) {
            Ok(pdf_doc) => {
                search_with_pdfium(&pdf_doc, &regex, &args, &pages, &mut sink)?;
                geometry_available = true;
            }
            // A pdfium load failure never fails the command: fall back to the
            // plain-text strategy that `main.rs` advertises in `--help`. The
            // reason is surfaced on stderr so the degradation stays visible.
            Err(err) => {
                eprintln!(
                    "pdf-cli: warning: pdfium could not open `{}` ({err}); \
                     falling back to plain-text search without geometry",
                    args.input
                );
                search_plain_text(&bytes, &regex, &args, &pages, &mut sink)?;
            }
        },
        Err(PdfError::PdfiumNotFound { .. }) => {
            eprintln!(
                "pdf-cli: warning: pdfium library not found; \
                 falling back to plain-text search without geometry"
            );
            search_plain_text(&bytes, &regex, &args, &pages, &mut sink)?;
        }
        Err(err) => return Err(err),
    }

    if !sink.had_text {
        return Err(PdfError::Operation {
            reason: format!(
                "No extractable text found on the selected pages of `{}`. \
                 The PDF is probably a scan without a text layer. \
                 pdf-cli does not run OCR: recognise the pages with a separate OCR tool first.",
                args.input
            ),
        });
    }

    let matched_pages: Vec<u32> = sink.matched_pages.iter().copied().collect();
    let hits: Vec<Value> = sink
        .hits
        .iter()
        .map(|hit| {
            json!({
                "page": hit.page,
                "text": hit.text,
                "bbox": match &hit.bbox {
                    Some(bbox) => bbox.to_json(),
                    None => Value::Null,
                },
                "context": hit.context,
            })
        })
        .collect();

    let summary = if sink.total == 0 {
        "no matches".to_string()
    } else {
        format!(
            "{} {} on {} of {} pages",
            sink.total,
            if sink.total == 1 { "match" } else { "matches" },
            matched_pages.len(),
            sink.pages_searched
        )
    };
    let summary = if sink.truncated {
        format!("{summary} (truncated)")
    } else {
        summary
    };

    let mut details = json!({
        "input": args.input,
        "pattern": args.pattern,
        "regex": args.regex,
        "ignore_case": args.ignore_case,
        "geometry_available": geometry_available,
        "total_matches": sink.total,
        "matched_pages": matched_pages,
        "pages_searched": sink.pages_searched,
        "hits": hits,
        "truncated": sink.truncated,
    });
    if dry_run {
        details["dry_run"] = json!(true);
    }

    Ok(CommandResult::ok("search", Some(summary), details))
}

/// Compile the pattern once, escaping it first when it is not a regex.
fn build_regex(pattern: &str, is_regex: bool, ignore_case: bool) -> Result<Regex, PdfError> {
    let body = if is_regex {
        pattern.to_string()
    } else {
        regex::escape(pattern)
    };
    let body = if ignore_case {
        format!("(?i){body}")
    } else {
        body
    };

    Regex::new(&body).map_err(|err| PdfError::Operation {
        reason: format!(
            "Invalid regular expression `{pattern}`: {err}. \
             Escape regex metacharacters (for example \\( \\) \\. \\[ \\] \\* \\+ \\?) \
             or drop --regex to search for the literal text."
        ),
    })
}

// ---------------------------------------------------------------------------
// pdfium strategy
// ---------------------------------------------------------------------------

fn search_with_pdfium(
    doc: &PdfDocument<'_>,
    regex: &Regex,
    args: &SearchArgs<'_>,
    pages: &[u32],
    sink: &mut Sink,
) -> Result<(), PdfError> {
    'pages: for &page_num in pages {
        sink.pages_searched += 1;

        let page = doc
            .pages()
            .get((page_num - 1) as u16)
            .map_err(pdfium_error)?;
        let frame = PageFrame::new(&page);
        let text = page.text().map_err(pdfium_error)?;

        // Rebuild the page text from pdfium's own character indices so that a
        // byte range in the string can be mapped back to a character range.
        let (scanned, indices, byte_starts) = scan_page(&text);
        if scanned.trim().is_empty() {
            continue;
        }
        sink.had_text = true;

        let raw = if args.regex {
            regex_matches(regex, &scanned, &indices, &byte_starts, &text, &frame)
        } else {
            literal_matches(
                &text,
                args.pattern,
                regex,
                args.ignore_case,
                &scanned,
                &indices,
                &byte_starts,
                &frame,
            )
        };

        for (start, end, text, bbox) in raw {
            let hit = Hit {
                page: page_num,
                text,
                bbox,
                context: make_context(&scanned, start, end, args.context),
            };
            if sink.push(hit) {
                break 'pages;
            }
        }
    }

    Ok(())
}

/// The page frame that pdfium text rectangles are expressed in.
///
/// `PdfPage::width()`/`height()` report the page's **displayed** size, i.e.
/// after the intrinsic `/Rotate` has been applied, while `FPDFText_GetRect`
/// returns coordinates in the **unrotated** media-box space. On an A4 page
/// with `/Rotate 90` the measured values are `width() = 841.89`,
/// `height() = 595.28`, yet text rects reach `top = 781.23` — so the rect space
/// is 841.89 points tall, which is the media box height and equals the
/// *reported width*. Flipping against `height()` would give a negative `y`.
///
/// The `bbox` is therefore reported in the page's unrotated PDF coordinate
/// frame, shifted to a top-left origin: `y = rect_space_height - rect.top()`.
struct PageFrame {
    /// Width of the space pdfium text rects live in, in points.
    rect_space_width: f32,
    /// Height of the space pdfium text rects live in, in points.
    rect_space_height: f32,
}

impl PageFrame {
    fn new(page: &PdfPage<'_>) -> Self {
        let rotation = match page.rotation() {
            Ok(PdfPageRenderRotation::Degrees90) => 90,
            Ok(PdfPageRenderRotation::Degrees180) => 180,
            Ok(PdfPageRenderRotation::Degrees270) => 270,
            _ => 0,
        };
        // A quarter turn transposes width and height, so the unrotated media
        // box extents are what `width()`/`height()` report in swapped order.
        let (rect_space_width, rect_space_height) = if rotation == 90 || rotation == 270 {
            (page.height().value, page.width().value)
        } else {
            (page.width().value, page.height().value)
        };
        Self {
            rect_space_width,
            rect_space_height,
        }
    }

    /// Converts a pdfium rect (bottom-left origin) into the CLI's
    /// top-left-origin bbox in PDF points, rounded to two decimals.
    ///
    /// Returns `None` when the rectangle is degenerate or falls outside the
    /// page: a malformed text layer (seen in the wild, e.g. coordinates in the
    /// tens of thousands of points) must yield `"bbox": null` rather than
    /// coordinates that look authoritative but are meaningless.
    fn to_bbox(&self, rect: &PdfRect) -> Option<Bbox> {
        let (left, bottom, right, top) = (
            rect.left().value,
            rect.bottom().value,
            rect.right().value,
            rect.top().value,
        );
        if !left.is_finite() || !bottom.is_finite() || !right.is_finite() || !top.is_finite() {
            return None;
        }

        let width = right - left;
        let height = top - bottom;
        if width <= 0.0 || height <= 0.0 {
            return None;
        }

        // Single vertical flip into the CLI's top-left origin convention.
        let x = left;
        let y = self.rect_space_height - top;
        if !x.is_finite() || !y.is_finite() {
            return None;
        }

        // Generous tolerance so a glyph overhanging the page edge is kept,
        // while nonsense coordinates far outside the page are rejected.
        const SLACK: f32 = 2.0;
        if x < -SLACK
            || y < -SLACK
            || x + width > self.rect_space_width + SLACK
            || y + height > self.rect_space_height + SLACK
        {
            return None;
        }

        Some(Bbox {
            x: round2(x),
            y: round2(y),
            width: round2(width),
            height: round2(height),
        })
    }
}

/// Scans the page text into a plain `String` plus the pdfium character index
/// of every character, and the byte offset of every character boundary.
fn scan_page(text: &PdfPageText<'_>) -> (String, Vec<usize>, Vec<usize>) {
    let chars = text.chars();
    let mut scanned = String::new();
    let mut indices: Vec<usize> = Vec::new();
    let mut byte_starts: Vec<usize> = Vec::new();

    for index in 0..chars.len() {
        let Ok(character) = chars.get(index) else {
            continue;
        };
        let Some(ch) = character.unicode_char() else {
            // Lone surrogate / unrepresentable code unit: skip it, but keep the
            // remaining indices so ranges still cover the whole match.
            continue;
        };
        byte_starts.push(scanned.len());
        scanned.push(ch);
        indices.push(index);
    }
    byte_starts.push(scanned.len());

    (scanned, indices, byte_starts)
}

/// Matches a literal pattern with pdfium's own search engine.
#[allow(clippy::too_many_arguments)]
fn literal_matches(
    text: &PdfPageText<'_>,
    needle: &str,
    regex: &Regex,
    ignore_case: bool,
    scanned: &str,
    indices: &[usize],
    byte_starts: &[usize],
    frame: &PageFrame,
) -> Vec<(usize, usize, String, Option<Bbox>)> {
    let options = PdfSearchOptions::new().match_case(!ignore_case);
    let Ok(search) = text.search(needle, &options) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    // Fallback cursor for the (rare) case where pdfium cannot hand back the
    // character indices for a result.
    let mut cursor = 0usize;

    while let Some(segments) = search.find_next() {
        let bounds = union_of_segments(&segments);
        let bbox = bounds.and_then(|rect| frame.to_bbox(&rect));

        let span = bounds
            .and_then(|rect| text.chars_inside_rect(rect).ok())
            .and_then(
                |chars| match (chars.first_char_index(), chars.last_char_index()) {
                    (Some(first), Some(last)) => {
                        match (
                            ordinal_of_index(indices, first),
                            ordinal_of_index(indices, last),
                        ) {
                            (Some(start), Some(end)) if end >= start => Some((start, end + 1)),
                            _ => None,
                        }
                    }
                    _ => None,
                },
            );

        match span {
            Some((start, mut end)) => {
                // `chars_inside_rect` returns every character inside the
                // rectangle, which can include a trailing space pdfium's own
                // search already excluded. Clamp to the needle's length.
                let expected = needle.chars().count();
                if end - start > expected {
                    end = start + expected;
                }
                out.push((
                    byte_starts[start],
                    byte_starts[end],
                    scanned[byte_starts[start]..byte_starts[end]].to_string(),
                    bbox,
                ));
            }
            None => {
                // No character indices: still report the hit, using the
                // pattern text and a best-effort textual position.
                let found = regex.find_at(scanned, cursor.min(scanned.len()));
                match found {
                    Some(m) => {
                        cursor = m.end();
                        out.push((
                            m.start(),
                            m.end(),
                            scanned[m.start()..m.end()].to_string(),
                            bbox,
                        ));
                    }
                    None => out.push((0, 0, needle.to_string(), bbox)),
                }
            }
        }
    }

    out
}

/// Matches a regular expression over the page's plain text and recovers the
/// bounding box from the corresponding pdfium character range.
fn regex_matches(
    regex: &Regex,
    scanned: &str,
    indices: &[usize],
    byte_starts: &[usize],
    text: &PdfPageText<'_>,
    frame: &PageFrame,
) -> Vec<(usize, usize, String, Option<Bbox>)> {
    let mut out = Vec::new();

    for found in regex.find_iter(scanned) {
        let (start, end) = (found.start(), found.end());
        if start == end {
            // Zero-width match: no geometry and no sensible text.
            continue;
        }

        let start_ord = ordinal_of_byte(byte_starts, start);
        let end_ord = ordinal_of_byte(byte_starts, end);
        if end_ord <= start_ord || end_ord > indices.len() {
            out.push((start, end, scanned[start..end].to_string(), None));
            continue;
        }

        let first_index = indices[start_ord];
        let last_index = indices[end_ord - 1];
        let count = last_index.saturating_sub(first_index) + 1;

        let bbox = union_of_segments(&text.segments_subset(first_index, count))
            .and_then(|rect| frame.to_bbox(&rect));

        out.push((start, end, scanned[start..end].to_string(), bbox));
    }

    out
}

/// Maps a pdfium character index to its ordinal in the scanned string.
fn ordinal_of_index(indices: &[usize], index: usize) -> Option<usize> {
    indices.binary_search(&index).ok()
}

/// Maps a byte offset in the scanned string to a character ordinal.
fn ordinal_of_byte(byte_starts: &[usize], offset: usize) -> usize {
    byte_starts.partition_point(|&start| start < offset)
}

/// Union of every segment's bounds, or `None` when no segment is usable.
fn union_of_segments(segments: &PdfPageTextSegments<'_>) -> Option<PdfRect> {
    let mut union: Option<(f32, f32, f32, f32)> = None;

    for segment in segments.iter() {
        let rect = segment.bounds();
        let (left, bottom, top, right) = (
            rect.left().value,
            rect.bottom().value,
            rect.top().value,
            rect.right().value,
        );
        if !left.is_finite() || !bottom.is_finite() || !top.is_finite() || !right.is_finite() {
            continue;
        }

        union = Some(match union {
            None => (left, bottom, top, right),
            Some((l, b, t, r)) => (l.min(left), b.min(bottom), t.max(top), r.max(right)),
        });
    }

    union.map(|(left, bottom, top, right)| PdfRect::new_from_values(bottom, left, top, right))
}

fn round2(value: f32) -> f64 {
    ((value as f64) * 100.0).round() / 100.0
}

fn pdfium_error(err: PdfiumError) -> PdfError {
    match err {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            PdfError::PasswordRequired
        }
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FormatError)
        | PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::SecurityError)
        | PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FileError) => {
            PdfError::ParseFailed {
                reason: format!("pdfium could not open the document: {err}"),
            }
        }
        other => PdfError::Operation {
            reason: format!("pdfium error while searching: {other}"),
        },
    }
}

// ---------------------------------------------------------------------------
// plain-text fallback strategy
// ---------------------------------------------------------------------------

fn search_plain_text(
    bytes: &[u8],
    regex: &Regex,
    args: &SearchArgs<'_>,
    pages: &[u32],
    sink: &mut Sink,
) -> Result<(), PdfError> {
    // `extract_text_from_mem` joins every page into one string without a
    // reliable separator, so the page-aware API is used instead: hit page
    // numbers must be correct even when geometry is unavailable.
    let page_texts = match args.password {
        Some(password) if !password.is_empty() => {
            pdf_extract::extract_text_from_mem_by_pages_encrypted(bytes, password)
        }
        _ => pdf_extract::extract_text_from_mem_by_pages(bytes),
    }
    .map_err(|err| PdfError::ParseFailed {
        reason: format!("Failed to extract text from `{}`: {err}", args.input),
    })?;

    for &page_num in pages {
        sink.pages_searched += 1;
        let text = page_texts
            .get((page_num - 1) as usize)
            .map(String::as_str)
            .unwrap_or_default();

        if text.trim().is_empty() {
            continue;
        }
        sink.had_text = true;

        for found in regex.find_iter(text) {
            let (start, end) = (found.start(), found.end());
            if start == end {
                continue;
            }

            let hit = Hit {
                page: page_num,
                text: text[start..end].to_string(),
                bbox: None,
                context: make_context(text, start, end, args.context),
            };
            if sink.push(hit) {
                return Ok(());
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// context rendering
// ---------------------------------------------------------------------------

/// Builds the `context` string: up to `window` characters on either side of
/// the match, with runs of whitespace collapsed to single spaces and `...`
/// marking a snippet that was cut.
fn make_context(text: &str, start: usize, end: usize, window: usize) -> String {
    let before: Vec<char> = text[..start].chars().rev().take(window).collect();
    let after: Vec<char> = text[end..].chars().take(window).collect();

    let cut_left = window == 0 || text[..start].chars().count() > window;
    let cut_right = window == 0 || text[end..].chars().count() > window;

    let mut snippet = String::new();
    if cut_left {
        snippet.push_str("...");
    }
    snippet.extend(before.iter().rev());
    snippet.push_str(&text[start..end]);
    snippet.extend(after.iter());
    if cut_right {
        snippet.push_str("...");
    }

    snippet.split_whitespace().collect::<Vec<_>>().join(" ")
}
