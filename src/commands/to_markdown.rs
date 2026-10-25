//! `to-markdown` — turn a PDF into Markdown.
//!
//! The conversion is geometry driven: pdfium gives us one box per glyph, and
//! every structural decision (line, space, heading, list, table, paragraph) is
//! made from those boxes, never from bare text order. That is what lets a
//! multi-column or table-heavy page come out readable instead of as one run-on
//! blob.
//!
//! Pipeline per page:
//!
//! 1. Collect one [`Glyph`] per text character (position, size, character).
//! 2. Group glyphs into [`Line`]s by vertical overlap (≈30% of the median glyph
//!    height), ordered top→bottom then left→right.
//! 3. Measure the page: body font size and median glyph width, both used as the
//!    yardstick for every later heuristic.
//! 4. Split lines into cells at wide horizontal gaps or literal `|` characters,
//!    then promote runs of ≥2 multi-cell lines into a Markdown table.
//! 5. Classify the remaining lines: heading (size ratio vs body, short line, or
//!    a literal `#` run), list item (bullet/number marker, indent from x offset),
//!    or body text.
//! 6. Join consecutive body lines into a paragraph while the geometry still
//!    agrees (same left edge, same size, small vertical gap, previous line does
//!    not end a sentence).
//! 7. Render the blocks as Markdown, wrapped in `<!-- page N -->` markers.
//!
//! `plain` mode skips steps 4–6 and emits one paragraph per text line.

use crate::error::PdfError;
use crate::io;
use crate::output::CommandResult;
use pdfium_render::prelude::*;
use std::cmp::Ordering;
use std::path::Path;

pub struct ToMarkdownArgs<'a> {
    pub input: &'a str,
    pub output: Option<&'a str>,
    pub pages: Option<&'a str>,
    pub mode: &'a str,
    pub password: Option<&'a str>,
    pub pdfium_path: Option<&'a str>,
}

/// Heading levels are chosen by the line's font size relative to body text.
const H1_RATIO: f32 = 1.9;
const H2_RATIO: f32 = 1.5;
const H3_RATIO: f32 = 1.25;
/// A line that long is body text even if it is set in a larger font.
const HEADING_MAX_CHARS: usize = 80;
/// Glyphs share a line when they overlap vertically by this fraction of the
/// page's median glyph height.
const LINE_OVERLAP: f32 = 0.3;
/// A horizontal gap wider than this fraction of the median glyph width no longer
/// belongs to the same word; it becomes a space.
const SPACE_GAP: f32 = 0.25;
/// A gap this many median glyph widths wide separates table cells.
const CELL_GAP: f32 = 2.5;
/// Two lines join into one paragraph only if the vertical gap stays below this
/// fraction of the line height.
const PARA_GAP: f32 = 0.6;
/// Stop nesting list items beyond this depth; deeper indents are noise.
const MAX_LIST_INDENT: usize = 4;

/// Which flavour of Markdown the caller asked for.
#[derive(Debug, Clone, Copy)]
enum Mode {
    /// Structure-aware Markdown (the default).
    Doc,
    /// One paragraph per extracted text line, no heuristics.
    Plain,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Doc => "doc",
            Mode::Plain => "plain",
        }
    }
}

fn parse_mode(mode: &str) -> Result<Mode, PdfError> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "" | "doc" => Ok(Mode::Doc),
        "plain" => Ok(Mode::Plain),
        other => Err(PdfError::Operation {
            reason: format!(
                "Unsupported --mode '{other}'. Valid modes: doc (structure-aware Markdown), \
                 plain (one paragraph per text line)."
            ),
        }),
    }
}

/// Describes how much structure was recovered; reported in `details`.
#[derive(Default)]
struct Stats {
    headings: usize,
    paragraphs: usize,
    lists: usize,
    tables: usize,
}

/// One text character with the box it occupies on the page.
#[derive(Debug, Clone, Copy)]
struct Glyph {
    ch: char,
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
    size: f32,
}

impl Glyph {
    fn height(&self) -> f32 {
        (self.top - self.bottom).max(0.0)
    }
}

/// A visually coherent run of glyphs on a single baseline band.
#[derive(Debug, Default)]
struct Line {
    glyphs: Vec<Glyph>,
    top: f32,
    bottom: f32,
}

