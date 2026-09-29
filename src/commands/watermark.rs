use crate::error::PdfError;
use crate::output::CommandResult;
use lopdf::{dictionary, Document, Object};
use std::path::Path;

pub fn add(
    input: &str,
    text: &str,
    output: &str,
    opacity: f32,
    rotation: f32,
    font_size: f32,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    if !Path::new(input).exists() {
        return Err(PdfError::FileNotFound {
            path: input.to_string(),
        });
    }

    if dry_run {
        return Ok(CommandResult::ok(
            "watermark-add",
            Some(format!(
                "[dry-run] Would add watermark '{}' to {}",
                text, input
            )),
            serde_json::json!({"dry_run": true, "text": text, "input": input}),
        ));
    }

    let mut doc = Document::load(input).map_err(|e| PdfError::ParseFailed {
        reason: e.to_string(),
    })?;

    let pages = doc.get_pages();
    let page_count = pages.len();

    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });

    for (_, page_id) in pages {
        let content = format!(
            "q\n{opacity} g\nBT\n/F1 {fs} Tf\n{rot} {rot2} {neg_rot} {rot} 0 0 Tm\n({txt}) Tj\nET\nQ\n",
            opacity = opacity,
            fs = font_size,
            rot = format!("{:.4}", rotation.to_radians().cos()),
            rot2 = format!("{:.4}", rotation.to_radians().sin()),
            neg_rot = format!("{:.4}", -rotation.to_radians().sin()),
            txt = text.replace('(', "\\(").replace(')', "\\)"),
        );

        let stream = lopdf::Stream::new(lopdf::Dictionary::new(), content.into_bytes());
        let stream_id = doc.add_object(stream);

        if let Ok(page_dict) = doc.get_object_mut(page_id).and_then(|o| o.as_dict_mut()) {
            page_dict.set(
                "Resources",
                dictionary! {
                    "Font" => dictionary! {
                        "F1" => font_id,
                    },
                },
            );
            let existing = page_dict
                .get(b"Contents")
                .ok()
                .and_then(|o| o.as_array().ok())
                .cloned();
            match existing {
                Some(mut arr) => {
                    arr.push(Object::Reference(stream_id));
                    page_dict.set("Contents", Object::Array(arr));
                }
                None => {
                    let original = page_dict.get(b"Contents").ok().cloned();
                    let mut arr = vec![Object::Reference(stream_id)];
                    if let Some(orig) = original {
                        arr.push(orig);
                    }
                    page_dict.set("Contents", Object::Array(arr));
                }
            }
        }
    }

    doc.save(output).map_err(|e| PdfError::Operation {
        reason: e.to_string(),
    })?;

    Ok(CommandResult::ok(
        "watermark-add",
        Some(format!(
            "Added '{}' watermark to {} pages → {}",
            text, page_count, output
        )),
        serde_json::json!({"output": output, "text": text, "pages": page_count}),
    ))
}
