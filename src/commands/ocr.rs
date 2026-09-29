use crate::error::PdfError;
use crate::output::CommandResult;
use std::path::Path;

/// 用 pdfium 把 PDF 某页渲染成 PNG 字节。
fn render_page_png(
    pdfium: &pdfium_render::prelude::Pdfium,
    input: &str,
    page_index: u16,
    width: u32,
) -> Result<Vec<u8>, PdfError> {
    use pdfium_render::prelude::*;
    let doc = pdfium
        .load_pdf_from_file(input, None)
        .map_err(|e| PdfError::ParseFailed {
            reason: e.to_string(),
        })?;
    let page = doc
        .pages()
        .get(page_index)
        .map_err(|e| PdfError::Operation {
            reason: format!("page {}: {}", page_index + 1, e),
        })?;
    let config = PdfRenderConfig::new().set_target_width(width as i32);
    let bitmap = page
        .render_with_config(&config)
        .map_err(|e| PdfError::Operation {
            reason: format!("render page {}: {}", page_index + 1, e),
        })?;
    let img = bitmap.as_image();
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| PdfError::Operation {
            reason: e.to_string(),
        })?;
    Ok(buf.into_inner())
}

fn ocr_bytes(service: &str, png: &[u8]) -> Result<(String, usize), PdfError> {
    let url = format!("{}/ocr/raw", service.trim_end_matches('/'));
    let resp = ureq::post(&url)
        .set("Content-Type", "image/png")
        .send_bytes(png);

    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            return Err(PdfError::Operation {
                reason: format!("OCR service returned HTTP {}: {}", code, body),
            });
        }
        Err(e) => {
            return Err(PdfError::Operation {
                reason: format!(
                    "Cannot reach OCR service at {}. Start ocr-rust first (see ROADMAP.md). Error: {}",
                    service, e
                ),
            });
        }
    };

    let json: serde_json::Value = resp.into_json().map_err(|e| PdfError::Operation {
        reason: format!("Invalid OCR response: {}", e),
    })?;

    let text = json
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let regions = json
        .get("regions")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    Ok((text, regions))
}

pub fn run(
    input: &str,
    output: Option<&str>,
    service: &str,
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

    if dry_run {
        return Ok(CommandResult::ok(
            "ocr",
            Some(format!("[dry-run] Would OCR {} via {}", input, service)),
            serde_json::json!({"dry_run": true, "input": input, "service": service}),
        ));
    }

    // 绑定 pdfium
    let bindings = match pdfium_path {
        Some(p) => pdfium_render::prelude::Pdfium::bind_to_library(p),
        None => pdfium_render::prelude::Pdfium::bind_to_system_library(),
    }
    .map_err(|e| PdfError::Operation {
        reason: format!(
            "pdfium library not found ({}). Pass --pdfium-path <pdfium.dll>",
            e
        ),
    })?;
    let pdfium = pdfium_render::prelude::Pdfium::new(bindings);

    // 先探页数
    let total = {
        let d = pdfium
            .load_pdf_from_file(input, None)
            .map_err(|e| PdfError::ParseFailed {
                reason: e.to_string(),
            })?;
        d.pages().len() as u32
    };

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
            range: pages_spec.unwrap_or("(all)").into(),
        });
    }

    let mut all_text = Vec::new();
    let mut page_results = Vec::new();
    let mut total_regions = 0usize;

    for &p in &selected {
        let png = render_page_png(&pdfium, input, (p - 1) as u16, width)?;
        let (text, regions) = ocr_bytes(service, &png)?;
        total_regions += regions;
        page_results.push(serde_json::json!({
            "page": p,
            "chars": text.chars().count(),
            "regions": regions,
        }));
        all_text.push(text);
    }

    let joined = all_text.join("\n\n");
    let char_count = joined.chars().count();

    let out_info = if let Some(out_path) = output {
        std::fs::write(out_path, &joined).map_err(|e| PdfError::Operation {
            reason: e.to_string(),
        })?;
        Some(out_path.to_string())
    } else {
        None
    };

    let preview = if output.is_none() {
        Some(joined.clone())
    } else {
        None
    };

    Ok(CommandResult::ok(
        "ocr",
        preview.or_else(|| {
            Some(format!(
                "OCR'd {} page(s) → {}",
                selected.len(),
                output.unwrap_or("")
            ))
        }),
        serde_json::json!({
            "input": input,
            "service": service,
            "pages_processed": selected.len(),
            "total_regions": total_regions,
            "char_count": char_count,
            "output": out_info,
            "pages": page_results,
        }),
    ))
}