impl Line {
    fn from_glyph(glyph: Glyph) -> Self {
        let mut line = Self::default();
        line.add(glyph);
        line
    }

    fn add(&mut self, glyph: Glyph) {
        if self.glyphs.is_empty() {
            self.top = glyph.top;
            self.bottom = glyph.bottom;
        } else {
            self.top = self.top.max(glyph.top);
            self.bottom = self.bottom.min(glyph.bottom);
        }
        self.glyphs.push(glyph);
    }

    /// Vertical overlap in points; positive means the glyph belongs to this band.
    fn overlap(&self, glyph: &Glyph) -> f32 {
        self.top.min(glyph.top) - self.bottom.max(glyph.bottom)
    }

    fn left(&self) -> f32 {
        self.glyphs
            .iter()
            .map(|g| g.left)
            .fold(f32::INFINITY, f32::min)
    }

    fn height(&self) -> f32 {
        (self.top - self.bottom).max(0.1)
    }

    fn size(&self) -> f32 {
        let mut sizes: Vec<f32> = self
            .glyphs
            .iter()
            .map(|g| g.size)
            .filter(|s| *s > 0.0)
            .collect();
        if sizes.is_empty() {
            0.0
        } else {
            median(&mut sizes)
        }
    }

    fn text(&self, space_gap: f32) -> String {
        glyphs_text(&self.glyphs, space_gap)
    }
}

/// A table cell candidate: its left edge and its text.
struct Cell {
    left: f32,
    text: String,
}

/// One rendered Markdown block of a page.
enum Block {
    Heading { level: u8, text: String },
    Paragraph(String),
    ListItem { indent: usize, text: String },
    Table(Vec<Vec<String>>),
}

fn cmp_f32(a: f32, b: f32) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

/// Median of a sample. Sorts in place; returns 0 for an empty sample, which the
/// callers always guard against.
fn median(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| cmp_f32(*a, *b));
    let mid = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    }
}

/// Rebuild a text run from glyph boxes, inserting a space wherever the geometric
/// gap says the words are separated (PDF text often carries no space glyphs).
fn glyphs_text(glyphs: &[Glyph], space_gap: f32) -> String {
    let mut out = String::new();
    let mut prev_right: Option<f32> = None;
    for glyph in glyphs {
        if glyph.ch.is_whitespace() || glyph.ch == '\u{00a0}' {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
            prev_right = Some(glyph.right);
            continue;
        }
        if let Some(prev) = prev_right {
            if glyph.left - prev > space_gap && !out.ends_with(' ') {
                out.push(' ');
            }
        }
        out.push(glyph.ch);
        prev_right = Some(glyph.right);
    }
    out.trim().to_string()
}

/// Read every glyph of `page`, skipping characters pdfium cannot map to Unicode
/// or cannot place on the page.
fn page_glyphs(page: &PdfPage<'_>, page_no: u32) -> Result<Vec<Glyph>, PdfError> {
    let text = page.text().map_err(|e| PdfError::Operation {
        reason: format!("Failed to read the text layer of page {page_no}: {e}"),
    })?;

    let mut glyphs: Vec<Glyph> = Vec::new();
    for ch in text.chars().iter() {
        let c = match ch.unicode_char() {
            Some(c) => c,
            None => continue,
        };
        if c.is_control() {
            continue;
        }
        // Every edge comes from the *loose* box. It spans the character's full
        // advance box (width and ascent/descent), which is what the layout
        // heuristics want: neighbouring glyphs on a line touch horizontally
        // instead of leaving ink-tight gaps around narrow letters like "l" or
        // "i", and all glyphs of one run share a uniform vertical band even when
        // their ink differs (".", "一", superscripts).
        let loose = match ch.loose_bounds() {
            Ok(b) => b,
            Err(_) => continue,
        };
        let left = loose.left().value;
        let right = loose.right().value;
        let bottom = loose.bottom().value;
        let top = loose.top().value;
        if !left.is_finite() || !top.is_finite() || !right.is_finite() || !bottom.is_finite() {
            continue;
        }
        let size = ch.scaled_font_size().value;
        glyphs.push(Glyph {
            ch: c,
            left,
            right: right.max(left),
            bottom,
            top: top.max(bottom),
            size: if size.is_finite() { size.max(0.0) } else { 0.0 },
        });
    }
    Ok(glyphs)
}

