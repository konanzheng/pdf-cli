use crate::error::PdfError;
use crate::font as fontmod;
use crate::output::CommandResult;
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use std::path::Path;

/// Base name of the font resource written into each text page's `/Resources`.
///
/// It is deliberately *not* `/F1`: `pagenum.rs` was bitten by reusing a name the
/// page already owned. A fresh `convert` document has no such clash today, but
/// keeping the same collision-free scheme means `convert` stays correct if it
/// ever gains page importing.
const FONT_RES_BASE: &str = "PDFCLIConv";

/// BaseFont name recorded in the embedded font's dictionaries.
const EMBEDDED_FONT_NAME: &str = "PDFCLIConvEmbedded";

/// The base-14 fallback and the subsetted TrueType font share one drawing path,
/// so each page just needs the right resource reference and string encoding.
enum PageFont<'a> {
    /// Built-in Helvetica; content strings are literal `(text) Tj`.
    Base14(ObjectId),
    /// Subsetted TrueType; content strings are Identity-H hex `<...> Tj`.
    Embedded(&'a fontmod::EmbeddedFont),
}

impl PageFont<'_> {
    fn font_id(&self) -> ObjectId {
        match self {
            PageFont::Base14(id) => *id,
            PageFont::Embedded(f) => f.font_id,
        }
    }

    /// Render one line as a PDF text-showing operator.
    fn show(&self, text: &str) -> String {
        match self {
            PageFont::Base14(_) => {
                let esc = text
                    .replace('\\', "\\\\")
                    .replace('(', "\\(")
                    .replace(')', "\\)");
                format!("({}) Tj", esc)
            }
            PageFont::Embedded(f) => format!("<{}> Tj", f.encode(text)),
        }
    }
}

fn page_size_pt(name: &str) -> (f32, f32) {
    match name.to_lowercase().as_str() {
        "a3" => (841.89, 1190.55),
        "a5" => (419.53, 595.28),
        "letter" => (612.0, 792.0),
        _ => (595.28, 841.89), // A4
    }
}

fn media_box(pw: f32, ph: f32) -> Object {
    Object::Array(vec![
        Object::Integer(0),
        Object::Integer(0),
        Object::Real(pw),
        Object::Real(ph),
    ])
}

/// Pick a `/Resources /Font` name that no page in this document uses yet.
fn unique_font_name(existing: &[Vec<u8>]) -> String {
    let mut n = 1u32;
    loop {
        let candidate = format!("{FONT_RES_BASE}{n}");
        if !existing.iter().any(|k| k.as_slice() == candidate.as_bytes()) {
            return candidate;
        }
        n += 1;
    }
}

/// Read just the first three bytes of `path` and report whether they are a UTF-8 BOM.
///
/// Used by `--dry-run`, which must not read whole files; unreadable files simply
/// report `false` here — the real run surfaces the error.
fn starts_with_bom(path: &str) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 3];
    matches!(f.read_exact(&mut head), Ok(())) && head == [0xEF, 0xBB, 0xBF]
}

/// Render up to `limit` characters for an error message.
///
/// Zero-width and control characters are invisible, so they are shown as `U+XXXX`;
/// without that, a message about a stray BOM or control byte would read as
/// `cannot render: ` with nothing after the colon.
fn format_chars(chars: &[char], limit: usize) -> String {
    let mut out = String::new();
    for ch in chars.iter().take(limit) {
        if ch.is_control() || *ch == '\u{FEFF}' {
            out.push_str(&format!("U+{:04X}", *ch as u32));
        } else {
            out.push(*ch);
        }
    }
    out
}

/// The first few non-ASCII characters of `text`, for the "no font supplied" error.
///
/// A *leading* BOM is an encoding marker, not content, and is not reported — callers
/// strip it first anyway. A U+FEFF anywhere else is genuinely suspicious, so it is
/// shown as the escape `U+FEFF` rather than an invisible character that would make
/// the message read as `(e.g. "" in file.txt)`.
fn non_ascii_sample(text: &str) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    for (i, c) in text.chars().enumerate() {
        if c.is_ascii() || (i == 0 && c == '\u{FEFF}') {
            continue;
        }
        if count == 12 {
            break;
        }
        // Zero-width / formatting characters would be invisible in the message.
        if c == '\u{FEFF}' || c.is_control() {
            out.push_str(&format!("U+{:04X}", c as u32));
        } else {
            out.push(c);
        }
        count += 1;
    }
    out
}

