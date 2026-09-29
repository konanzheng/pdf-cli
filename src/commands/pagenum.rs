use crate::error::PdfError;
use crate::output::CommandResult;
use lopdf::{dictionary, Dictionary, Document, Object, Stream};
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
                Object::Real(v) => *v as f32,
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

#[allow(clippy::too_many_arguments)]
pub fn run(
    input: &str,
    output: &str,
    position: &str,
    format: &str,
    start: u32,
    font_size: f32,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    if !Path::new(input).exists() {
        return Err(PdfError::FileNotFound {
            path: input.to_string(),
        });
    }

    let mut doc = Document::load(input).map_err(|e| PdfError::ParseFailed {
        reason: e.to_string(),
    })?;

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
                "pages": total,
                "position": position,
                "format": format,
                "start": start,
            }),
        ));
    }

    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });

    let mut numbered = 0u32;
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
        let esc = text
            .replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)");
        let content = format!(
            "q\nBT\n/F1 {:.1} Tf\n0 g\n{:.1} {:.1} Td\n({}) Tj\nET\nQ\n",
            font_size, x, y, esc
        );
        let content_id = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));

        if let Ok(d) = doc.get_object_mut(page_id).and_then(|o| o.as_dict_mut()) {
            // 合并 Resources
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
            font_dict.set("F1", Object::Reference(font_id));
            res.set("Font", Object::Dictionary(font_dict));
            d.set("Resources", Object::Dictionary(res));

            // 追加内容流
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

    doc.save(output).map_err(|e| PdfError::Operation {
        reason: e.to_string(),
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
        }),
    ))
}