/// Group glyphs into lines by vertical overlap, then order the lines in reading
/// order (top to bottom, left to right).
fn group_lines(mut glyphs: Vec<Glyph>) -> Vec<Line> {
    if glyphs.is_empty() {
        return Vec::new();
    }

    let mut heights: Vec<f32> = glyphs
        .iter()
        .map(|g| g.height())
        .filter(|h| *h > 0.5)
        .collect();
    let median_h = if heights.is_empty() {
        10.0
    } else {
        median(&mut heights).max(1.0)
    };
    let min_overlap = LINE_OVERLAP * median_h;

    glyphs.sort_by(|a, b| {
        cmp_f32(b.top, a.top).then_with(|| cmp_f32(a.left, b.left))
    });

    let mut lines: Vec<Line> = Vec::new();
    for glyph in glyphs {
        let mut best: Option<(usize, f32)> = None;
        for (index, line) in lines.iter().enumerate() {
            let overlap = line.overlap(&glyph);
            if overlap >= min_overlap && best.is_none_or(|(_, b)| overlap > b) {
                best = Some((index, overlap));
            }
        }
        match best {
            Some((index, _)) => lines[index].add(glyph),
            None => lines.push(Line::from_glyph(glyph)),
        }
    }

    for line in &mut lines {
        line.glyphs.sort_by(|a, b| cmp_f32(a.left, b.left));
    }
    lines.sort_by(|a, b| cmp_f32(b.top, a.top).then_with(|| cmp_f32(a.left(), b.left())));
    lines
}

/// Median font size of the whole page — the yardstick for "body text".
fn body_size(glyphs: &[Glyph]) -> f32 {
    let mut sizes: Vec<f32> = glyphs
        .iter()
        .map(|g| g.size)
        .filter(|s| *s > 0.5)
        .collect();
    if sizes.is_empty() {
        10.0
    } else {
        median(&mut sizes).max(1.0)
    }
}

/// Median advance width of a non-space glyph.
fn char_width(glyphs: &[Glyph]) -> f32 {
    let mut widths: Vec<f32> = glyphs
        .iter()
        .filter(|g| !g.ch.is_whitespace() && g.right > g.left)
        .map(|g| g.right - g.left)
        .collect();
    if widths.is_empty() {
        5.0
    } else {
        median(&mut widths).max(0.5)
    }
}

/// Split a line into cells at column boundaries.
///
/// Two independent signals mark a boundary: a geometric gap wide enough to be a
/// column gutter, and an explicit `|` character (a PDF generated from Markdown
/// carries the pipes through as ordinary text). The returned flag says whether a
/// literal pipe was used, because pipe-delimited rows stay aligned in a
/// proportional font only by column count, not by x position.
fn split_cells(line: &Line, cell_gap: f32, space_gap: f32) -> (Vec<Cell>, bool) {
    let mut cells: Vec<Cell> = Vec::new();
    let mut current: Vec<Glyph> = Vec::new();
    let mut prev_right: Option<f32> = None;
    let mut saw_pipe = false;

    let push_cell = |cells: &mut Vec<Cell>, current: &mut Vec<Glyph>| {
        let text = glyphs_text(current, space_gap);
        if !text.is_empty() {
            let left = current.first().map_or(0.0, |g| g.left);
            cells.push(Cell { left, text });
        }
        current.clear();
    };

    for glyph in &line.glyphs {
        if glyph.ch == '|' {
            saw_pipe = true;
            push_cell(&mut cells, &mut current);
            prev_right = None;
            continue;
        }
        let wide_gap = match prev_right {
            Some(prev) => glyph.left - prev > cell_gap,
            None => false,
        };
        if wide_gap && !current.is_empty() {
            push_cell(&mut cells, &mut current);
        }
        current.push(*glyph);
        prev_right = Some(glyph.right);
    }
    push_cell(&mut cells, &mut current);
    (cells, saw_pipe)
}

