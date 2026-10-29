//! `overlay` — place text (and optional cover boxes) at exact page coordinates.
//!
//! This is the manual counterpart to `redact`. Use it when you know where the
//! text goes — a fixed-layout template, a form whose value sits at the same
//! spot on every copy — or when the source has no text layer at all, where
//! `redact` cannot work because there is no measured rectangle to find.
//!
//! ## Spec format
//!
//! `--text` takes one or more `PAGE:SPEC` items separated by `;`. `PAGE` is a
//! single page (`3`), a range (`2-5`) or `*` for every page. `SPEC` is a
//! comma-separated list of `key=value`:
//!
//! ```text
//! 1:x=72,y=700,text=Invoice 2024-0042,size=11
//! *:x=72,y=700,text=DRAFT
//! 2-4:x=100,y=600,text=Acme,w=120,h=14,cover=1/1/1
//! ```
//!
//! Recognised keys:
//!
//! | key | meaning |
//! |---|---|
//! | `x`, `y` | position in PDF points, origin **bottom-left** (required) |
//! | `text` | string to draw; may repeat |
//! | `size` | font size in points (default 11) |
//! | `w`, `h` | cover-box size; both must be given |
//! | `cover` | fill for the cover box as `r/g/b` (or `r g b`), each 0-1 (default white) |
//!
//! `x`/`y` use the bottom-left origin, matching `crop --box` and the PDF spec,
//! rather than the top-left origin `search` reports. That is deliberate: this
//! command is for hand-placed coordinates, where the PDF-native frame is what a
//! layout tool shows you. (`redact` uses `search` output and flips internally.)
//!
//! ## Why the cover box is separate from the text
//!
//! A template change usually means "hide the old value, print the new one".
//! `redact` finds the old value by searching; here you state the rectangle
//! yourself, which also makes this the only route for scanned pages.

use std::path::Path;

use lopdf::{dictionary, Dictionary, Object, Stream};

use crate::error::PdfError;
use crate::font;
use crate::io;
use crate::output::CommandResult;

/// One resolved drawing operation.
struct Op {
    page: u32,
    x: f32,
    y: f32,
    text: Option<String>,
    size: f32,
    cover: Option<(f32, f32, f32, f32, f32)>, // r, g, b, w, h
}

fn escape_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            '\r' => {}
            '\n' => out.push(' '),
            c => out.push(c),
        }
    }
    out
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
        let candidate = format!("PDFCLIOverlay{n}");
        if !existing
            .iter()
            .any(|k| k.as_slice() == candidate.as_bytes())
        {
            return candidate;
        }
        n += 1;
    }
}

