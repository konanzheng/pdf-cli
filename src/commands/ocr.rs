//! OCR a scanned PDF through an external `ocr-rust` HTTP service.
//!
//! Pipeline: pdfium renders each selected page to PNG, the PNG is POSTed as the
//! raw request body to `POST {service}/ocr/raw`, and the JSON reply (`text`,
//! `regions`) is collected. The page image never leaves the local service.
//!
//! # `--lang` — language hint
//!
//! The hint is forwarded as the **query parameter** `lang` on `/ocr/raw`:
//!
//! ```text
//! POST http://localhost:18004/ocr/raw?lang=ch
//! Content-Type: image/png
//! <raw PNG bytes>
//! ```
//!
//! * The body is a raw image, so the query string is the only channel a hint can
//!   travel on — it is the same channel the service already uses for
//!   `model=small|tiny`.
//! * `ureq` URL-encodes the value, so `eng+ch` goes out as `eng%2Bch` and decodes
//!   back to `eng+ch` on the server side.
//! * With no `--lang` the parameter is not appended at all: the request stays
//!   byte-for-byte what it was before the flag existed.
//!
//! Values the service knows about are PaddleOCR language codes:
//!
//! | value | script |
//! |-----------------|---------------------------------------------|
//! | `ch` | Chinese, plus Latin (the default model) |
//! | `eng` | English |
//! | `eng+ch` | English and Chinese (same as `ch` in practice) |
//! | `japan`, `korean`, `french`, `german`, `ru`, ... | the other PaddleOCR models |
//!
//! The current `ocr-rust` build only models `OcrQuery { model }`; its actix
//! `web::Query` extractor ignores parameters it does not know, so a hint sent to
//! it is accepted and simply unused. A service that does read `lang` picks it up
//! unchanged. Either way the value we actually sent is echoed in `details.lang`
//! (`null` when the flag was omitted), so a caller can always tell whether a hint
//! was in play.
//!
//! # `--output-pdf` — searchable PDF
//!
//! `--output-pdf out.pdf` writes a new PDF with one page per OCR'd page: the
//! rendered page image, plus an **invisible** text layer (`3 Tr`, render mode 3)
//! carrying the recognised text. That is what makes `pdf-cli extract` and
//! `pdf-cli search` work on the result — the raster alone is unreadable text.
//!
//! Font selection mirrors `pagenum.rs`:
//!
//! * pure-ASCII text uses the base-14 `Helvetica` font with `WinAnsiEncoding`
//!   (nothing embedded, so the file stays image-sized);
//! * anything else is embedded via [`crate::font::embed`], which subsets a system
//!   CJK font and — crucially — writes the `/ToUnicode` CMap. Without that CMap an
//!   Identity-H font extracts as nothing at all.
//!
//! The subset is built from the recognised text of *every* page, so one font
//! object serves the whole document. Text is placed on the region boxes the
//! service returns when there is exactly one box per text line; otherwise it is
//! laid out top-down with fixed leading. A page the service found no text on
//! still gets its image, so the page count always matches the input.

use crate::error::PdfError;
use crate::font;
use crate::output::CommandResult;
use lopdf::{dictionary, Dictionary, Document, Object, Stream};
use std::path::Path;

/// Font resource name used inside the generated PDF.
const FONT_RES_NAME: &str = "PDFLIOCRF1";

/// A region the service recognised: its text plus its box in image pixels
/// (`[x0, y0, x1, y1]`, origin top-left), or `None` when no usable polygon came
/// back.
type Region = (String, Option<[f32; 4]>);

/// One page worth of OCR input and output.
struct OcrPage {
    /// 1-based page number in the source document.
    page_number: u32,
    /// Text the service recognised, lines separated by `\n`.
    text: String,
    /// Recognised regions, in reading order.
    regions: Vec<Region>,
    /// The rendered page and the geometry needed to place text on it.
    render: PageRender,
}

/// A rendered page plus the geometry needed to map image pixels to PDF points.
struct PageRender {
    png: Vec<u8>,
    image_width: u32,
    image_height: u32,
    page_width: f32,
    page_height: f32,
}

/// What the searchable-PDF writer did, for `details`.
struct PdfReport {
    font_embedded: bool,
    glyphs_subset: usize,
    pages_with_text: usize,
    lines_dropped: usize,
}