/// `# Heading` style markers carried through as literal text.
fn literal_heading(text: &str) -> Option<(u8, String)> {
    let trimmed = text.trim_start();
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = trimmed.get(hashes..)?;
    // Require the separating space so "#hashtag" stays body text.
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    if rest.is_empty() {
        return None;
    }
    Some((hashes as u8, rest.to_string()))
}

/// A Markdown table alignment row (`|---|---|`) carries no content.
fn is_table_separator_row(cells: &[Cell]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let trimmed = cell.text.trim();
            !trimmed.is_empty()
                && trimmed
                    .chars()
                    .all(|c| c == '-' || c == ':' || c.is_whitespace())
        })
}

/// Strip a bullet or enumerator from a line, returning the item body.
fn detect_list_marker(text: &str) -> Option<String> {
    let trimmed = text.trim_start();

    for bullet in ['\u{2022}', '\u{25e6}', '\u{25aa}', '\u{00b7}'] {
        if let Some(rest) = trimmed.strip_prefix(bullet) {
            let rest = rest.trim_start();
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    for bullet in ['-', '*'] {
        if let Some(rest) = trimmed.strip_prefix(bullet) {
            // Require the separator space so "----" or "-5 °C" stay body text.
            if rest.starts_with(char::is_whitespace) {
                let rest = rest.trim_start();
                if !rest.is_empty() {
                    return Some(rest.to_string());
                }
            }
        }
    }
    if let Some(rest) = strip_number_marker(trimmed) {
        return Some(rest);
    }
    if let Some(rest) = strip_alpha_marker(trimmed) {
        return Some(rest);
    }
    None
}

/// `1.` / `1)` style enumerators.
fn strip_number_marker(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut digits = 0;
    while digits < bytes.len() && bytes[digits].is_ascii_digit() {
        digits += 1;
    }
    if digits == 0 || digits > 3 {
        return None;
    }
    match bytes.get(digits) {
        Some(b'.') | Some(b')') => {}
        _ => return None,
    }
    let rest = text.get(digits + 1..)?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    }
}

/// `a.` / `a)` style enumerators.
fn strip_alpha_marker(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_lowercase() {
        return None;
    }
    match bytes[1] {
        b'.' | b')' => {}
        _ => return None,
    }
    let rest = text.get(2..)?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    }
}

/// Nesting depth of a list item, derived from how far its bullet sits past the
/// page's body-text left margin.
fn indent_level(left: f32, margin: f32, body: f32) -> usize {
    let step = (body * 2.0).max(8.0);
    let offset = (left - margin).max(0.0);
    ((offset / step).round() as usize).min(MAX_LIST_INDENT)
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x303F
        | 0x3040..=0x30FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xAC00..=0xD7AF
        | 0xF900..=0xFAFF
        | 0xFF00..=0xFFEF)
}

/// Join two text fragments: CJK pairs need no space, Latin words do.
fn join_fragments(acc: &mut String, next: &str) {
    let last = acc.chars().last();
    let first = next.chars().next();
    let needs_space = match (last, first) {
        (Some(a), Some(b)) => !(is_cjk(a) && is_cjk(b)),
        _ => true,
    };
    if needs_space && !acc.ends_with(' ') && !next.starts_with(' ') {
        acc.push(' ');
    }
    acc.push_str(next);
}

fn ends_sentence(text: &str) -> bool {
    text.chars()
        .last()
        .is_none_or(|c| matches!(c, '.' | '!' | '?' | ':' | ';' | '\u{3002}'))
}

