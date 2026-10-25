use crate::error::PdfError;
use crate::io;
use crate::output::CommandResult;
use lopdf::{Dictionary, Document, Object, StringFormat};
use std::path::Path;

fn set_str(dict: &mut Dictionary, key: &str, val: &str) {
    dict.set(
        key.as_bytes().to_vec(),
        Object::String(val.as_bytes().to_vec(), StringFormat::Literal),
    );
}

fn get_str(dict: &Dictionary, key: &[u8]) -> Option<String> {
    dict.get(key)
        .ok()
        .and_then(|o| o.as_str().ok())
        .map(|b| String::from_utf8_lossy(b).into_owned())
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    input: &str,
    output: &str,
    title: Option<&str>,
    author: Option<&str>,
    subject: Option<&str>,
    keywords: Option<&str>,
    creator: Option<&str>,
    producer: Option<&str>,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    if !Path::new(input).exists() {
        return Err(PdfError::FileNotFound {
            path: input.to_string(),
        });
    }

    let mut doc = io::load_doc_path(input)?;

    if dry_run {
        return Ok(CommandResult::ok(
            "meta",
            Some(format!("[dry-run] Would update metadata of {}", input)),
            serde_json::json!({
                "dry_run": true,
                "input": input,
                "title": title,
                "author": author,
                "subject": subject,
                "keywords": keywords,
                "creator": creator,
                "producer": producer,
            }),
        ));
    }

    // 取出（或新建）Info 字典
    let info_id = doc
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|o| o.as_reference().ok());

    let mut info = match info_id {
        Some(id) => doc
            .get_object(id)
            .and_then(|o| o.as_dict())
            .cloned()
            .unwrap_or_default(),
        None => Dictionary::new(),
    };

    if let Some(v) = title {
        set_str(&mut info, "Title", v);
    }
    if let Some(v) = author {
        set_str(&mut info, "Author", v);
    }
    if let Some(v) = subject {
        set_str(&mut info, "Subject", v);
    }
    if let Some(v) = keywords {
        set_str(&mut info, "Keywords", v);
    }
    if let Some(v) = creator {
        set_str(&mut info, "Creator", v);
    }
    if let Some(v) = producer {
        set_str(&mut info, "Producer", v);
    }

    // 回写
    match info_id {
        Some(id) => {
            if let Ok(obj) = doc.get_object_mut(id) {
                *obj = Object::Dictionary(info.clone());
            }
        }
        None => {
            let new_id = doc.add_object(Object::Dictionary(info.clone()));
            doc.trailer.set("Info", Object::Reference(new_id));
        }
    }

    doc.save(output).map_err(|e| PdfError::Operation {
        reason: e.to_string(),
    })?;

    // 回读确认
    let check = Document::load(output).ok();
    let after = check
        .and_then(|d| {
            d.trailer
                .get(b"Info")
                .ok()
                .and_then(|o| o.as_reference().ok())
                .and_then(|id| d.get_object(id).ok())
                .and_then(|o| o.as_dict().ok())
                .map(|dict| {
                    serde_json::json!({
                        "title": get_str(dict, b"Title"),
                        "author": get_str(dict, b"Author"),
                        "subject": get_str(dict, b"Subject"),
                        "keywords": get_str(dict, b"Keywords"),
                        "creator": get_str(dict, b"Creator"),
                        "producer": get_str(dict, b"Producer"),
                    })
                })
        })
        .unwrap_or(serde_json::json!({}));

    Ok(CommandResult::ok(
        "meta",
        Some(format!("Updated metadata → {}", output)),
        serde_json::json!({
            "output": output,
            "metadata": after,
        }),
    ))
}
