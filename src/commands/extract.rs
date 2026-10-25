use crate::error::PdfError;
use crate::io;
use crate::output::CommandResult;

pub fn run(
    input: &str,
    output: Option<&str>,
    pages: Option<&str>,
    password: Option<&str>,
) -> Result<CommandResult, PdfError> {
    // Loading through `io::load_doc` gives us the right error for encrypted
    // inputs (`PASSWORD_REQUIRED` / `WRONG_PASSWORD`) instead of a generic
    // parse failure, and supports `--input -` via stdin.
    let doc = io::load_doc(input, password)?;
    let total = doc.get_pages().len() as u32;

    // Strict parsing: a bad `--pages` is an error, never a silent empty result.
    let selected = io::parse_page_range(pages, total)?;

    let mut chunks: Vec<String> = Vec::new();
    let mut empty_pages: Vec<u32> = Vec::new();

    for page_no in &selected {
        let text = doc
            .extract_text(&[*page_no])
            .map_err(|e| PdfError::Operation {
                reason: format!("Failed to extract text from page {page_no}: {e}"),
            })?;
        if text.trim().is_empty() {
            empty_pages.push(*page_no);
        }
        chunks.push(text);
    }

    let final_text = chunks.join("\n\n");
    let char_count = final_text.chars().count();

    // A scanned PDF has no text layer at all. Reporting success with an empty
    // string silently is the worst outcome for an agent, so name the remedy.
    if char_count == 0 {
        return Err(PdfError::Operation {
            reason: format!(
                "No text layer found in {input}{}. The PDF is probably scanned images. \
                 Try: pdf-cli ocr --input {input} --output text.txt",
                if selected.len() == 1 {
                    format!(" (page {})", selected[0])
                } else {
                    String::new()
                }
            ),
        });
    }

    let details = serde_json::json!({
        "source": if io::is_stdio(input) { "stdin" } else { input },
        "char_count": char_count,
        "pages_requested": selected,
        "page_count": selected.len(),
        "empty_pages": empty_pages,
    });

    match output {
        Some(out_path) => {
            io::write_text(out_path, &final_text)?;
            let mut details = details;
            details["output"] = serde_json::json!(out_path);
            Ok(CommandResult::ok(
                "extract",
                Some(format!(
                    "Extracted {} chars → {}",
                    char_count, out_path
                )),
                details,
            ))
        }
        None => Ok(CommandResult::ok(
            "extract",
            Some(final_text),
            details,
        )),
    }
}
