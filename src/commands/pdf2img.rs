use crate::error::PdfError;
use crate::output::CommandResult;
use std::path::Path;

pub fn run(
    input: &str,
    output_dir: &str,
    format: &str,
    width: u32,
    pages_spec: Option<&str>,
    pdfium_path: Option<&str>,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    if !Path::new(input).exists() {
        return Err(PdfError::FileNotFound {
            path: input.to_string(),
        });
    }

    let fmt = format.to_lowercase();
    if fmt != "png" && fmt != "jpeg" && fmt != "jpg" {
        return Err(PdfError::Operation {
            reason: format!("Unsupported image format: {}. Use png or jpeg.", format),
        });
    }

    if dry_run {
        return Ok(CommandResult::ok(
            "pdf-to-image",
            Some(format!(
                "[dry-run] Would render {} → {} ({}px wide, {})",
                input, output_dir, width, fmt
            )),
            serde_json::json!({"dry_run": true, "input": input, "output_dir": output_dir, "format": fmt, "width": width}),
        ));
    }

    std::fs::create_dir_all(output_dir).map_err(|e| PdfError::Operation {
        reason: e.to_string(),
    })?;

    // 绑定 pdfium 动态库
    let bindings = match pdfium_path {
        Some(p) => pdfium_render::prelude::Pdfium::bind_to_library(p),
        None => pdfium_render::prelude::Pdfium::bind_to_system_library(),
    }
    .map_err(|e| PdfError::Operation {
        reason: format!(
            "pdfium library not found ({}). Download from https://github.com/bblanchon/pdfium-binaries/releases and pass --pdfium-path <path to pdfium.dll>",
            e
        ),
    })?;

    use pdfium_render::prelude::*;
    let pdfium = Pdfium::new(bindings);

    let doc = pdfium
        .load_pdf_from_file(input, None)
        .map_err(|e| PdfError::ParseFailed {
            reason: e.to_string(),
        })?;

    let total = doc.pages().len() as u32;
    let selected: Vec<u32> = match pages_spec {
        Some(spec) => {
            let mut v = Vec::new();
            for part in spec.split(',') {
                let part = part.trim();
                if part.contains('-') {
                    let b: Vec<&str> = part.splitn(2, '-').collect();
                    let s: u32 = b[0]
                        .trim()
                        .parse()
                        .map_err(|_| PdfError::InvalidPageRange { range: spec.into() })?;
                    let e: u32 = b[1]
                        .trim()
                        .parse()
                        .map_err(|_| PdfError::InvalidPageRange { range: spec.into() })?;
                    for p in s..=e {
                        if p >= 1 && p <= total {
                            v.push(p);
                        }
                    }
                } else if let Ok(p) = part.parse::<u32>() {
                    if p >= 1 && p <= total {
                        v.push(p);
                    }
                }
            }
            v
        }
        None => (1..=total).collect(),
    };

    if selected.is_empty() {
        return Err(PdfError::InvalidPageRange {
            range: pages_spec.unwrap_or("(all)").to_string(),
        });
    }

    let stem = Path::new(input)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("page");
    let ext = if fmt == "jpg" { "jpeg" } else { fmt.as_str() };

    let config = PdfRenderConfig::new().set_target_width(width as i32);

    let mut outputs = Vec::new();
    for &p in &selected {
        let page = doc
            .pages()
            .get((p - 1) as u16)
            .map_err(|e| PdfError::Operation {
                reason: format!("page {}: {}", p, e),
            })?;
        let bitmap = page
            .render_with_config(&config)
            .map_err(|e| PdfError::Operation {
                reason: format!("render page {}: {}", p, e),
            })?;
        let img = bitmap.as_image();
        let out_path = format!("{}/{}_{:03}.{}", output_dir, stem, p, ext);
        img.save(&out_path).map_err(|e| PdfError::Operation {
            reason: format!("save {}: {}", out_path, e),
        })?;
        outputs.push(out_path);
    }

    Ok(CommandResult::ok(
        "pdf-to-image",
        Some(format!(
            "Rendered {} page(s) → {} ({}px wide)",
            outputs.len(),
            output_dir,
            width
        )),
        serde_json::json!({
            "input": input,
            "output_dir": output_dir,
            "format": fmt,
            "width": width,
            "page_count": outputs.len(),
            "files": outputs,
        }),
    ))
}