/// Turn the lines of one page into Markdown blocks (doc mode).
fn build_blocks(lines: &[Line], body: f32, char_w: f32) -> Vec<Block> {
    let space_gap = SPACE_GAP * char_w;
    let cell_gap = CELL_GAP * char_w;

    let margin = lines
        .iter()
        .map(|l| l.left())
        .filter(|x| x.is_finite())
        .fold(f32::INFINITY, f32::min);
    let margin = if margin.is_finite() { margin } else { 0.0 };

    let mut blocks: Vec<Block> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = &lines[index];
        let text = line.text(space_gap);
        if text.is_empty() {
            index += 1;
            continue;
        }

        // Literal Markdown markers win over geometry: a PDF produced *from*
        // Markdown carries its `#` and `|` through as ordinary text at body
        // size, so the font-ratio test below would never see a heading.
        if let Some((level, heading_text)) = literal_heading(&text) {
            blocks.push(Block::Heading {
                level,
                text: heading_text,
            });
            index += 1;
            continue;
        }

        let size = match line.size() {
            0.0 => body,
            s => s,
        };
        let ratio = size / body;

        if ratio >= H3_RATIO
            && text.chars().count() <= HEADING_MAX_CHARS
            && detect_list_marker(&text).is_none()
        {
            let level = if ratio >= H1_RATIO {
                1
            } else if ratio >= H2_RATIO {
                2
            } else {
                3
            };
            blocks.push(Block::Heading { level, text });
            index += 1;
            continue;
        }

        if let Some(item) = detect_list_marker(&text) {
            let indent = indent_level(line.left(), margin, body);
            blocks.push(Block::ListItem { indent, text: item });
            index += 1;
            continue;
        }

        // Table: a run of lines that all split into the same columns. Rows are
        // matched by cell count and, when the split was geometric, by shared
        // column origins — every line on the page is already one baseline band,
        // so a new baseline is automatically a new row.
        let (cells, piped) = split_cells(line, cell_gap, space_gap);
        if cells.len() >= 2 && cells.len() <= 8 {
            let width = cells.len();
            let starts: Vec<f32> = cells.iter().map(|c| c.left).collect();
            let mut rows: Vec<Vec<String>> =
                vec![cells.iter().map(|c| c.text.clone()).collect()];
            let mut next = index + 1;
            while next < lines.len() {
                let gap = lines[next - 1].bottom - lines[next].top;
                if gap > body * 1.5 {
                    break;
                }
                if literal_heading(&lines[next].text(space_gap)).is_some() {
                    break;
                }
                if detect_list_marker(&lines[next].text(space_gap)).is_some() {
                    break;
                }
                let (more, row_piped) = split_cells(&lines[next], cell_gap, space_gap);
                if more.len() != width {
                    break;
                }
                // A pipe-delimited run is defined by its column count; a
                // geometric one must also keep the same left edges.
                if row_piped && piped {
                    if is_table_separator_row(&more) {
                        next += 1;
                        continue;
                    }
                } else {
                    let aligned = more
                        .iter()
                        .zip(starts.iter())
                        .all(|(cell, start)| (cell.left - *start).abs() <= char_w * 1.5);
                    if !aligned {
                        break;
                    }
                }
                rows.push(more.iter().map(|c| c.text.clone()).collect());
                next += 1;
            }
            if rows.len() >= 2 {
                blocks.push(Block::Table(rows));
                index = next;
                continue;
            }
        }

        // Everything else is body text: merge following lines while the geometry
        // keeps agreeing with the first line of the paragraph.
        let mut paragraph = text;
        let mut previous = line;
        let mut next = index + 1;
        while next < lines.len() {
            let candidate = &lines[next];
            let candidate_text = candidate.text(space_gap);
            if candidate_text.is_empty() {
                break;
            }
            let candidate_size = match candidate.size() {
                0.0 => body,
                s => s,
            };
            if candidate_size / body >= H3_RATIO {
                break;
            }
            if literal_heading(&candidate_text).is_some() {
                break;
            }
            if detect_list_marker(&candidate_text).is_some() {
                break;
            }
            if split_cells(candidate, cell_gap, space_gap).0.len() >= 2 {
                break;
            }
            if (candidate.left() - previous.left()).abs() > char_w * 1.5 {
                break;
            }
            if (candidate_size - size).abs() > size * 0.15 + 0.5 {
                break;
            }
            let gap = previous.bottom - candidate.top;
            if gap > PARA_GAP * previous.height().max(body) {
                break;
            }
            if ends_sentence(&paragraph) {
                break;
            }
            if paragraph.ends_with('-') {
                paragraph.pop();
                paragraph.push_str(&candidate_text);
            } else {
                join_fragments(&mut paragraph, &candidate_text);
            }
            previous = candidate;
            next += 1;
        }
        blocks.push(Block::Paragraph(paragraph));
        index = next.max(index + 1);
    }

    blocks
}

