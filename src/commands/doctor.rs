use crate::error::PdfError;
use crate::output::CommandResult;
use lopdf::{Dictionary, Document, Object};
use std::collections::BTreeMap;
use std::path::Path;

const PT_TO_MM: f64 = 25.4 / 72.0;

fn standard_size(w: f64, h: f64) -> &'static str {
    let (long, short) = if w >= h { (w, h) } else { (h, w) };
    const CANDIDATES: &[(&str, f64, f64)] = &[
        ("A3", 842.0, 1191.0),
        ("A4", 595.0, 842.0),
        ("A5", 420.0, 595.0),
        ("A6", 298.0, 420.0),
        ("Letter", 612.0, 792.0),
        ("Legal", 612.0, 1008.0),
        ("Tabloid", 792.0, 1224.0),
    ];
    for (name, cw, ch) in CANDIDATES {
        let (cl, cs) = if cw >= ch { (*cw, *ch) } else { (*ch, *cw) };
        if (long - cl).abs() < 2.0 && (short - cs).abs() < 2.0 {
            return name;
        }
    }
    "custom"
}

fn num(obj: &Object) -> f64 {
    match obj {
        Object::Integer(v) => *v as f64,
        Object::Real(v) => *v as f64,
        _ => 0.0,
    }
}

fn deref_dict<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Dictionary> {
    match obj {
        Object::Dictionary(d) => Some(d),
        Object::Reference(id) => doc.get_object(*id).ok().and_then(|o| o.as_dict().ok()),
        _ => None,
    }
}

fn media_box(dict: &Dictionary) -> (f64, f64) {
    if let Ok(mb) = dict.get(b"MediaBox").and_then(|o| o.as_array()) {
        if mb.len() >= 4 {
            return (
                (num(&mb[2]) - num(&mb[0])).abs(),
                (num(&mb[3]) - num(&mb[1])).abs(),
            );
        }
    }
    (595.0, 842.0)
}

fn content_bytes(doc: &Document, obj: &Object) -> u64 {
    match obj {
        Object::Reference(id) => match doc.get_object(*id) {
            Ok(Object::Stream(s)) => s.content.len() as u64,
            _ => 0,
        },
        Object::Array(arr) => arr.iter().map(|o| content_bytes(doc, o)).sum(),
        _ => 0,
    }
}

fn filter_label(obj: &Object) -> String {
    match obj {
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        Object::Array(a) => a.iter().map(filter_label).collect::<Vec<_>>().join("+"),
        _ => "?".into(),
    }
}

