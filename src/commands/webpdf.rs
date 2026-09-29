use crate::error::PdfError;
use crate::output::CommandResult;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn find_edge(explicit: Option<&str>) -> Result<PathBuf, PdfError> {
    if let Some(p) = explicit {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Ok(pb);
        }
        return Err(PdfError::BrowserNotFound {
            reason: format!("specified path does not exist: {}", p),
        });
    }
    let candidates = [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files (x86)\Microsoft\Edge Beta\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge Beta\Application\msedge.exe",
        r"C:\Program Files (x86)\Microsoft\Edge Dev\Application\msedge.exe",
    ];
    for c in candidates {
        if Path::new(c).is_file() {
            return Ok(PathBuf::from(c));
        }
    }
    Err(PdfError::BrowserNotFound {
        reason: "no Microsoft Edge installation detected in standard locations".into(),
    })
}

/// 本地路径 → file:/// URL（正确处理 Windows 的 \\?\ 前缀）。
fn to_file_url(path: &Path) -> String {
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut s = abs.to_string_lossy().replace('\\', "/");
    if let Some(rest) = s.strip_prefix("//?/") {
        s = rest.to_string();
    }
    let s = s.trim_start_matches('/');
    format!("file:///{}", s)
}

fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// 为 SVG / 图片生成 A4 包装 HTML。
fn wrap_html(src_url: &str, tmp_dir: &Path, landscape: bool) -> Result<PathBuf, PdfError> {
    let page = if landscape { "A4 landscape" } else { "A4" };
    let html = format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><style>
@page {{ size: {}; margin: 0; }}
html, body {{ margin: 0; padding: 0; width: 100%; height: 100%; }}
body {{ display: flex; align-items: center; justify-content: center; }}
img {{ display: block; max-width: 100%; max-height: 100vh; }}
</style></head><body><img src="{}"></body></html>"#,
        page, src_url
    );
    let p = tmp_dir.join("wrap.html");
    std::fs::write(&p, html).map_err(|e| PdfError::Operation {
        reason: e.to_string(),
    })?;
    Ok(p)
}

pub fn run(
    input: &str,
    output: &str,
    edge_path: Option<&str>,
    landscape: bool,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    let edge = find_edge(edge_path)?;

    let (url, kind) = if is_url(input) {
        (input.to_string(), "url")
    } else {
        if !Path::new(input).exists() {
            return Err(PdfError::FileNotFound {
                path: input.to_string(),
            });
        }
        let ext = Path::new(input)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let k = match ext.as_str() {
            "svg" => "svg",
            "html" | "htm" => "html",
            _ => "image",
        };
        (to_file_url(Path::new(input)), k)
    };

    if dry_run {
        return Ok(CommandResult::ok(
            "web-to-pdf",
            Some(format!(
                "[dry-run] Would print {} ({}) → {}",
                input, kind, output
            )),
            serde_json::json!({"dry_run": true, "input": input, "kind": kind, "output": output, "edge": edge.to_string_lossy()}),
        ));
    }

    // 计算绝对输出路径并确保父目录存在
    let out_path = {
        let p = Path::new(output);
        let parent = p
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent).ok();
        let abs_parent = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
        abs_parent.join(p.file_name().unwrap_or_default())
    };
    let _ = std::fs::remove_file(&out_path);
    let out_str = out_path.to_string_lossy().to_string();

    let tmp = tempfile::tempdir().map_err(|e| PdfError::Operation {
        reason: e.to_string(),
    })?;
    let profile = tmp.path().join("profile");
    std::fs::create_dir_all(&profile).ok();

    let target_url = match kind {
        "svg" | "image" => {
            let wrap = wrap_html(&url, tmp.path(), landscape)?;
            to_file_url(&wrap)
        }
        _ => url.clone(),
    };

    let mut cmd = Command::new(&edge);
    cmd.arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-extensions")
        .arg("--virtual-time-budget=10000")
        .arg("--no-pdf-header-footer")
        .arg(format!("--user-data-dir={}", profile.to_string_lossy()))
        .arg(format!("--print-to-pdf={}", out_str))
        .arg(&target_url);

    let result = cmd.output().map_err(|e| PdfError::Operation {
        reason: format!("failed to launch Edge: {}", e),
    })?;

    // Edge 可能 fork，主进程退出时 PDF 尚未写完 —— 轮询等待文件落盘并稳定
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut last: u64 = 0;
    loop {
        if out_path.exists() {
            let sz = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
            if sz > 0 && sz == last {
                break;
            }
            last = sz;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    let size = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        let stderr = String::from_utf8_lossy(&result.stderr);
        return Err(PdfError::Operation {
            reason: format!(
                "Edge did not produce a PDF (exit={:?}). stderr: {}",
                result.status.code(),
                stderr.chars().take(500).collect::<String>()
            ),
        });
    }

    Ok(CommandResult::ok(
        "web-to-pdf",
        Some(format!(
            "Printed {} ({}) → {} ({} bytes)",
            input, kind, output, size
        )),
        serde_json::json!({
            "input": input,
            "kind": kind,
            "output": output,
            "size_bytes": size,
            "edge": edge.to_string_lossy(),
        }),
    ))
}
