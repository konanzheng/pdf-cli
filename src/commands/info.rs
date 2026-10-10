use crate::error::PdfError;
use crate::output::CommandResult;
use lopdf::Document;

pub fn run(input: &str) -> Result<CommandResult, PdfError> {
    if !std::path::Path::new(input).exists() {
        return Err(PdfError::FileNotFound {
            path: input.to_string(),
        });
    }

    let doc = Document::load(input).map_err(|e| PdfError::ParseFailed {
        reason: e.to_string(),
    })?;

    let pages = doc.get_pages();
    let page_count = pages.len();
    let encrypted = doc.is_encrypted();
    let version = doc.version.clone();

    let metadata = doc
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|obj| obj.as_reference().ok())
        .and_then(|id| doc.get_object(id).ok())
        .and_then(|obj| obj.as_dict().ok())
        .map(|dict| {
            let get_str = |key: &[u8]| -> String {
                dict.get(key)
                    .ok()
                    .and_then(|o| o.as_str().ok())
                    .map(|s| String::from_utf8_lossy(s).to_string())
                    .unwrap_or_default()
            };
            serde_json::json!({
                "title": get_str(b"Title"),
                "author": get_str(b"Author"),
                "subject": get_str(b"Subject"),
                "creator": get_str(b"Creator"),
                "producer": get_str(b"Producer"),
            })
        })
        .unwrap_or(serde_json::json!({}));

    let details = serde_json::json!({
        "file": input,
        "pages": page_count,
        "encrypted": encrypted,
        "pdf_version": version,
        "metadata": metadata,
    });

    let out = format!(
        "File: {}\nPDF version: {}\nPages: {}\nEncrypted: {}\nMetadata: {}",
        input,
        version,
        page_count,
        if encrypted { "yes" } else { "no" },
        serde_json::to_string_pretty(&details["metadata"]).unwrap_or_default()
    );

    Ok(CommandResult::ok("info", Some(out), details))
}
