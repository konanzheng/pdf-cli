use crate::error::PdfError;
use crate::output::CommandResult;
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use std::path::Path;

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
    font_id: ObjectId,
) -> Result<Vec<ObjectId>, PdfError> {
    let line_height = font_size * 1.5;
    let margin = 50.0;
    let lines_per_page = (((ph - 2.0 * margin) / line_height).floor() as usize).max(1);

    let all_lines: Vec<&str> = text.lines().collect();
    let chunks: Vec<&[&str]> = if all_lines.is_empty() {
        vec![&[][..]]
    } else {
        all_lines.chunks(lines_per_page).collect()
    };

    let mut page_ids = Vec::new();
    for chunk in chunks {
        let mut content = String::from("BT\n");
        content.push_str(&format!("/F1 {:.1} Tf\n", font_size));
        let start_y = ph - margin;
        content.push_str(&format!("{:.1} {:.1} Td\n", margin, start_y));
        for (i, line) in chunk.iter().enumerate() {
            if i > 0 {
                content.push_str(&format!("0 -{:.1} Td\n", line_height));
            }
            let esc = line
                .replace('\\', "\\\\")
                .replace('(', "\\(")
                .replace(')', "\\)");
            content.push_str(&format!("({}) Tj\n", esc));
        }
        content.push_str("ET\n");
        let content_id = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));

        let mut font_dict = Dictionary::new();
        font_dict.set("F1", Object::Reference(font_id));
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

pub fn run(
    input: &str,
    output: &str,
    page_size: &str,
    font_size: f32,
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

    if dry_run {
        return Ok(CommandResult::ok(
            "convert",
            Some(format!(
                "[dry-run] Would convert {} file(s) → {}",
                files.len(),
                output
            )),
            serde_json::json!({"dry_run": true, "input_count": files.len(), "output": output}),
        ));
    }

    let (pw, ph) = page_size_pt(page_size);
    let mut doc = Document::with_version("1.5");

    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });

    let mut page_ids: Vec<ObjectId> = Vec::new();

    for file in &files {
        let ext = Path::new(file)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "jpg" | "jpeg" | "png" | "bmp" | "webp" | "tiff" | "gif" => {
                let id = make_image_page(&mut doc, file, pw, ph)?;
                page_ids.push(id);
            }
            "md" | "txt" => {
                let text = std::fs::read_to_string(file).map_err(|e| PdfError::Operation {
                    reason: e.to_string(),
                })?;
                let ids = make_text_pages(&mut doc, &text, pw, ph, font_size, font_id)?;
                page_ids.extend(ids);
            }
            _ => return Err(PdfError::UnsupportedFormat { format: ext }),
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

    Ok(CommandResult::ok(
        "convert",
        Some(format!(
            "Converted {} file(s) → {} ({} pages)",
            files.len(),
            output,
            page_ids.len()
        )),
        serde_json::json!({
            "output": output,
            "input_count": files.len(),
            "pages_created": page_ids.len(),
            "page_size": page_size,
        }),
    ))
}
