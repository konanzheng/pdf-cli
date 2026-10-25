use crate::error::PdfError;
use crate::font;
use crate::io;
use crate::output::CommandResult;
use lopdf::{dictionary, Dictionary, Object, Stream};
use std::path::Path;

fn position_offset(pos: &str, w: f32, h: f32) -> (f32, f32) {
    let margin = 30.0;
    match pos {
        "bottom-right" => (w - margin - 120.0, margin),
        "bottom-left" => (margin, margin),
        "top-center" => (w / 2.0 - 60.0, h - margin - 10.0),
        "top-right" => (w - margin - 120.0, h - margin - 10.0),
        "top-left" => (margin, h - margin - 10.0),
        _ => (w / 2.0 - 60.0, margin), // bottom-center (default)
    }
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

/// True when every byte fits in ASCII, meaning the base-14 Helvetica font can
/// render the text without any embedding. Pure-ASCII templates stay small.
fn is_ascii(text: &str) -> bool {
    text.is_ascii()
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    input: &str,
    output: &str,
    position: &str,
    format: &str,
    start: u32,
    font_size: f32,
    font_path: Option<&str>,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    if !io::is_stdio(input) && !Path::new(input).exists() {
        return Err(PdfError::FileNotFound {
            path: input.to_string(),
        });
    }

    let mut doc = io::load_doc(input, None)?;

    let pages = doc.get_pages();
    let total = pages.len();

    if dry_run {
        return Ok(CommandResult::ok(
            "pagenum",
            Some(format!(
                "[dry-run] Would add page numbers to {} pages (format: {})",
                total, format
            )),
            serde_json::json!({
                "dry_run": true,
                "input": input,
                "output": output,
                "pages": total,
                "position": position,
                "format": format,
                "start": start,
                "embedded_font": !is_ascii(format),
            }),
        ));
    }

    // The subset must cover *every* page-number string, not just one of them:
    // `第 1 页` and `第 12 页` use different digit glyphs, and a subset built from
    // a single sample would silently refuse to render the other pages.
    let last_n = start + total as u32 - 1;
    let all_text: String = (start..=last_n)
        .map(|n| {
            format
                .replace("{n}", &n.to_string())
                .replace("{total}", &last_n.to_string())
        })
        .collect();

    // ASCII-only text uses the zero-cost base-14 font. Anything else is embedded
    // as a subsetted TrueType font, which is what fixes CJK mojibake.
    let embedded = if all_text.is_ascii() && font_path.is_none() {
        None
    } else {
        Some(font::embed(
            &mut doc,
            &all_text,
            font_path,
            "PDFCLIEmbedded",
        )?)
    };

    let font_id = match &embedded {
        None => Some(doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        })),
        Some(f) => Some(f.font_id),
    };
    let font_id = font_id.expect("font id always set");

    let mut numbered = 0u32;
    let mut skipped: Vec<u32> = Vec::new();
    let mut page_nums: Vec<u32> = pages.keys().copied().collect();
    page_nums.sort();

    for (i, page_num) in page_nums.iter().enumerate() {
        let page_id = match pages.get(page_num) {
            Some(id) => *id,
            None => continue,
        };

        let page_dict = match doc.get_object(page_id).and_then(|o| o.as_dict()) {
            Ok(d) => d.clone(),
            Err(_) => continue,
        };
        let (w, h) = media_box(&page_dict);

        let display_n = start + i as u32;
        let text = format
            .replace("{n}", &display_n.to_string())
            .replace("{total}", &(start + total as u32 - 1).to_string());

        let (x, y) = position_offset(position, w, h);

        // Build the text-showing operator appropriate to the chosen encoding.
        let show = match &embedded {
            Some(f) => {
                if !f.covers(&text) {
                    // The font lacks a glyph: report it rather than emit blanks.
                    skipped.push(*page_num);
                    continue;
                }
                format!("<{}> Tj", f.encode(&text))
            }
            None => {
                let esc = text
                    .replace('\\', "\\\\")
                    .replace('(', "\\(")
                    .replace(')', "\\)");
                format!("({esc}) Tj")
            }
        };

        // Choose a resource name that cannot collide with the page's own fonts.
        // The page very often already uses /F1 for its body text; reusing that
        // entry would make the original text render with our subsetted font --
        // i.e. as mojibake. Read the existing names first, build the content
        // stream, and only then take a mutable borrow of the page.
        let existing_font_names: Vec<Vec<u8>> = doc
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
        let res_name = loop {
            let candidate = format!("PDFCLINum{n}");
            if !existing_font_names
                .iter()
                .any(|k| k.as_slice() == candidate.as_bytes())
            {
                break candidate;
            }
            n += 1;
        };

        let content = format!(
            "q\nBT\n/{res_name} {:.1} Tf\n0 g\n{:.1} {:.1} Td\n{}\nET\nQ\n",
            font_size, x, y, show
        );
        let content_id = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));

        if let Ok(d) = doc.get_object_mut(page_id).and_then(|o| o.as_dict_mut()) {
            // Merge the page's existing resources rather than replacing them.
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
            font_dict.set(res_name.as_str(), Object::Reference(font_id));
            res.set("Font", Object::Dictionary(font_dict));
            d.set("Resources", Object::Dictionary(res));

            // Append the new content stream after the existing ones.
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
        numbered += 1;
    }

    if !skipped.is_empty() {
        return Err(PdfError::Operation {
            reason: format!(
                "The selected font cannot render the page-number text on page(s) {:?}. \
                 Pass --font <path to a .ttf/.ttc that contains these characters>.",
                skipped
            ),
        });
    }

    doc.save(output).map_err(|e| PdfError::Operation {
        reason: format!("Failed to write {output}: {e}"),
    })?;

    Ok(CommandResult::ok(
        "pagenum",
        Some(format!(
            "Added page numbers to {} pages → {}",
            numbered, output
        )),
        serde_json::json!({
            "output": output,
            "pages_numbered": numbered,
            "position": position,
            "format": format,
            "start": start,
            "font_embedded": embedded.is_some(),
            "glyphs_subset": embedded.as_ref().map(|f| f.cids.len()),
        }),
    ))
}