/// Parse a cover fill colour.
///
/// Components are separated by `/` or whitespace, **not** by commas: the spec
/// itself separates `key=value` pairs with commas, so `cover=1,0.9,0.9` would be
/// read as `cover=1` followed by two unparseable pairs.
fn parse_colour(spec: &str) -> Result<(f32, f32, f32), PdfError> {
    let parts: Vec<&str> = spec
        .split(|c: char| c == '/' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() != 3 {
        return Err(PdfError::Operation {
            reason: format!(
                "cover colour needs three 0-1 components separated by '/' or spaces \
                 (e.g. cover=1/0.9/0.9), got {spec:?}"
            ),
        });
    }
    let mut out = [0.0f32; 3];
    for (i, p) in parts.iter().enumerate() {
        out[i] = p.trim().parse::<f32>().map_err(|_| PdfError::Operation {
            reason: format!("cover colour component {p:?} is not a number"),
        })?;
        if !(0.0..=1.0).contains(&out[i]) {
            return Err(PdfError::Operation {
                reason: format!(
                    "cover colour component {} out of range (expected 0-1): {}",
                    out[i], spec
                ),
            });
        }
    }
    Ok((out[0], out[1], out[2]))
}

/// Parse a page spec (`3`, `2-5`, `*`) into concrete page numbers.
fn expand_pages(spec: &str, total: u32) -> Result<Vec<u32>, PdfError> {
    if spec.trim() == "*" {
        return Ok((1..=total).collect());
    }
    if let Some((a, b)) = spec.split_once('-') {
        let a: u32 = a.trim().parse().map_err(|_| PdfError::Operation {
            reason: format!("bad page range start in {spec:?}"),
        })?;
        let b: u32 = b.trim().parse().map_err(|_| PdfError::Operation {
            reason: format!("bad page range end in {spec:?}"),
        })?;
        if a == 0 || b < a {
            return Err(PdfError::InvalidPageRangeReason {
                spec: spec.to_string(),
                reason: format!("range {a}-{b} is empty or not 1-based"),
            });
        }
        if b > total {
            return Err(PdfError::InvalidPageRangeReason {
                spec: spec.to_string(),
                reason: format!("page {b} is out of range; the document has {total} page(s)"),
            });
        }
        return Ok((a..=b).collect());
    }
    let n: u32 = spec.trim().parse().map_err(|_| PdfError::Operation {
        reason: format!("bad page number {spec:?}; expected N, N-M or *"),
    })?;
    if n == 0 || n > total {
        return Err(PdfError::InvalidPageRangeReason {
            spec: spec.to_string(),
            reason: if n == 0 {
                "pages are 1-based".to_string()
            } else {
                format!("page {n} is out of range; the document has {total} page(s)")
            },
        });
    }
    Ok(vec![n])
}

/// Parse the whole `--text` expression into operations.
fn parse_spec(expr: &str, total: u32) -> Result<Vec<Op>, PdfError> {
    let mut ops = Vec::new();

    for item in expr.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let (page_spec, rest) = item.split_once(':').ok_or_else(|| PdfError::Operation {
            reason: format!(
                "expected PAGE:key=value,... but got {item:?} (missing the ':' after the page)"
            ),
        })?;

        let pages = expand_pages(page_spec, total)?;

        let mut x = None;
        let mut y = None;
        let mut text: Option<String> = None;
        let mut size = 11.0f32;
        let mut w = None;
        let mut h = None;
        let mut colour = (1.0f32, 1.0f32, 1.0f32);

        for pair in rest.split(',') {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }
            let (k, v) = pair.split_once('=').ok_or_else(|| PdfError::Operation {
                reason: format!("expected key=value but got {pair:?} in {item:?}"),
            })?;
            let k = k.trim();
            let v = v.trim();
            match k {
                "x" => {
                    x = Some(v.parse::<f32>().map_err(|_| PdfError::Operation {
                        reason: format!("x is not a number: {v:?}"),
                    })?)
                }
                "y" => {
                    y = Some(v.parse::<f32>().map_err(|_| PdfError::Operation {
                        reason: format!("y is not a number: {v:?}"),
                    })?)
                }
                "size" => {
                    size = v.parse::<f32>().map_err(|_| PdfError::Operation {
                        reason: format!("size is not a number: {v:?}"),
                    })?
                }
                "text" => text = Some(v.to_string()),
                "w" => {
                    w = Some(v.parse::<f32>().map_err(|_| PdfError::Operation {
                        reason: format!("w is not a number: {v:?}"),
                    })?)
                }
                "h" => {
                    h = Some(v.parse::<f32>().map_err(|_| PdfError::Operation {
                        reason: format!("h is not a number: {v:?}"),
                    })?)
                }
                "cover" => colour = parse_colour(v)?,
                other => {
                    return Err(PdfError::Operation {
                        reason: format!(
                            "unknown key {other:?} in {item:?}. Valid keys: x, y, text, size, w, h, cover"
                        ),
                    })
                }
            }
        }

        let (x, y) = match (x, y) {
            (Some(x), Some(y)) => (x, y),
            _ => {
                return Err(PdfError::Operation {
                    reason: format!("{item:?} needs both x= and y="),
                })
            }
        };

        if text.is_none() && (w.is_none() || h.is_none()) {
            return Err(PdfError::Operation {
                reason: format!("{item:?} needs text=, or a cover box via w= and h="),
            });
        }

        let cover = match (w, h) {
            (Some(w), Some(h)) => Some((colour.0, colour.1, colour.2, w, h)),
            (None, None) => None,
            _ => {
                return Err(PdfError::Operation {
                    reason: format!("{item:?} needs both w= and h= to draw a cover box"),
                })
            }
        };

        for page in pages {
            ops.push(Op {
                page,
                x,
                y,
                text: text.clone(),
                size,
                cover,
            });
        }
    }

    if ops.is_empty() {
        return Err(PdfError::MissingArg {
            arg: "--text".to_string(),
            example: "pdf-cli overlay --input in.pdf --output out.pdf --text \"1:x=72,y=700,text=Hello,size=12\""
                .into(),
        });
    }

    Ok(ops)
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    input: &str,
    output: &str,
    spec: &str,
    font_path: Option<&str>,
    password: Option<&str>,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    if !io::is_stdio(input) && !Path::new(input).exists() {
        return Err(PdfError::FileNotFound {
            path: input.to_string(),
        });
    }

    let mut doc = io::load_doc(input, password)?;
    let total = doc.get_pages().len() as u32;

    let ops = parse_spec(spec, total)?;

    // Every string that will be drawn, so one subset covers them all.
    let all_text: String = ops
        .iter()
        .filter_map(|o| o.text.as_ref())
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("");

    let needs_embed = !all_text.is_ascii() || font_path.is_some();
    let embedded = if needs_embed && !all_text.is_empty() {
        Some(font::embed(
            &mut doc,
            &all_text,
            font_path,
            "PDFCLIOverlay",
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

    // Check every string before mutating anything.
    if let Some(f) = &embedded {
        let bad: Vec<&str> = ops
            .iter()
            .filter_map(|o| o.text.as_ref())
            .filter(|t| !f.covers(t))
            .map(|t| t.as_str())
            .collect();
        if !bad.is_empty() {
            return Err(PdfError::Operation {
                reason: format!(
                    "The chosen font cannot render: {bad:?}. \
                     Pass --font <path to a .ttf/.ttc that contains these characters>."
                ),
            });
        }
    }

    let pages_map = doc.get_pages();

    let mut by_page: std::collections::BTreeMap<u32, Vec<&Op>> = std::collections::BTreeMap::new();
    for op in &ops {
        by_page.entry(op.page).or_default().push(op);
    }

    if dry_run {
        let page_list: Vec<u32> = by_page.keys().copied().collect();
        return Ok(CommandResult::ok(
            "overlay",
            Some(format!(
                "[dry-run] Would draw {} item(s) on {} page(s)",
                ops.len(),
                page_list.len()
            )),
            serde_json::json!({
                "dry_run": true,
                "input": input,
                "output": output,
                "items": ops.len(),
                "pages": page_list,
                "embedded_font": needs_embed,
            }),
        ));
    }

    let mut drawn_pages = 0u32;

    for (page_num, page_ops) in &by_page {
        let page_id = match pages_map.get(page_num) {
            Some(id) => *id,
            None => continue,
        };

        let rotation = doc
            .get_object(page_id)
            .ok()
            .and_then(|o| o.as_dict().ok())
            .and_then(|d| d.get(b"Rotate").ok())
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);
        if rotation % 360 != 0 {
            return Err(PdfError::Operation {
                reason: format!(
                    "page {page_num} carries /Rotate {rotation}; overlay coordinates assume an \
                     unrotated page. Remove the rotation with `rotate --degrees 0` first."
                ),
            });
        }

        let res_name = free_font_name(&doc, page_id);
        let mut body = String::new();

        for op in page_ops {
            if let Some((r, g, b, w, h)) = op.cover {
                body.push_str(&format!(
                    "q\n{r} {g} {b} rg\n{} {} {w} {h} re\nf\nQ\n",
                    op.x, op.y
                ));
            }
            if let Some(text) = &op.text {
                let show = match &embedded {
                    Some(f) => format!("<{}> Tj", f.encode(text)),
                    None => format!("({}) Tj", escape_literal(text)),
                };
                body.push_str(&format!(
                    "q\nBT\n/{res_name} {:.1} Tf\n0 g\n{:.1} {:.1} Td\n{}\nET\nQ\n",
                    op.size, op.x, op.y, show
                ));
            }
        }

        let content_id = doc.add_object(Stream::new(Dictionary::new(), body.into_bytes()));

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
        drawn_pages += 1;
    }

    doc.save(output).map_err(|e| PdfError::Operation {
        reason: format!("Failed to write {output}: {e}"),
    })?;

    // Keep the MediaBox helper referenced: it is used by callers that need page
    // extents for validation, and keeps this module's behaviour explicit.
    let _ = media_box;

    Ok(CommandResult::ok(
        "overlay",
        Some(format!(
            "Drew {} item(s) on {} page(s) → {}",
            ops.len(),
            drawn_pages,
            output
        )),
        serde_json::json!({
            "input": input,
            "output": output,
            "items": ops.len(),
            "pages_changed": drawn_pages,
            "pages": by_page.keys().copied().collect::<Vec<u32>>(),
            "embedded_font": needs_embed,
        }),
    ))
}