/// Normalise text read from disk before it is measured, subsetted or drawn.
///
/// Windows tooling (notably PowerShell 5.1's `Set-Content -Encoding UTF8`, and
/// Notepad's "UTF-8 with BOM") prepends a UTF-8 BOM. `str::is_ascii` is false for
/// U+FEFF, so without this a perfectly ordinary ASCII file would be reported as
/// unrenderable non-ASCII text. A BOM is a zero-width encoding marker, never
/// content, so drop exactly one leading occurrence.
///
/// Tabs are expanded here too, *before* the font subset is computed: the subset is
/// derived from this same text, so expanding later would ask the subset for a
/// space glyph it never collected and `covers` would reject the line. The base-14
/// path benefits as well, since a raw tab inside a PDF literal string is not a
/// reliable text advance.
///
/// Returns the normalised text and whether a BOM was removed.
fn normalize_text(text: &str) -> (String, bool) {
    let (text, stripped) = match text.strip_prefix('\u{FEFF}') {
        Some(rest) => (rest, true),
        None => (text, false),
    };
    // `lines()` handles CRLF already; replacing tabs is what matters here.
    (text.replace('\t', "    "), stripped)
}

/// Reject a `--font` that cannot render every character of `text`.
///
/// `font::embed` reports only "missing glyphs" for the whole file; an agent needs
/// to know *which* characters are unrenderable, so validate with the same parser
/// the embedding module uses before paying for subsetting.
fn validate_font_coverage(path: &str, text: &str) -> Result<(), PdfError> {
    let data = std::fs::read(path).map_err(|e| PdfError::Operation {
        reason: format!("Cannot read font {path}: {e}"),
    })?;

    let face = ttf_parser::Face::parse(&data, 0).map_err(|e| PdfError::Operation {
        reason: format!(
            "Cannot use font {path}: it is not a readable TrueType/OpenType font ({e}).\n  \
             → Pass a .ttf/.ttc file, e.g. --font C:\\Windows\\Fonts\\simhei.ttf"
        ),
    })?;

    let mut missing: Vec<char> = Vec::new();
    for ch in text.chars() {
        // Whitespace becomes positioning (or a blank glyph) and never blocks output.
        if ch.is_whitespace() {
            continue;
        }
        if face.glyph_index(ch).is_none() && !missing.contains(&ch) {
            missing.push(ch);
        }
    }

    if !missing.is_empty() {
        let shown = format_chars(&missing, 20);
        let more = if missing.len() > 20 {
            format!(" …and {} more", missing.len() - 20)
        } else {
            String::new()
        };
        return Err(PdfError::Operation {
            reason: format!(
                "Font {path} cannot render {} character(s) used by the input: {shown}{more}\n  \
                 → Use a font whose character map contains these glyphs, or remove them from \
                 the input",
                missing.len()
            ),
        });
    }

    Ok(())
}

fn make_image_page(doc: &mut Document, path: &str, pw: f32, ph: f32) -> Result<ObjectId, PdfError> {
    let img = image::open(path).map_err(|e| PdfError::Operation {
        reason: format!("Cannot open image {}: {}", path, e),
    })?;
    let rgb = img.to_rgb8();
    let (iw, ih) = rgb.dimensions();
    let raw = rgb.into_raw();

    let mut img_dict = Dictionary::new();
    img_dict.set("Type", Object::Name(b"XObject".to_vec()));
    img_dict.set("Subtype", Object::Name(b"Image".to_vec()));
    img_dict.set("Width", Object::Integer(iw as i64));
    img_dict.set("Height", Object::Integer(ih as i64));
    img_dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
    img_dict.set("BitsPerComponent", Object::Integer(8));

    let mut img_stream = Stream::new(img_dict, raw);
    let _ = img_stream.compress();
    let img_id = doc.add_object(img_stream);

    // 缩放到页面内（留 8% 边距），居中
    let scale = (pw / iw as f32).min(ph / ih as f32) * 0.92;
    let dw = iw as f32 * scale;
    let dh = ih as f32 * scale;
    let x = (pw - dw) / 2.0;
    let y = (ph - dh) / 2.0;

    let content = format!(
        "q\n{:.2} 0 0 {:.2} {:.2} {:.2} cm\n/Im0 Do\nQ\n",
        dw, dh, x, y
    );
    let content_id = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));

    let mut xobj = Dictionary::new();
    xobj.set("Im0", Object::Reference(img_id));
    let resources = dictionary! {
        "XObject" => Object::Dictionary(xobj),
        "ProcSet" => Object::Array(vec![
            Object::Name(b"PDF".to_vec()),
            Object::Name(b"ImageC".to_vec()),
        ]),
    };

    let page = dictionary! {
        "Type" => "Page",
        "MediaBox" => media_box(pw, ph),
        "Resources" => Object::Dictionary(resources),
        "Contents" => Object::Reference(content_id),
    };
    Ok(doc.add_object(page))
}