pub fn run(input: &str) -> Result<CommandResult, PdfError> {
    if !Path::new(input).exists() {
        return Err(PdfError::FileNotFound {
            path: input.to_string(),
        });
    }
    let file_size = std::fs::metadata(input)
        .map(|m| m.len())
        .map_err(|e| PdfError::Operation {
            reason: e.to_string(),
        })?;

    let doc = Document::load(input).map_err(|e| PdfError::ParseFailed {
        reason: e.to_string(),
    })?;

    let pages = doc.get_pages();
    let total_pages = pages.len();
    let version = doc.version.clone();
    let encrypted = doc.is_encrypted();
    let linearized = doc.trailer.get(b"Linearized").is_ok();

    let mut metadata = serde_json::json!({});
    if let Ok(info_obj) = doc.trailer.get(b"Info") {
        if let Some(d) = deref_dict(&doc, info_obj) {
            let s = |k: &[u8]| {
                d.get(k)
                    .ok()
                    .and_then(|o| o.as_str().ok())
                    .map(|b| String::from_utf8_lossy(b).into_owned())
            };
            metadata = serde_json::json!({
                "title": s(b"Title"),
                "author": s(b"Author"),
                "subject": s(b"Subject"),
                "keywords": s(b"Keywords"),
                "creator": s(b"Creator"),
                "producer": s(b"Producer"),
                "creation_date": s(b"CreationDate"),
                "mod_date": s(b"ModDate"),
            });
        }
    }

    let mut total_images: u64 = 0;
    let mut image_formats: BTreeMap<String, u64> = BTreeMap::new();
    let mut font_types: BTreeMap<String, u64> = BTreeMap::new();
    let mut page_infos: Vec<serde_json::Value> = Vec::new();
    let mut total_content_bytes: u64 = 0;
    let mut warnings: Vec<String> = Vec::new();
    let mut sizes_seen: BTreeMap<String, u64> = BTreeMap::new();

    for (page_num, page_id) in &pages {
        let page_dict = match doc.get_object(*page_id) {
            Ok(Object::Dictionary(d)) => d.clone(),
            _ => continue,
        };

        let (w_pt, h_pt) = media_box(&page_dict);
        let rotation = page_dict
            .get(b"Rotate")
            .ok()
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);

        let mut page_images: u64 = 0;
        let mut page_fonts: Vec<String> = Vec::new();

        if let Some(res) = page_dict
            .get(b"Resources")
            .ok()
            .and_then(|o| deref_dict(&doc, o))
        {
            if let Some(xd) = res.get(b"XObject").ok().and_then(|o| deref_dict(&doc, o)) {
                for (_name, xref) in xd.iter() {
                    let xobj = match xref {
                        Object::Reference(id) => doc.get_object(*id).ok(),
                        other => Some(other),
                    };
                    if let Some(Object::Stream(s)) = xobj {
                        let subtype = s
                            .dict
                            .get(b"Subtype")
                            .ok()
                            .and_then(|o| o.as_name().ok())
                            .unwrap_or(b"");
                        if subtype == b"Image" {
                            page_images += 1;
                            total_images += 1;
                            let fl = s
                                .dict
                                .get(b"Filter")
                                .map(filter_label)
                                .unwrap_or_else(|_| "raw".into());
                            *image_formats.entry(fl).or_insert(0) += 1;
                        }
                    }
                }
            }

            if let Some(fd) = res.get(b"Font").ok().and_then(|o| deref_dict(&doc, o)) {
                for (_name, fref) in fd.iter() {
                    let fobj = match fref {
                        Object::Reference(id) => doc.get_object(*id).ok(),
                        other => Some(other),
                    };
                    if let Some(Object::Dictionary(fdict)) = fobj {
                        let subtype = fdict
                            .get(b"Subtype")
                            .ok()
                            .and_then(|o| o.as_name().ok())
                            .map(|n| String::from_utf8_lossy(n).into_owned())
                            .unwrap_or_else(|| "Unknown".into());
                        let base = fdict
                            .get(b"BaseFont")
                            .ok()
                            .and_then(|o| o.as_name().ok())
                            .map(|n| String::from_utf8_lossy(n).into_owned())
                            .unwrap_or_default();
                        let key = if base.is_empty() {
                            subtype
                        } else {
                            format!("{} {}", subtype, base)
                        };
                        *font_types.entry(key.clone()).or_insert(0) += 1;
                        if !page_fonts.contains(&key) {
                            page_fonts.push(key);
                        }
                    }
                }
            }
        }

        let cbytes = page_dict
            .get(b"Contents")
            .ok()
            .map(|c| content_bytes(&doc, c))
            .unwrap_or(0);
        total_content_bytes += cbytes;

        let std = standard_size(w_pt, h_pt).to_string();
        *sizes_seen.entry(std.clone()).or_insert(0) += 1;

        if cbytes == 0 {
            warnings.push(format!("page {} is empty (no content stream)", page_num));
        }
        if rotation % 360 != 0 {
            warnings.push(format!("page {} has rotation {}°", page_num, rotation));
        }

        page_infos.push(serde_json::json!({
            "page": page_num,
            "width_pt": (w_pt * 10.0).round() / 10.0,
            "height_pt": (h_pt * 10.0).round() / 10.0,
            "width_mm": ((w_pt * PT_TO_MM) * 10.0).round() / 10.0,
            "height_mm": ((h_pt * PT_TO_MM) * 10.0).round() / 10.0,
            "standard_size": std,
            "rotation": rotation,
            "images": page_images,
            "fonts": page_fonts,
            "content_bytes": cbytes,
        }));
    }

    if sizes_seen.len() > 1 {
        let joined = sizes_seen
            .iter()
            .map(|(k, v)| format!("{}×{}", k, v))
            .collect::<Vec<_>>()
            .join(", ");
        warnings.push(format!("mixed page sizes detected: {}", joined));
    }
    if encrypted {
        warnings.push("document is encrypted".to_string());
    }
    if total_pages == 0 {
        warnings.push("no pages found".to_string());
    }

    let details = serde_json::json!({
        "file": input,
        "file_size_bytes": file_size,
        "pdf_version": version,
        "linearized": linearized,
        "encrypted": encrypted,
        "total_pages": total_pages,
        "total_images": total_images,
        "total_content_bytes": total_content_bytes,
        "image_formats": image_formats,
        "font_types": font_types,
        "page_sizes": sizes_seen,
        "metadata": metadata,
        "pages": page_infos,
        "warnings": warnings,
    });

    let mut out = String::new();
    out.push_str(&format!("File: {}\n", input));
    out.push_str(&format!(
        "Size: {} bytes ({:.1} KB)\n",
        file_size,
        file_size as f64 / 1024.0
    ));
    out.push_str(&format!("PDF version: {}\n", version));
    out.push_str(&format!(
        "Linearized: {}\n",
        if linearized { "yes" } else { "no" }
    ));
    out.push_str(&format!(
        "Encrypted: {}\n",
        if encrypted { "yes" } else { "no" }
    ));
    out.push_str(&format!("Pages: {}\n", total_pages));
    out.push_str(&format!("Images: {} total\n", total_images));
    if !image_formats.is_empty() {
        out.push_str("Image filters: ");
        out.push_str(
            &image_formats
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push('\n');
    }
    out.push_str(&format!("Content bytes: {}\n", total_content_bytes));
    if let Some(obj) = metadata.as_object() {
        let non_null: Vec<String> = obj
            .iter()
            .filter(|(_, v)| !v.is_null())
            .map(|(k, v)| format!("{}={}", k, v.as_str().unwrap_or("")))
            .collect();
        if !non_null.is_empty() {
            out.push_str(&format!("Metadata: {}\n", non_null.join(", ")));
        }
    }

    out.push_str("\nPage details:\n");
    out.push_str("  #    size(pt)      size(mm)      std     rot  imgs  fonts  content\n");
    for p in &page_infos {
        out.push_str(&format!(
            "  {:<4} {:<13} {:<13} {:<7} {:<4} {:<5} {:<6} {}\n",
            p["page"],
            format!(
                "{:.0}x{:.0}",
                p["width_pt"].as_f64().unwrap_or(0.0),
                p["height_pt"].as_f64().unwrap_or(0.0)
            ),
            format!(
                "{:.0}x{:.0}",
                p["width_mm"].as_f64().unwrap_or(0.0),
                p["height_mm"].as_f64().unwrap_or(0.0)
            ),
            p["standard_size"].as_str().unwrap_or(""),
            p["rotation"],
            p["images"],
            p["fonts"].as_array().map(|a| a.len()).unwrap_or(0),
            p["content_bytes"],
        ));
    }

    if !warnings.is_empty() {
        out.push_str("\nWarnings:\n");
        for w in &warnings {
            out.push_str(&format!("  ! {}\n", w));
        }
    }

    Ok(CommandResult::ok("doctor", Some(out), details))
}
