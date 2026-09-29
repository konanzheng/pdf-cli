use crate::error::PdfError;
use crate::output::CommandResult;
use lopdf::Document;
use std::path::Path;

pub fn compress(
    input: &str,
    output: &str,
    level: &str,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    if !Path::new(input).exists() {
        return Err(PdfError::FileNotFound {
            path: input.to_string(),
        });
    }

    let input_size = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);

    if dry_run {
        return Ok(CommandResult::ok(
            "compress",
            Some(format!(
                "[dry-run] Would compress {} ({:.1} KB) → {}",
                input,
                input_size as f64 / 1024.0,
                output
            )),
            serde_json::json!({"dry_run": true, "input_size": input_size, "level": level}),
        ));
    }

    let mut doc = Document::load(input).map_err(|e| PdfError::ParseFailed {
        reason: e.to_string(),
    })?;

    // Remove unused objects and recompress streams
    doc.prune_objects();
    doc.compress();

    doc.save(output).map_err(|e| PdfError::Operation {
        reason: e.to_string(),
    })?;

    let output_size = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
    let ratio = if input_size > 0 {
        (1.0 - output_size as f64 / input_size as f64) * 100.0
    } else {
        0.0
    };

    Ok(CommandResult::ok(
        "compress",
        Some(format!(
            "Compressed: {:.1} KB → {:.1} KB (saved {:.0}%)",
            input_size as f64 / 1024.0,
            output_size as f64 / 1024.0,
            ratio
        )),
        serde_json::json!({
            "output": output,
            "input_size": input_size,
            "output_size": output_size,
            "saved_percent": format!("{:.1}", ratio),
            "level": level
        }),
    ))
}
