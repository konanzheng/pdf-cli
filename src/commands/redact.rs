//! `redact` — cover an existing run of text and print a replacement in its place.
//!
//! ## What this solves
//!
//! Documents that repeat the same few words across dozens of pages — a contract
//! template with a different company name on each copy, an invoice batch with a
//! different number, a report where one figure changed. Doing that by hand in an
//! editor is slow and error-prone; this does it in one pass.
//!
//! ## How it works, and what it cannot do
//!
//! The rectangle occupied by the matched text is filled with an opaque box, then
//! the replacement string is drawn on top at the same origin. Finding the
//! rectangle is `search`'s job: `search::locate` returns pdfium's measured
//! bounding box per hit, and this command only draws.
//!
//! **The original text is still inside the content stream.** A cover box hides
//! it visually, but `extract` / `search` / copy-paste still reach it, and so does
//! any tool that parses the stream. Removing the operators themselves would mean
//! rewriting text-showing runs, which shreds layout on real-world PDFs. So this
//! is a *presentation* fix, not a security control: for genuinely
//! confidential material (an SSN, a password, a signature) flatten or rasterise
//! the page instead, and never rely on this to redact for publication.
//!
//! ## Coordinate conventions
//!
//! `search::locate` reports the box in the CLI's top-left-origin frame, while
//! PDF content streams are bottom-left. The vertical flip is
//! `y_pdf = page_height - (y_top + height)`, computed per page from that page's
//! `/MediaBox`.
//!
//! Only the vertical flip is applied. The horizontal placement is unchanged; on
//! a page carrying `/Rotate 90` the drawn text therefore starts at the right
//! place but is not itself rotated, so rotated pages are reported in
//! `details.note` instead of being silently mangled.

use std::path::Path;

use lopdf::{dictionary, Dictionary, Object, Stream};

use crate::commands::search;
use crate::error::PdfError;
use crate::font;
use crate::io;
use crate::output::CommandResult;

/// One replacement instruction, resolved to coordinates.
struct Fix {
    page: u32,
    /// Cover rectangle in PDF user space (bottom-left origin).
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    replacement: String,
    /// Size to draw the replacement at, in points.
    font_size: f32,
}

/// Escape a string for a PDF literal string `(...)`.
fn escape_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            // A raw newline in a literal is legal but confusing to diff; fold it.
            '\r' => {}
            '\n' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// Validate `--cover-color` and render it as a PDF `rg` operand triple.
///
/// The value is pasted into a content stream, so it must be parsed into numbers
/// rather than trusted: an arbitrary string here would be a content-stream
/// injection.
fn colour_operand(spec: &str) -> Result<String, PdfError> {
    let parts: Vec<&str> = spec
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() != 3 {
        return Err(PdfError::Operation {
            reason: format!(
                "--cover-color must give three 0-1 components as \"r,g,b\", got {spec:?}"
            ),
        });
    }
    let mut out = String::new();
    for part in parts {
        let v: f32 = part.parse().map_err(|_| PdfError::Operation {
            reason: format!("--cover-color component {part:?} is not a number"),
        })?;
        if !(0.0..=1.0).contains(&v) {
            return Err(PdfError::Operation {
                reason: format!("--cover-color component {v} is outside 0-1"),
            });
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format!("{v:.3}"));
    }
    Ok(out)
}

fn media_box(dict: &Dictionary) -> (f32, f32) {
    if let Ok(mb) = dict.get(b"MediaBox").and_then(|o| o.as_array()) {
        if mb.len() >= 4 {
            let n = |o: &Object| match o {
                Object::Integer(v) => *v as f32,
                Object::Real(v) => *v,
                _ => 0.0,
            };
            let w = (n(&mb[2]) - n(&mb[0])).abs();
            let h = (n(&mb[3]) - n(&mb[1])).abs();
            if w > 0.0 && h > 0.0 {
                return (w, h);
            }
        }
    }
    (595.28, 841.89)
}