/// Render one page of `input` to PNG.
fn render_page(
    pdfium: &pdfium_render::prelude::Pdfium,
    input: &str,
    page_index: u16,
    width: u32,
) -> Result<PageRender, PdfError> {
    use pdfium_render::prelude::*;
    let doc = pdfium
        .load_pdf_from_file(input, None)
        .map_err(|e| PdfError::ParseFailed {
            reason: e.to_string(),
        })?;
    let page = doc
        .pages()
        .get(page_index)
        .map_err(|e| PdfError::Operation {
            reason: format!("page {}: {}", page_index + 1, e),
        })?;
    let mut page_width = page.width().value;
    let mut page_height = page.height().value;
    let config = PdfRenderConfig::new().set_target_width(width as i32);
    let bitmap = page
        .render_with_config(&config)
        .map_err(|e| PdfError::Operation {
            reason: format!("render page {}: {}", page_index + 1, e),
        })?;
    let img = bitmap.as_image();
    let image_width = img.width();
    let image_height = img.height();
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| PdfError::Operation {
            reason: e.to_string(),
        })?;

    // pdfium applies the page's /Rotate while rendering, so the bitmap can be
    // landscape while the MediaBox is portrait. Match the box to the bitmap so
    // the image is not stretched when it is drawn back.
    if (image_width > image_height) != (page_width > page_height) {
        std::mem::swap(&mut page_width, &mut page_height);
    }

    Ok(PageRender {
        png: buf.into_inner(),
        image_width,
        image_height,
        page_width,
        page_height,
    })
}

/// Pull a bounding box out of a region's `bounding_box` polygon.
///
/// The service emits `[[x, y], ...]` in rendered-image pixels with the origin at
/// the top-left. Fewer than four points is not a usable box.
fn bbox_from_json(value: &serde_json::Value) -> Option<[f32; 4]> {
    let points = value.as_array()?;
    let mut x0 = f32::INFINITY;
    let mut y0 = f32::INFINITY;
    let mut x1 = f32::NEG_INFINITY;
    let mut y1 = f32::NEG_INFINITY;
    let mut seen = 0usize;
    for point in points {
        let Some(pair) = point.as_array() else {
            continue;
        };
        if pair.len() < 2 {
            continue;
        }
        let (Some(x), Some(y)) = (pair[0].as_f64(), pair[1].as_f64()) else {
            continue;
        };
        let (x, y) = (x as f32, y as f32);
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
        seen += 1;
    }
    if seen >= 4 && x1 > x0 && y1 > y0 {
        Some([x0, y0, x1, y1])
    } else {
        None
    }
}

/// POST one rendered page to the service and parse the reply.
fn ocr_bytes(
    service: &str,
    png: &[u8],
    lang: Option<&str>,
) -> Result<(String, Vec<Region>), PdfError> {
    let url = format!("{}/ocr/raw", service.trim_end_matches('/'));
    let mut request = ureq::post(&url).set("Content-Type", "image/png");
    if let Some(hint) = lang.map(str::trim).filter(|l| !l.is_empty()) {
        // `--lang` rides in the query string, the same place `model=` lives: the
        // body is the raw image, so a body field could never be read.
        request = request.query("lang", hint);
    }
    let resp = request.send_bytes(png);

    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            return Err(PdfError::Operation {
                reason: format!("OCR service returned HTTP {}: {}", code, body),
            });
        }
        Err(e) => {
            return Err(PdfError::Operation {
                reason: format!(
                    "Cannot reach OCR service at {}. Start the service first: \
                     `cd F:\\rust\\ocr-rust; .\\target\\release\\ocr-rust.exe` (listens on port 18004, \
                     see ROADMAP.md). Error: {}",
                    service, e
                ),
            });
        }
    };

    let json: serde_json::Value = resp.into_json().map_err(|e| PdfError::Operation {
        reason: format!("Invalid OCR response: {}", e),
    })?;

    // A service that reports failure inside a 200 response must not look like a
    // successful page with empty text.
    if json.get("success").and_then(|v| v.as_bool()) == Some(false) {
        let reason = json
            .get("error")
            .or_else(|| json.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(PdfError::Operation {
            reason: format!("OCR service reported failure: {}", reason),
        });
    }

    let text = json
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let regions = json
        .get("regions")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let text = item
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let bbox = item.get("bounding_box").and_then(bbox_from_json);
                    (text, bbox)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok((text, regions))
}

/// Escape a string for a `(...) Tj` operand in a base-14 font.
fn escape_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            _ => out.push(ch),
        }
    }
    out
}

/// The lines of a page that actually get drawn, in order.
fn page_lines(page: &OcrPage) -> Vec<&str> {
    page.text
        .split('\n')
        .map(|line| line.trim_end())
        .filter(|line| !line.is_empty())
        .collect()
}

