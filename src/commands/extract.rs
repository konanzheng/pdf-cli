use crate::error::PdfError;
use crate::io;
use crate::output::CommandResult;

pub fn run(
    input: &str,
    output: Option<&str>,
    pages: Option<&str>,
    password: Option<&str>,
) -> Result<CommandResult, PdfError> {
    // A batch of documents (`a.pdf,b.pdf`, a directory, or a glob) is processed
    // document by document. The merged text is the payload, and per-document
    // results are reported in `documents` so a batch run stays inspectable.
    let files = io::expand_batch(input)?;

    if files.len() == 1 {
        let single = io::path_to_arg(&files[0]);
        return run_one(&single, output, pages, password);
    }

    let mut combined: Vec<String> = Vec::new();
    let mut documents: Vec<serde_json::Value> = Vec::new();
    let mut total_chars = 0usize;

    for path in &files {
        let one = io::path_to_arg(path);
        // Fail loudly per document: a batch that silently skips a bad file would
        // let an agent believe it saw the whole set.
        let r = run_one(&one, None, pages, password)?;
        let text = r.output.clone().unwrap_or_default();
        total_chars += r.details["char_count"].as_u64().unwrap_or(0) as usize;
        documents.push(serde_json::json!({
            "source": r.details["source"],
            "char_count": r.details["char_count"],
            "page_count": r.details["page_count"],
            "empty_pages": r.details["empty_pages"],
        }));
        combined.push(text);
    }

    let joined = combined.join("\n\n");
    let mut details = serde_json::json!({
        "source": input,
        "document_count": files.len(),
        "char_count": total_chars,
        "documents": documents,
    });

    match output {
        Some(out_path) if io::is_stdio(out_path) => {
            details["output"] = serde_json::json!("-");
            Ok(CommandResult::ok("extract", Some(joined), details))
        }
        Some(out_path) => {
            io::write_text(out_path, &joined)?;
            details["output"] = serde_json::json!(out_path);
            Ok(CommandResult::ok(
                "extract",
                Some(format!(
                    "Extracted {total_chars} chars from {} documents → {out_path}",
                    files.len()
                )),
                details,
            ))
        }
        None => Ok(CommandResult::ok("extract", Some(joined), details)),
    }
}

fn run_one(
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

    let mut details = serde_json::json!({
        "source": if io::is_stdio(input) { "stdin" } else { input },
        "char_count": char_count,
        "pages_requested": selected,
        "page_count": selected.len(),
        "empty_pages": empty_pages,
    });

    match output {
        Some(out_path) if io::is_stdio(out_path) => {
            // `--output -` and "no --output" both mean "the text is the result".
            // main() rejects `--output -` combined with `--json`/`--json-lines`,
            // so stdout carries either the text or the envelope, never both.
            details["output"] = serde_json::json!("-");
            Ok(CommandResult::ok("extract", Some(final_text), details))
        }
        Some(out_path) => {
            io::write_text(out_path, &final_text)?;
            details["output"] = serde_json::json!(out_path);
            Ok(CommandResult::ok(
                "extract",
                Some(format!("Extracted {char_count} chars → {out_path}")),
                details,
            ))
        }
        None => Ok(CommandResult::ok("extract", Some(final_text), details)),
    }
}