/// Pick a resource name that cannot collide with the page's existing fonts.
///
/// Real PDFs very often already define `/F1`; reusing it would make the
/// document's own text render with our subsetted font, i.e. as mojibake.
fn free_font_name(doc: &lopdf::Document, page_id: lopdf::ObjectId) -> String {
    let existing: Vec<Vec<u8>> = doc
        .get_object(page_id)
        .ok()
        .and_then(|o| o.as_dict().ok())
        .and_then(|d| d.get(b"Resources").ok())
        .and_then(|o| o.as_dict().ok())
        .and_then(|r| r.get(b"Font").ok())
        .and_then(|o| o.as_dict().ok())
        .map(|f| f.iter().map(|(k, _)| k.clone()).collect())
        .unwrap_or_default();

    let mut n = 1u32;
    loop {
        let candidate = format!("PDFCLIRedact{n}");
        if !existing
            .iter()
            .any(|k| k.as_slice() == candidate.as_bytes())
        {
            return candidate;
        }
        n += 1;
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    input: &str,
    output: &str,
    find: &str,
    replace: &str,
    pages: Option<&str>,
    cover_color: &str,
    font_path: Option<&str>,
    padding: f32,
    password: Option<&str>,
    pdfium_path: Option<&str>,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    if !io::is_stdio(input) && !Path::new(input).exists() {
        return Err(PdfError::FileNotFound {
            path: input.to_string(),
        });
    }
    if find.is_empty() {
        return Err(PdfError::MissingArg {
            arg: "--find".to_string(),
            example:
                "pdf-cli redact --input in.pdf --output out.pdf --find \"Acme\" --replace \"Beta\""
                    .into(),
        });
    }

    // Parse before touching the document so a typo cannot reach a content stream.
    let cover_operand = colour_operand(cover_color)?;

    // pdfium is not optional here: without measured rectangles there is nothing
    // to cover, and guessing a position would wreck the page.
    let hits = search::locate(
        input,
        find,
        false,
        false,
        pages,
        password,
        pdfium_path,
    )
    .map_err(|e| match e {
        PdfError::PdfiumNotFound { .. } => PdfError::Operation {
            reason: "redact needs pdfium to measure where the text sits; it has no fallback. \
                     Pass --pdfium-path <path to pdfium.dll> or place pdfium.dll next to pdf-cli.exe."
                .into(),
        },
        other => other,
    })?;

    if hits.is_empty() {
        return Ok(CommandResult::ok(
            "redact",
            Some(format!("no occurrences of {find:?}; nothing written")),
            serde_json::json!({
                "input": input,
                "find": find,
                "replace": replace,
                "matches": 0,
                "written": false,
                "original_text_retained": true,
            }),
        ));
    }

    let mut doc = io::load_doc(input, password)?;
    let pages_map = doc.get_pages();

    // Drawing the replacement in CJK needs an embedded subset; pure ASCII can
    // use a base-14 font at no cost. Build the subset from every replacement so
    // digits that only appear on later pages are still covered.
    let all_replacements: String = hits
        .iter()
        .map(|h| format!("{replace}{}", h.text))
        .collect::<Vec<_>>()
        .join("");
    let needs_embed = !replace.is_ascii() || font_path.is_some();
    let embedded = if needs_embed {
        Some(font::embed(
            &mut doc,
            &all_replacements,
            font_path,
            "PDFCLIRedact",
        )?)
    } else {
        None
    };

    let fallback_font_id = match &embedded {
        None => Some(doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        })),
        Some(f) => Some(f.font_id),
    };

    // Resolve every hit into a concrete drawing instruction first, so a font
    // that cannot render some replacement fails before the document is touched.
    let mut fixes: Vec<Fix> = Vec::new();
    let mut missing_glyphs: Vec<(u32, String)> = Vec::new();

    for hit in &hits {
        let page_id = match pages_map.get(&hit.page) {
            Some(id) => *id,
            None => continue,
        };
        let page_dict = doc
            .get_object(page_id)
            .and_then(|o| o.as_dict())
            .map_err(|e| PdfError::Operation {
                reason: format!("page {} has no dictionary: {e}", hit.page),
            })?
            .clone();
        let (_, page_height) = media_box(&page_dict);

        if let Some(f) = &embedded {
            if !f.covers(&format!("{replace}{}", hit.text)) {
                missing_glyphs.push((hit.page, hit.text.clone()));
                continue;
            }
        }

        // Flip the top-left-origin box into PDF user space (bottom-left).
        let x = (hit.bbox.x as f32) - padding;
        let width = (hit.bbox.width as f32) + padding * 2.0;
        let height = (hit.bbox.height as f32) + padding * 2.0;
        let y = page_height - (hit.bbox.y as f32) - (hit.bbox.height as f32) - padding;

        fixes.push(Fix {
            page: hit.page,
            x: x.max(0.0),
            y: y.max(0.0),
            width,
            height,
            replacement: replace.to_string(),
            // Match the source glyph height so the patch blends in.
            font_size: (hit.bbox.height as f32).max(1.0),
        });
    }

    if !missing_glyphs.is_empty() {
        let detail = missing_glyphs
            .iter()
            .map(|(p, t)| format!("page {p} ({t:?})"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(PdfError::Operation {
            reason: format!(
                "The chosen font cannot render the replacement text for: {detail}. \
                 Pass --font <path to a .ttf/.ttc that contains these characters>."
            ),
        });
    }

    if dry_run {
        let page_set: Vec<u32> = {
            let mut v: Vec<u32> = fixes.iter().map(|f| f.page).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        return Ok(CommandResult::ok(
            "redact",
            Some(format!(
                "[dry-run] Would replace {} occurrence(s) of {:?} on {} page(s)",
                fixes.len(),
                find,
                page_set.len()
            )),
            serde_json::json!({
                "dry_run": true,
                "input": input,
                "output": output,
                "find": find,
                "replace": replace,
                "occurrences": fixes.len(),
                "pages": page_set,
                "embedded_font": needs_embed,
                "cover_color": cover_color,
            }),
        ));
    }

    // Group the drawing instructions by page so each page gets one extra stream.
    let mut by_page: std::collections::BTreeMap<u32, Vec<&Fix>> = std::collections::BTreeMap::new();
    for fix in &fixes {
        by_page.entry(fix.page).or_default().push(fix);
    }

    let mut rotated_pages: Vec<u32> = Vec::new();
    let mut written_pages = 0u32;

    for (page_num, page_fixes) in &by_page {
        let page_id = match pages_map.get(page_num) {
            Some(id) => *id,
            None => continue,
        };

        // A rotated page's text rects do not map onto the content stream by a
        // vertical flip alone. Report rather than draw in the wrong place.
        let rotation = doc
            .get_object(page_id)
            .ok()
            .and_then(|o| o.as_dict().ok())
            .and_then(|d| d.get(b"Rotate").ok())
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);
        if rotation % 360 != 0 {
            rotated_pages.push(*page_num);
            continue;
        }

        let res_name = free_font_name(&doc, page_id);

        let mut stream_body = String::new();
        for fix in page_fixes {
            // Cover: an opaque filled rectangle over the original glyphs.
            stream_body.push_str(&format!(
                "q\n{} rg\n{} {} {} {} re\nf\nQ\n",
                cover_operand, fix.x, fix.y, fix.width, fix.height
            ));

            // Replacement, drawn at the same origin so it lands in the old place.
            let show = match &embedded {
                Some(f) => format!("<{}> Tj", f.encode(&fix.replacement)),
                None => format!("({}) Tj", escape_literal(&fix.replacement)),
            };
            stream_body.push_str(&format!(
                "q\nBT\n/{res_name} {:.1} Tf\n0 g\n{:.1} {:.1} Td\n{}\nET\nQ\n",
                fix.font_size, fix.x, fix.y, show
            ));
        }

        let content_id = doc.add_object(Stream::new(Dictionary::new(), stream_body.into_bytes()));

        // Register the font and append the stream, preserving existing resources.
        if let Ok(d) = doc.get_object_mut(page_id).and_then(|o| o.as_dict_mut()) {
            let mut res = d
                .get(b"Resources")
                .ok()
                .and_then(|o| o.as_dict().ok())
                .cloned()
                .unwrap_or_default();
            let mut font_dict = res
                .get(b"Font")
                .ok()
                .and_then(|o| o.as_dict().ok())
                .cloned()
                .unwrap_or_default();
            let font_id = match &embedded {
                None => fallback_font_id.expect("font id always set"),
                Some(f) => f.font_id,
            };
            font_dict.set(res_name.as_str(), Object::Reference(font_id));
            res.set("Font", Object::Dictionary(font_dict));
            d.set("Resources", Object::Dictionary(res));

            match d.get(b"Contents").ok().cloned() {
                Some(Object::Array(mut arr)) => {
                    arr.push(Object::Reference(content_id));
                    d.set("Contents", Object::Array(arr));
                }
                Some(orig) => {
                    d.set(
                        "Contents",
                        Object::Array(vec![orig, Object::Reference(content_id)]),
                    );
                }
                None => {
                    d.set("Contents", Object::Reference(content_id));
                }
            }
        }
        written_pages += 1;
    }

    doc.save(output).map_err(|e| PdfError::Operation {
        reason: format!("Failed to write {output}: {e}"),
    })?;

    let mut note = String::new();
    if !rotated_pages.is_empty() {
        note = format!(
            "Skipped rotated page(s) {rotated_pages:?}: their text rectangles do not map onto \
             the content stream by a vertical flip alone, so drawing there would land in the \
             wrong place. Remove the rotation with `rotate --degrees 0` first."
        );
    }

    // Nothing was actually drawn (every hit sat on a rotated page). Writing an
    // output file here would look like success while changing nothing, so fail.
    if written_pages == 0 {
        return Err(PdfError::Operation {
            reason: format!(
                "Found {} occurrence(s) of {:?}, but none could be replaced: {note}",
                fixes.len(),
                find
            ),
        });
    }

    // Report the occurrences that were actually covered, not the ones skipped.
    let applied = fixes
        .iter()
        .filter(|f| !rotated_pages.contains(&f.page))
        .count();

    let summary = format!(
        "Replaced {} occurrence(s) of {:?} on {} page(s) → {}",
        applied, find, written_pages, output
    );

    Ok(CommandResult::ok(
        "redact",
        Some(if note.is_empty() {
            summary.clone()
        } else {
            format!("{summary}\n  note: {note}")
        }),
        serde_json::json!({
            "input": input,
            "output": output,
            "find": find,
            "replace": replace,
            "occurrences": applied,
            "matches_found": fixes.len(),
            "pages_changed": written_pages,
            "skipped_rotated_pages": rotated_pages,
            "embedded_font": needs_embed,
            "cover_color": cover_color,
            "note": note,
            "original_text_retained": true,
        }),
    ))
}