fn make_text_pages(
    doc: &mut Document,
    text: &str,
    pw: f32,
    ph: f32,
    font_size: f32,
    page_font: &PageFont<'_>,
) -> Result<Vec<ObjectId>, PdfError> {
    let line_height = font_size * 1.5;
    let margin = 50.0;
    let lines_per_page = (((ph - 2.0 * margin) / line_height).floor() as usize).max(1);

    // Tabs are expanded here, before the font subset is built: the subset is
    // derived from the same text, so expanding later would ask the subset for
    // space glyphs it never collected.
    let lines: Vec<&str> = text.lines().collect();
    let chunks: Vec<&[&str]> = if lines.is_empty() {
        vec![&[][..]]
    } else {
        lines.chunks(lines_per_page).collect()
    };

    let mut page_ids = Vec::new();
    for chunk in chunks {
        // A fresh page owns no fonts yet; the helper still does the naming so the
        // collision rules match `pagenum.rs` if resources are ever merged in.
        let res_name = unique_font_name(&[]);

        let mut content = String::from("BT\n");
        content.push_str(&format!("/{res_name} {:.1} Tf\n", font_size));
        let start_y = ph - margin;
        content.push_str(&format!("{:.1} {:.1} Td\n", margin, start_y));
        for (i, line) in chunk.iter().enumerate() {
            if i > 0 {
                content.push_str(&format!("0 -{:.1} Td\n", line_height));
            }
            // Defensive: `validate_font_coverage` already rejected unrenderable
            // characters, but never emit silently-dropped text if that changes.
            if let PageFont::Embedded(f) = page_font {
                if !f.covers(line) {
                    let missing: Vec<char> = line
                        .chars()
                        .filter(|c| !f.cids.contains_key(c))
                        .take(20)
                        .collect();
                    return Err(PdfError::Operation {
                        reason: format!(
                            "The embedded font cannot render these character(s): {}\n  \
                             → Choose a font whose character map contains them",
                            format_chars(&missing, 20)
                        ),
                    });
                }
            }
            content.push_str(&page_font.show(line));
            content.push('\n');
        }
        content.push_str("ET\n");
        let content_id = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));

        let mut font_dict = Dictionary::new();
        font_dict.set(res_name.as_str(), Object::Reference(page_font.font_id()));
        let resources = dictionary! {
            "Font" => Object::Dictionary(font_dict),
            "ProcSet" => Object::Array(vec![
                Object::Name(b"PDF".to_vec()),
                Object::Name(b"Text".to_vec()),
            ]),
        };
        let page = dictionary! {
            "Type" => "Page",
            "MediaBox" => media_box(pw, ph),
            "Resources" => Object::Dictionary(resources),
            "Contents" => Object::Reference(content_id),
        };
        page_ids.push(doc.add_object(page));
    }
    Ok(page_ids)
}