/// Invisible text drawing ops for one page.
///
/// Every line gets its own `BT ... ET` block with an absolute `Tm`, so nothing
/// leaks between lines and the numbers stay easy to audit. Returns the ops plus
/// the number of lines that had to be dropped because the subsetted font cannot
/// render them.
fn page_text_ops(
    page: &OcrPage,
    embedded: Option<&font::EmbeddedFont>,
    base14: bool,
) -> (String, usize) {
    let lines = page_lines(page);
    if lines.is_empty() {
        return (String::new(), 0);
    }

    let pw = page.render.page_width;
    let ph = page.render.page_height;
    let sx = pw / (page.render.image_width.max(1) as f32);
    let sy = ph / (page.render.image_height.max(1) as f32);

    // Use the service's boxes only when they line up one-to-one with our lines;
    // a partial set would silently misplace text.
    let boxes: Option<Vec<[f32; 4]>> = {
        let candidate: Vec<Option<[f32; 4]>> = page.regions.iter().map(|(_, b)| *b).collect();
        if candidate.len() == lines.len() && candidate.iter().all(|b| b.is_some()) {
            Some(candidate.into_iter().flatten().collect())
        } else {
            None
        }
    };

    let mut ops = String::new();
    let mut dropped = 0usize;
    for (index, line) in lines.iter().enumerate() {
        let (font_size, x, baseline) = match &boxes {
            Some(boxes) => {
                // `bx1` is unused on purpose: the width only decides the fit
                // quality, and glyph size is driven by box height.
                let [bx0, by0, _bx1, by1] = boxes[index];
                let box_h = (by1 - by0) * sy;
                // Fit the glyphs inside the box, clamped so an absurd box cannot
                // produce an unusable size.
                let size = (box_h * 0.85).clamp(4.0, 96.0);
                let x = (bx0 * sx).clamp(0.0, pw);
                // PDF y grows upward, the service's y grows downward.
                let baseline = (ph - by1 * sy).clamp(0.0, ph);
                (size, x, baseline)
            }
            None => {
                // Fixed leading, top-down, like a plain text page.
                let size = 12.0f32;
                let baseline = ph - 36.0 - (index as f32) * 16.0;
                (size, 36.0f32, baseline)
            }
        };
        if baseline < 0.0 {
            continue;
        }

        let show = if base14 {
            format!("({}) Tj", escape_literal(line))
        } else {
            let Some(f) = embedded else { continue };
            if !f.covers(line) {
                // Rather than emit a half-empty line, count it and report it.
                dropped += 1;
                continue;
            }
            let hex = f.encode(line);
            if hex.is_empty() {
                continue;
            }
            format!("<{}> Tj", hex)
        };

        ops.push_str(&format!(
            "BT\n/{} {:.2} Tf\n3 Tr\n0 g\n1 0 0 1 {:.2} {:.2} Tm\n{}\nET\n",
            FONT_RES_NAME, font_size, x, baseline, show
        ));
    }
    (ops, dropped)
}

/// Decode a PNG into the RGB bytes an image XObject wants.
fn png_to_rgb(png: &[u8], page: u32) -> Result<(u32, u32, Vec<u8>), PdfError> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).map_err(|e| {
        PdfError::Operation {
            reason: format!("Cannot decode rendered page {}: {}", page, e),
        }
    })?;
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    Ok((w, h, rgb.into_raw()))
}

fn media_box(width: f32, height: f32) -> Object {
    Object::Array(vec![
        Object::Integer(0),
        Object::Integer(0),
        Object::Real(width),
        Object::Real(height),
    ])
}

