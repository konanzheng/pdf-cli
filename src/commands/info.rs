use crate::error::PdfError;
use crate::io;
use crate::output::CommandResult;

pub fn run(input: &str) -> Result<CommandResult, PdfError> {
    // No local existence check here: `io::load_doc_path` resolves files,
    // directories and globs through `expand_inputs`, and reports a glob or
    // directory that matches several files as an actionable error. A bare
    // `Path::exists()` would reject those inputs before they are expanded.

    let doc = io::load_doc_path(input)?;

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

    // Report the path the input actually resolved to alongside the raw argument:
    // with a glob or a directory the two differ, and an agent that echoes `file`
    // back into a later command needs the concrete path.
    let resolved = io::resolve_input_once(input)
        .map(|p| io::path_to_arg(&p))
        .unwrap_or_else(|_| input.to_string());

    let details = serde_json::json!({
        "file": input,
        "resolved": resolved,
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
