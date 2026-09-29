use crate::error::PdfError;
use crate::output::CommandResult;

pub fn run(
    input: &str,
    output: Option<&str>,
    pages: Option<&str>,
) -> Result<CommandResult, PdfError> {
    if !std::path::Path::new(input).exists() {
        return Err(PdfError::FileNotFound {
            path: input.to_string(),
        });
    }

    let bytes = std::fs::read(input).map_err(|e| PdfError::Operation {
        reason: e.to_string(),
    })?;

    let text = pdf_extract::extract_text_from_mem(&bytes).map_err(|e| PdfError::ParseFailed {
        reason: e.to_string(),
    })?;

    let final_text = if let Some(pages_spec) = pages {
        let page_texts: Vec<&str> = text.split('\x0C').collect();
        let mut selected = Vec::new();
        for part in pages_spec.split(',') {
            if part.contains('-') {
                let bounds: Vec<usize> = part
                    .splitn(2, '-')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                if bounds.len() == 2 {
                    for i in bounds[0]..=bounds[1] {
                        if i >= 1 && i <= page_texts.len() {
                            selected.push(page_texts[i - 1]);
                        }
                    }
                }
            } else if let Ok(p) = part.trim().parse::<usize>() {
                if p >= 1 && p <= page_texts.len() {
                    selected.push(page_texts[p - 1]);
                }
            }
        }
        selected.join("\n\x0C\n")
    } else {
        text.clone()
    };

    if let Some(out_path) = output {
        std::fs::write(out_path, &final_text).map_err(|e| PdfError::Operation {
            reason: e.to_string(),
        })?;
        Ok(CommandResult::ok(
            "extract",
            Some(format!(
                "Extracted {} chars → {}",
                final_text.len(),
                out_path
            )),
            serde_json::json!({"output": out_path, "char_count": final_text.len()}),
        ))
    } else {
        Ok(CommandResult::ok(
            "extract",
            Some(final_text),
            serde_json::json!({"source": input, "char_count": text.len()}),
        ))
    }
}