fn format_table_row(cells: &[String], columns: usize) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(columns);
    for column in 0..columns {
        let cell = cells
            .get(column)
            .map(|c| c.trim().replace('|', "\\|"))
            .unwrap_or_default();
        parts.push(if cell.is_empty() { " ".to_string() } else { cell });
    }
    format!("| {} |", parts.join(" | "))
}

fn render_blocks(blocks: &[Block]) -> String {
    let mut out = String::new();
    // List items stay on consecutive lines so they render as one Markdown list
    // instead of a run of one-item lists.
    let mut in_list = false;
    for block in blocks {
        match block {
            Block::Heading { level, text } => {
                if !out.is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str(&format!("{} {}", "#".repeat(*level as usize), text));
                in_list = false;
            }
            Block::Paragraph(text) => {
                if !out.is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str(text);
                in_list = false;
            }
            Block::ListItem { indent, text } => {
                if !out.is_empty() {
                    out.push_str(if in_list { "\n" } else { "\n\n" });
                }
                out.push_str(&format!("{}- {}", "  ".repeat(*indent), text));
                in_list = true;
            }
            Block::Table(rows) => {
                if rows.is_empty() {
                    continue;
                }
                let columns = rows.iter().map(|r| r.len()).max().unwrap_or(0);
                if columns == 0 {
                    continue;
                }
                if !out.is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str(&format_table_row(&rows[0], columns));
                out.push('\n');
                let separator = vec!["---".to_string(); columns];
                out.push_str(&format_table_row(&separator, columns));
                for row in rows.iter().skip(1) {
                    out.push('\n');
                    out.push_str(&format_table_row(row, columns));
                }
                in_list = false;
            }
        }
    }
    out
}

fn convert_doc_page(glyphs: &[Glyph], stats: &mut Stats) -> String {
    if glyphs.is_empty() {
        return String::new();
    }
    let body = body_size(glyphs);
    let char_w = char_width(glyphs);
    let lines = group_lines(glyphs.to_vec());
    let blocks = build_blocks(&lines, body, char_w);
    for block in &blocks {
        match block {
            Block::Heading { .. } => stats.headings += 1,
            Block::Paragraph(_) => stats.paragraphs += 1,
            Block::ListItem { .. } => stats.lists += 1,
            Block::Table(_) => stats.tables += 1,
        }
    }
    render_blocks(&blocks)
}

fn convert_plain_page(glyphs: &[Glyph]) -> String {
    if glyphs.is_empty() {
        return String::new();
    }
    let char_w = char_width(glyphs);
    let space_gap = SPACE_GAP * char_w;
    let lines = group_lines(glyphs.to_vec());
    let mut paragraphs: Vec<String> = Vec::new();
    for line in &lines {
        let text = line.text(space_gap);
        if !text.is_empty() {
            paragraphs.push(text);
        }
    }
    paragraphs.join("\n\n")
}