fn file_ext(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

fn is_text_ext(ext: &str) -> bool {
    matches!(ext, "md" | "txt")
}

fn is_image_ext(ext: &str) -> bool {
    matches!(
        ext,
        "jpg" | "jpeg" | "png" | "bmp" | "webp" | "tiff" | "gif"
    )
}

pub fn run(
    input: &str,
    output: &str,
    page_size: &str,
    font_size: f32,
    font: Option<&str>,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    let files: Vec<&str> = input.split(',').map(|s| s.trim()).collect();
    for f in &files {
        if !Path::new(f).exists() {
            return Err(PdfError::FileNotFound {
                path: f.to_string(),
            });
        }
    }

    // Check the font path before doing any work, so a typo in --font is reported
    // the same way as a typo in --input.
    if let Some(p) = font {
        if !Path::new(p).exists() {
            return Err(PdfError::FileNotFound {
                path: p.to_string(),
            });
        }
    }

    if dry_run {
        let mut details = serde_json::json!({
            "dry_run": true,
            "input_count": files.len(),
            "output": output,
            "font_embedded": font.is_some(),
            // Dry-run returns before the full read, so peek at the leading bytes
            // rather than reporting a field that a real run would not produce.
            "bom_stripped": files
                .iter()
                .any(|f| is_text_ext(&file_ext(f)) && starts_with_bom(f)),
        });
        if let Some(p) = font {
            details["font_path"] = serde_json::json!(p);
        }
        return Ok(CommandResult::ok(
            "convert",
            Some(format!(
                "[dry-run] Would convert {} file(s) → {}",
                files.len(),
                output
            )),
            details,
        ));
    }

    // Read every text input up front: one subset has to cover the characters of
    // *all* files, since a single font object is shared by every text page.
    let mut texts: Vec<String> = Vec::new();
    let mut bom_stripped = false;
    for file in files.iter() {
        let ext = file_ext(file);
        if is_text_ext(&ext) {
            let text = std::fs::read_to_string(file).map_err(|e| PdfError::Operation {
                reason: format!("Cannot read text file {file}: {e}"),
            })?;

            let (text, stripped) = normalize_text(&text);
            bom_stripped |= stripped;

            // Without an embedded font the base-14 Helvetica is used, and its
            // WinAnsi encoding cannot represent non-ASCII. Producing a PDF of
            // question marks here would be silent corruption, so fail loudly.
            if !text.is_ascii() && font.is_none() {
                return Err(PdfError::Operation {
                    reason: format!(
                        "Input contains non-ASCII text but no TrueType font was supplied, so \
                         the characters cannot be rendered (e.g. \"{}\" in {}).\n  \
                         → Try: pdf-cli convert --input {} --output {} --font \
                         C:\\Windows\\Fonts\\simhei.ttf",
                        non_ascii_sample(&text),
                        file,
                        input,
                        output
                    ),
                });
            }

            texts.push(text);
        } else if !is_image_ext(&ext) {
            return Err(PdfError::UnsupportedFormat { format: ext });
        }
    }

    let combined: String = texts.join("\n");

    if let Some(p) = font {
        if !combined.is_empty() {
            validate_font_coverage(p, &combined)?;
        }
    }

    let (pw, ph) = page_size_pt(page_size);
    let mut doc = Document::with_version("1.5");

    // Base-14 Helvetica is only allocated when it will actually be used.
    let base14_id = if font.is_none() && !texts.is_empty() {
        Some(doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        }))
    } else {
        None
    };

    // A requested font is always embedded, even for pure-ASCII text.
    let embedded: Option<fontmod::EmbeddedFont> = match font {
        Some(p) if !combined.is_empty() => Some(fontmod::embed(
            &mut doc,
            &combined,
            Some(p),
            EMBEDDED_FONT_NAME,
        )?),
        _ => None,
    };

    // `None` when the input is images only: no text page will ask for a font.
    let page_font: Option<PageFont<'_>> = if texts.is_empty() {
        None
    } else {
        Some(match &embedded {
            Some(f) => PageFont::Embedded(f),
            None => PageFont::Base14(
                base14_id.expect("base-14 font id set whenever text input exists"),
            ),
        })
    };

    let mut page_ids: Vec<ObjectId> = Vec::new();
    let mut text_cursor = 0usize;

    for file in files.iter() {
        let ext = file_ext(file);

        if is_text_ext(&ext) {
            // `texts` was filled in the same order as `files`, skipping images.
            let text = texts.get(text_cursor).map(|t| t.as_str()).unwrap_or("");
            text_cursor += 1;
            let Some(pf) = &page_font else {
                return Err(PdfError::Operation {
                    reason: "Internal error: text input without a font".into(),
                });
            };
            let ids = make_text_pages(&mut doc, text, pw, ph, font_size, pf)?;
            page_ids.extend(ids);
        } else {
            let id = make_image_page(&mut doc, file, pw, ph)?;
            page_ids.push(id);
        }
    }

    if page_ids.is_empty() {
        return Err(PdfError::Operation {
            reason: "No pages were generated".into(),
        });
    }

    let kids: Vec<Object> = page_ids.iter().map(|id| Object::Reference(*id)).collect();
    let pages_dict = dictionary! {
        "Type" => "Pages",
        "Kids" => Object::Array(kids),
        "Count" => Object::Integer(page_ids.len() as i64),
    };
    let pages_id = doc.add_object(pages_dict);

    for &pid in &page_ids {
        if let Ok(d) = doc.get_object_mut(pid).and_then(|o| o.as_dict_mut()) {
            d.set("Parent", Object::Reference(pages_id));
        }
    }

    let catalog = dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(pages_id),
    };
    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", Object::Reference(catalog_id));

    doc.save(output).map_err(|e| PdfError::Operation {
        reason: e.to_string(),
    })?;

    let mut details = serde_json::json!({
        "output": output,
        "input_count": files.len(),
        "pages_created": page_ids.len(),
        "page_size": page_size,
        "font_embedded": embedded.is_some(),
        "glyphs_subset": embedded.as_ref().map_or(0, |f| f.cids.len()),
        "bom_stripped": bom_stripped,
    });
    if let (Some(p), Some(_)) = (font, &embedded) {
        details["font_path"] = serde_json::json!(p);
    }

    Ok(CommandResult::ok(
        "convert",
        Some(format!(
            "Converted {} file(s) → {} ({} pages)",
            files.len(),
            output,
            page_ids.len()
        )),
        details,
    ))
}