/// Build the searchable PDF: page image + invisible text layer, one page per
/// OCR'd page.
fn build_searchable_pdf(pages: &[OcrPage], output: &str) -> Result<PdfReport, PdfError> {
    // One subset serves every page, so it must be built from all the text at
    // once: `第一页` and `第十二页` need different digit glyphs.
    let all_text: String = pages
        .iter()
        .map(|p| p.text.as_str())
        .collect::<Vec<&str>>()
        .join("\n");

    let mut doc = Document::with_version("1.5");

    // ASCII-only documents stay tiny: base-14 Helvetica, nothing embedded.
    // Anything else is subsetted once, up front, so page building is uniform.
    let base14 = all_text.is_ascii();
    let embedded = if base14 {
        None
    } else {
        Some(font::embed(&mut doc, &all_text, None, "PDFLIOCREmbedded")?)
    };

    let font_id = if base14 {
        Some(doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        }))
    } else {
        embedded.as_ref().map(|f| f.font_id)
    };

    let mut page_ids: Vec<Object> = Vec::new();
    let mut pages_with_text = 0usize;
    let mut lines_dropped = 0usize;

    for page in pages {
        let (iw, ih, raw) = png_to_rgb(&page.render.png, page.page_number)?;
        let mut image_dict = Dictionary::new();
        image_dict.set("Type", Object::Name(b"XObject".to_vec()));
        image_dict.set("Subtype", Object::Name(b"Image".to_vec()));
        image_dict.set("Width", Object::Integer(iw as i64));
        image_dict.set("Height", Object::Integer(ih as i64));
        image_dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        image_dict.set("BitsPerComponent", Object::Integer(8));
        let mut image_stream = Stream::new(image_dict, raw);
        // A page render is mostly flat colour; Flate keeps the file reasonable.
        let _ = image_stream.compress();
        let image_id = doc.add_object(image_stream);

        let (text_ops, dropped) = page_text_ops(page, embedded.as_ref(), base14);
        lines_dropped += dropped;
        if !text_ops.is_empty() {
            pages_with_text += 1;
        }

        let pw = page.render.page_width;
        let ph = page.render.page_height;

        // Image first (opaque, full page), then the invisible text on top.
        let mut content = format!("q\n{:.2} 0 0 {:.2} 0 0 cm\n/Im0 Do\nQ\n", pw, ph);
        content.push_str(&text_ops);
        let content_id = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));

        let mut resources = Dictionary::new();
        resources.set(
            "XObject",
            Object::Dictionary(dictionary! { "Im0" => Object::Reference(image_id) }),
        );
        resources.set(
            "ProcSet",
            Object::Array(vec![
                Object::Name(b"PDF".to_vec()),
                Object::Name(b"Text".to_vec()),
                Object::Name(b"ImageC".to_vec()),
            ]),
        );
        if let Some(id) = font_id {
            let mut fonts = Dictionary::new();
            fonts.set(FONT_RES_NAME, Object::Reference(id));
            resources.set("Font", Object::Dictionary(fonts));
        }

        let page_dict = dictionary! {
            "Type" => "Page",
            "MediaBox" => media_box(pw, ph),
            "Resources" => Object::Dictionary(resources),
            "Contents" => Object::Reference(content_id),
        };
        page_ids.push(Object::Reference(doc.add_object(page_dict)));
    }

    if page_ids.is_empty() {
        return Err(PdfError::Operation {
            reason: "No pages to write".into(),
        });
    }

    let page_count = page_ids.len() as i64;
    let pages_id = doc.add_object(dictionary! {
        "Type" => "Pages",
        "Kids" => Object::Array(page_ids.clone()),
        "Count" => Object::Integer(page_count),
    });
    for object in &page_ids {
        if let Object::Reference(id) = object {
            if let Ok(dict) = doc.get_object_mut(*id).and_then(|o| o.as_dict_mut()) {
                dict.set("Parent", Object::Reference(pages_id));
            }
        }
    }

    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(pages_id),
    });
    doc.trailer.set("Root", Object::Reference(catalog_id));

    doc.save(output).map_err(|e| PdfError::Operation {
        reason: format!("Failed to write {}: {}", output, e),
    })?;

    Ok(PdfReport {
        font_embedded: embedded.is_some(),
        glyphs_subset: embedded.as_ref().map_or(0, |f| f.cids.len()),
        pages_with_text,
        lines_dropped,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    input: &str,
    output: Option<&str>,
    service: &str,
    width: u32,
    pages_spec: Option<&str>,
    pdfium_path: Option<&str>,
    lang: Option<&str>,
    output_pdf: Option<&str>,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    if !Path::new(input).exists() {
        return Err(PdfError::FileNotFound {
            path: input.to_string(),
        });
    }

    // The effective hint: what we will put on the wire (and report).
    let effective_lang = lang.map(str::trim).filter(|l| !l.is_empty());

    if dry_run {
        return Ok(CommandResult::ok(
            "ocr",
            Some(format!("[dry-run] Would OCR {} via {}", input, service)),
            serde_json::json!({
                "dry_run": true,
                "input": input,
                "service": service,
                "lang": effective_lang,
                "output_pdf": output_pdf,
            }),
        ));
    }

    // 绑定 pdfium
    let bindings = match pdfium_path {
        Some(p) => pdfium_render::prelude::Pdfium::bind_to_library(p),
        None => pdfium_render::prelude::Pdfium::bind_to_system_library(),
    }
    .map_err(|e| PdfError::Operation {
        reason: format!(
            "pdfium library not found ({}). Pass --pdfium-path <pdfium.dll>",
            e
        ),
    })?;
    let pdfium = pdfium_render::prelude::Pdfium::new(bindings);

    // 先探页数
    let total = {
        let d = pdfium
            .load_pdf_from_file(input, None)
            .map_err(|e| PdfError::ParseFailed {
                reason: e.to_string(),
            })?;
        d.pages().len() as u32
    };

    let selected: Vec<u32> = match pages_spec {
        Some(spec) => {
            let mut v = Vec::new();
            for part in spec.split(',') {
                let part = part.trim();
                if part.contains('-') {
                    let b: Vec<&str> = part.splitn(2, '-').collect();
                    let s: u32 = b[0]
                        .trim()
                        .parse()
                        .map_err(|_| PdfError::InvalidPageRange { range: spec.into() })?;
                    let e: u32 = b[1]
                        .trim()
                        .parse()
                        .map_err(|_| PdfError::InvalidPageRange { range: spec.into() })?;
                    for p in s..=e {
                        if p >= 1 && p <= total {
                            v.push(p);
                        }
                    }
                } else if let Ok(p) = part.parse::<u32>() {
                    if p >= 1 && p <= total {
                        v.push(p);
                    }
                }
            }
            v
        }
        None => (1..=total).collect(),
    };

    if selected.is_empty() {
        return Err(PdfError::InvalidPageRange {
            range: pages_spec.unwrap_or("(all)").into(),
        });
    }

    let mut ocr_pages: Vec<OcrPage> = Vec::new();
    let mut all_text = Vec::new();
    let mut page_results = Vec::new();
    let mut total_regions = 0usize;

    for &p in &selected {
        let render = render_page(&pdfium, input, (p - 1) as u16, width)?;
        let (text, regions) = ocr_bytes(service, &render.png, effective_lang)?;
        total_regions += regions.len();
        page_results.push(serde_json::json!({
            "page": p,
            "chars": text.chars().count(),
            "regions": regions.len(),
        }));
        all_text.push(text.clone());
        ocr_pages.push(OcrPage {
            page_number: p,
            text,
            regions,
            render,
        });
    }

    let joined = all_text.join("\n\n");
    let char_count = joined.chars().count();

    let out_info = if let Some(out_path) = output {
        std::fs::write(out_path, &joined).map_err(|e| PdfError::Operation {
            reason: e.to_string(),
        })?;
        Some(out_path.to_string())
    } else {
        None
    };

    // Only touch the PDF path when the user asked for one: without the flag the
    // command behaves exactly as it did before the feature existed.
    let mut pdf_path_written: Option<String> = None;
    let mut pdf_report: Option<PdfReport> = None;
    if let Some(pdf_path) = output_pdf {
        let report = build_searchable_pdf(&ocr_pages, pdf_path)?;
        if report.lines_dropped > 0 {
            return Err(PdfError::Operation {
                reason: format!(
                    "{} line(s) of recognised text could not be rendered by the embedded font \
                     (no glyphs for some characters). The text output is unaffected; \
                     rerun without --output-pdf for the text, or install a font covering them.",
                    report.lines_dropped
                ),
            });
        }
        pdf_path_written = Some(pdf_path.to_string());
        pdf_report = Some(report);
    }

    let preview = if output.is_none() && pdf_path_written.is_none() {
        Some(joined.clone())
    } else {
        None
    };

    let summary = match (&pdf_path_written, &pdf_report, output) {
        (Some(pdf_path), Some(report), _) => Some(format!(
            "OCR'd {} page(s) → {} ({} page(s) with text, {} page(s) total)",
            selected.len(),
            pdf_path,
            report.pages_with_text,
            selected.len()
        )),
        (_, _, Some(out)) => Some(format!("OCR'd {} page(s) → {}", selected.len(), out)),
        _ => None,
    };

    let mut details = serde_json::json!({
        "input": input,
        "service": service,
        "lang": effective_lang,
        "pages_processed": selected.len(),
        "total_regions": total_regions,
        "char_count": char_count,
        "output": out_info,
        "output_pdf": pdf_path_written,
        "searchable": pdf_report.is_some(),
        "pages_with_text": pdf_report.as_ref().map_or(0, |r| r.pages_with_text),
        "pages": page_results,
    });
    if let Some(report) = &pdf_report {
        details["font_embedded"] = serde_json::json!(report.font_embedded);
        details["glyphs_subset"] = serde_json::json!(report.glyphs_subset);
    }

    Ok(CommandResult::ok("ocr", preview.or(summary), details))
}