fn human_size(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let value = bytes as f64;
    if value >= MB {
        format!("{:.1} MB", value / MB)
    } else if value >= KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Convert a PDF into Markdown.
///
/// See the module docs for the per-page pipeline: glyph boxes → lines → page
/// metrics → tables/lists/headings → paragraph joining → Markdown. Every choice
/// is derived from geometry with fixed thresholds, so the same input always
/// produces the same document (no hash-map iteration order anywhere).
///
/// `args.output` decides the destination: with a path the Markdown is written
/// there (`-` means stdout) and a one-line summary is printed instead; without a
/// path the Markdown itself is the command output and is also mirrored into
/// `details.markdown` so `--json` callers never see a truncated document.
/// `dry_run` runs the whole conversion, reports every field, and writes nothing.
pub fn run(args: ToMarkdownArgs<'_>, dry_run: bool) -> Result<CommandResult, PdfError> {
    let mode = parse_mode(args.mode)?;

    // Read once, then reuse for both the password-aware validation below and the
    // pdfium layout pass, so `--input -` (stdin) works too.
    let bytes = io::read_bytes(args.input)?;

    // Validating through `io::load_doc_bytes` gives encrypted inputs the proper
    // PASSWORD_REQUIRED / WRONG_PASSWORD code instead of a generic parse failure.
    let doc = io::load_doc_bytes(&bytes, args.password)?;
    let total_pages = doc.get_pages().len() as u32;
    if total_pages == 0 {
        return Err(PdfError::Operation {
            reason: format!("{} contains no pages", args.input),
        });
    }
    let selected = io::parse_page_range(args.pages, total_pages)?;
    if selected.is_empty() {
        return Err(PdfError::InvalidPageRangeReason {
            spec: args.pages.unwrap_or_default().to_string(),
            reason: "page range selects no pages".to_string(),
        });
    }

    let pdfium = io::bind_pdfium(args.pdfium_path)?;
    let pdf = pdfium
        .load_pdf_from_byte_vec(bytes, args.password)
        .map_err(|e| PdfError::ParseFailed {
            reason: format!("pdfium could not open {}: {e}", args.input),
        })?;

    let mut stats = Stats::default();
    let mut pages: Vec<String> = Vec::with_capacity(selected.len());
    let mut visible_chars = 0usize;

    for page_no in &selected {
        let page_index = u16::try_from(page_no - 1).map_err(|_| PdfError::Operation {
            reason: format!("Page {page_no} is out of pdfium's supported page index range"),
        })?;
        let page = pdf
            .pages()
            .get(page_index)
            .map_err(|e| PdfError::Operation {
                reason: format!("Failed to load page {page_no}: {e}"),
            })?;
        let glyphs = page_glyphs(&page, *page_no)?;
        visible_chars += glyphs.iter().filter(|g| !g.ch.is_whitespace()).count();
        let body = match mode {
            Mode::Doc => convert_doc_page(&glyphs, &mut stats),
            Mode::Plain => convert_plain_page(&glyphs),
        };
        pages.push(if body.is_empty() {
            format!("<!-- page {page_no} -->")
        } else {
            format!("<!-- page {page_no} -->\n\n{body}")
        });
    }

    // A scanned PDF has no text layer at all. Saying "success, empty document"
    // silently would be the worst outcome for an agent, so name the remedy.
    if visible_chars == 0 {
        return Err(PdfError::Operation {
            reason: format!(
                "No text layer found in {}. The PDF is probably a scanned image. \
                 pdf-cli does not run OCR: recognise the pages with a separate OCR tool first.",
                args.input
            ),
        });
    }

    let markdown = pages.join("\n\n---\n\n");
    let char_count = markdown.chars().count();
    let size = human_size(markdown.len());

    let mut details = serde_json::json!({
        "input": if io::is_stdio(args.input) { "stdin" } else { args.input },
        "output": match args.output {
            Some(path) => serde_json::Value::String(path.to_string()),
            None => serde_json::Value::Null,
        },
        "mode": mode.as_str(),
        "pages_converted": selected.len(),
        "chars": char_count,
        "headings": stats.headings,
        "paragraphs": stats.paragraphs,
        "lists": stats.lists,
        "tables": stats.tables,
    });

    // stdout case: keep the whole document in the JSON payload as well.
    if args.output.is_none() {
        details["markdown"] = serde_json::json!(markdown);
    }
    if dry_run {
        details["dry_run"] = serde_json::json!(true);
    }

    let summary = format!(
        "converted {} page{} to markdown ({size})",
        selected.len(),
        if selected.len() == 1 { "" } else { "s" }
    );

    match (args.output, dry_run) {
        (Some(path), false) => {
            if let Some(parent) = Path::new(path).parent() {
                if !parent.as_os_str().is_empty() && !io::is_stdio(path) {
                    std::fs::create_dir_all(parent).map_err(|e| PdfError::Operation {
                        reason: format!(
                            "Failed to create output directory {}: {e}",
                            parent.display()
                        ),
                    })?;
                }
            }
            io::write_text(path, &markdown)?;
            Ok(CommandResult::ok(
                "to-markdown",
                Some(format!("{summary} → {path}")),
                details,
            ))
        }
        (Some(path), true) => Ok(CommandResult::ok(
            "to-markdown",
            Some(format!("[dry-run] would write {summary} → {path}")),
            details,
        )),
        (None, _) => Ok(CommandResult::ok("to-markdown", Some(markdown), details)),
    }
}
