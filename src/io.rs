//! Shared I/O helpers: stdin/stdout (`-`), password-aware document loading,
//! strict page-range parsing, input globbing and pdfium binding.
//!
//! Every helper here returns [`PdfError`] so commands can stay short and the
//! JSON error contract stays uniform.

use crate::error::PdfError;
use lopdf::Document;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The magic path that means "stdin" for inputs or "stdout" for outputs.
pub const STDIO: &str = "-";

/// Process-wide password supplied via the global `--password` flag.
///
/// Commands that accept their own password pass it explicitly; everything else
/// falls back to this, so a single `--password` on the command line unlocks every
/// input without threading the value through 17 function signatures.
static GLOBAL_PASSWORD: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Record the global `--password` value. Later calls are ignored, which keeps the
/// value immutable for the lifetime of the process.
pub fn set_global_password(password: Option<&str>) {
    if let Some(p) = password {
        if !p.is_empty() {
            let _ = GLOBAL_PASSWORD.set(p.to_string());
        }
    }
}

fn effective_password(explicit: Option<&str>) -> Option<&str> {
    match explicit {
        Some(p) if !p.is_empty() => Some(p),
        _ => GLOBAL_PASSWORD.get().map(|s| s.as_str()),
    }
}

pub fn is_stdio(path: &str) -> bool {
    path == STDIO
}

/// Read raw bytes from a file path, or from stdin when `input == "-"`.
///
/// A directory or glob is accepted only when it resolves to exactly one PDF;
/// resolving to several is a mistake worth reporting rather than guessing which
/// file the caller meant. Use `expand_inputs` for the batch form.
pub fn read_bytes(input: &str) -> Result<Vec<u8>, PdfError> {
    if is_stdio(input) {
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(|e| PdfError::Operation {
                reason: format!("Failed to read stdin: {e}"),
            })?;
        if buf.is_empty() {
            return Err(PdfError::Operation {
                reason: "No data received on stdin".into(),
            });
        }
        Ok(buf)
    } else if !Path::new(input).exists() && !looks_like_glob(input) {
        Err(PdfError::FileNotFound {
            path: input.to_string(),
        })
    } else {
        let path = resolve_single_input(input)?;
        std::fs::read(&path).map_err(|e| PdfError::Operation {
            reason: format!("Failed to read {input}: {e}"),
        })
    }
}

/// Resolve `input` to the one file it names.
///
/// A plain file is returned as-is. Directories and globs are expanded via
/// [`expand_inputs`]; more than one match is rejected because every command that
/// funnels through [`read_bytes`] operates on a single document.
fn resolve_single_input(input: &str) -> Result<PathBuf, PdfError> {
    if Path::new(input).is_file() {
        return Ok(PathBuf::from(input));
    }

    let mut files = expand_inputs(input)?;
    match files.len() {
        1 => files
            .pop()
            .ok_or_else(|| PdfError::FileNotFound {
                path: input.to_string(),
            }),
        n => Err(PdfError::Operation {
            reason: format!(
                "\"{input}\" matched {n} PDF files, but this command reads a single document.\n  \
                 → Pass one explicit file, or use a command that accepts batches"
            ),
        }),
    }
}

/// Write raw bytes to a file path, or to stdout when `output == "-"`.
pub fn write_bytes(output: &str, data: &[u8]) -> Result<(), PdfError> {
    if is_stdio(output) {
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        lock.write_all(data).map_err(|e| PdfError::Operation {
            reason: format!("Failed to write stdout: {e}"),
        })?;
        lock.flush().map_err(|e| PdfError::Operation {
            reason: format!("Failed to flush stdout: {e}"),
        })
    } else {
        std::fs::write(output, data).map_err(|e| PdfError::Operation {
            reason: format!("Failed to write {output}: {e}"),
        })
    }
}

/// Write text, appending a trailing newline when writing to a file (not stdout).
pub fn write_text(output: &str, text: &str) -> Result<(), PdfError> {
    if is_stdio(output) {
        write_bytes(output, text.as_bytes())
    } else {
        write_bytes(output, format!("{text}\n").as_bytes())
    }
}

/// Load a PDF from a path or stdin, transparently decrypting it with `password`.
///
/// `password` may be `None`; the global `--password` value is then used.
/// Returns [`PdfError::PasswordRequired`] when the file is encrypted and no
/// password was supplied, and [`PdfError::WrongPassword`] when it does not open.
pub fn load_doc(input: &str, password: Option<&str>) -> Result<Document, PdfError> {
    let bytes = read_bytes(input)?;
    load_doc_bytes(&bytes, password)
}

/// Load a PDF straight from a path, decrypting with the global password.
///
/// This is the drop-in replacement for `Document::load(path)` in commands that
/// do not take their own `--password` argument: it upgrades the opaque
/// "decryption error" into the structured `PASSWORD_REQUIRED` / `WRONG_PASSWORD`
/// codes an agent can act on.
pub fn load_doc_path(path: &str) -> Result<Document, PdfError> {
    load_doc(path, None)
}

/// Load a PDF from an in-memory buffer (used after custom byte handling).
pub fn load_doc_bytes(bytes: &[u8], password: Option<&str>) -> Result<Document, PdfError> {
    let mut doc = Document::load_mem(bytes).map_err(|e| PdfError::ParseFailed {
        reason: e.to_string(),
    })?;
    if doc.is_encrypted() {
        match effective_password(password) {
            Some(pwd) => {
                doc.decrypt(pwd)
                    .map_err(|_| PdfError::WrongPassword {
                        path: "<memory>".into(),
                    })?;
            }
            None => return Err(PdfError::PasswordRequired),
        }
    }
    Ok(doc)
}

/// Parse a page spec such as `1-3,5,9-` into a 1-based, de-duplicated page list.
///
/// Unlike the legacy per-command parsers this is *strict*: a malformed token or a
/// page number outside `1..=total` is an error, never silently dropped.
pub fn parse_page_range(spec: Option<&str>, total: u32) -> Result<Vec<u32>, PdfError> {
    let spec = match spec {
        None => return Ok((1..=total).collect()),
        Some(s) if s.trim().is_empty() => return Ok((1..=total).collect()),
        Some(s) => s.trim(),
    };

    let mut pages: Vec<u32> = Vec::new();

    for raw in spec.split(',') {
        let part = raw.trim();
        if part.is_empty() {
            return Err(PdfError::InvalidPageRangeReason {
                spec: spec.to_string(),
                reason: "Empty segment (stray comma)".into(),
            });
        }

        let (start, end) = match part.split_once('-') {
            None => {
                let p = parse_page_num(part, spec)?;
                (p, p)
            }
            Some((a, b)) => {
                let a = a.trim();
                let b = b.trim();
                if a.is_empty() && b.is_empty() {
                    return Err(PdfError::InvalidPageRangeReason {
                        spec: spec.to_string(),
                        reason: "A range needs at least one bound, e.g. 3- or -5".into(),
                    });
                }
                let start = if a.is_empty() { 1 } else { parse_page_num(a, spec)? };
                let end = if b.is_empty() {
                    total
                } else {
                    parse_page_num(b, spec)?
                };
                (start, end)
            }
        };

        if start < 1 {
            return Err(PdfError::InvalidPageRangeReason {
                spec: spec.to_string(),
                reason: "Page numbers start at 1".into(),
            });
        }
        if start > end {
            return Err(PdfError::InvalidPageRangeReason {
                spec: spec.to_string(),
                reason: format!("Range {start}-{end} is reversed"),
            });
        }
        if end > total {
            return Err(PdfError::InvalidPageRangeReason {
                spec: spec.to_string(),
                reason: format!("Page {end} is out of range (document has {total} page(s))"),
            });
        }

        for p in start..=end {
            if !pages.contains(&p) {
                pages.push(p);
            }
        }
    }

    if pages.is_empty() {
        return Err(PdfError::InvalidPageRangeReason {
            spec: spec.to_string(),
            reason: "Range matched no pages".into(),
        });
    }

    Ok(pages)
}

fn parse_page_num(text: &str, spec: &str) -> Result<u32, PdfError> {
    text.parse::<u32>()
        .map_err(|_| PdfError::InvalidPageRangeReason {
            spec: spec.to_string(),
            reason: format!("\"{text}\" is not a page number"),
        })
}

/// Expand `--input` into a list of files.
///
/// Accepts a single file, a directory (all `*.pdf` directly inside it), or a glob
/// pattern such as `docs/**/*.pdf`. A literal path that does not exist yields
/// [`PdfError::FileNotFound`]; a pattern that matches nothing yields a clear error.
pub fn expand_inputs(input: &str) -> Result<Vec<PathBuf>, PdfError> {
    if is_stdio(input) {
        return Ok(vec![PathBuf::from(STDIO)]);
    }

    let path = Path::new(input);

    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    if path.is_dir() {
        let mut files: Vec<PathBuf> = walkdir::WalkDir::new(path)
            .max_depth(1)
            .sort_by_file_name()
            .into_iter()
            .filter_map(|e| e.ok())
            .map(|e| e.into_path())
            .filter(|p| p.is_file() && is_pdf_path(p))
            .collect();
        if files.is_empty() {
            return Err(PdfError::Operation {
                reason: format!("No PDF files found in directory: {input}"),
            });
        }
        files.shrink_to_fit();
        return Ok(files);
    }

    if looks_like_glob(input) {
        let mut files: Vec<PathBuf> = glob::glob(input)
            .map_err(|e| PdfError::Operation {
                reason: format!("Invalid glob pattern \"{input}\": {e}"),
            })?
            .filter_map(|r| r.ok())
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        if files.is_empty() {
            return Err(PdfError::Operation {
                reason: format!("Glob pattern matched no files: {input}"),
            });
        }
        return Ok(files);
    }

    Err(PdfError::FileNotFound {
        path: input.to_string(),
    })
}

/// Render a resolved path back into the `&str` form the command runners accept.
///
/// Batch expansion hands commands a `PathBuf`, but every runner takes `&str`
/// (so that `-` for stdio and glob-looking strings stay representable). This
/// keeps that conversion explicit and in one place. A path that is not valid
/// UTF-8 falls back to its lossy form rather than failing: the bytes still name
/// the same file on every platform we support, and the caller only opens it.
pub fn path_to_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Public wrapper over single-input resolution, for callers that want to report
/// which concrete file an argument (file / directory / glob) actually named.
pub fn resolve_input_once(input: &str) -> Result<PathBuf, PdfError> {
    resolve_single_input(input)
}

fn looks_like_glob(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('[')
}

/// Split a `--input` value into the individual documents a multi-file command
/// should process.
///
/// Unlike [`expand_inputs`] (which answers "what does this one path refer to"),
/// this is the entry point for commands that genuinely accept a batch: it
/// combines three syntaxes that agents reach for interchangeably —
///
/// * a comma-separated list: `a.pdf,b.pdf`
/// * a directory: every `*.pdf` directly inside it
/// * a glob: `docs/*.pdf`
///
/// A plain single file yields a one-element vector. Repeated/duplicate paths are
/// collapsed while preserving first-seen order, so `merge --input a.pdf,a.pdf`
/// still merges two copies (callers that care about duplicates should not use
/// this) — resolution here is about *locating* inputs, not de-duplicating them.
pub fn expand_batch(input: &str) -> Result<Vec<PathBuf>, PdfError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(PdfError::MissingArg {
            arg: "input".into(),
            example: "pdf-cli merge --input a.pdf,b.pdf --output merged.pdf".into(),
        });
    }

    // A single path that happens to resolve (file / dir / glob) is one batch token.
    let mut out: Vec<PathBuf> = Vec::new();
    for token in trimmed.split(',') {
        let token = token.trim();
        if token.is_empty() {
            return Err(PdfError::MissingArg {
                arg: "input".into(),
                example: "pdf-cli merge --input a.pdf,b.pdf --output merged.pdf".into(),
            });
        }
        out.extend(expand_inputs(token)?);
    }

    if out.is_empty() {
        return Err(PdfError::Operation {
            reason: format!("No PDF files matched: {input}"),
        });
    }
    Ok(out)
}

fn is_pdf_path(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

/// Bind the pdfium shared library, honouring `--pdfium-path` and falling back to
/// the system search path. The error message tells an agent exactly how to fix it.
pub fn bind_pdfium(
    pdfium_path: Option<&str>,
) -> Result<pdfium_render::prelude::Pdfium, PdfError> {
    let bindings = match pdfium_path {
        Some(p) => pdfium_render::prelude::Pdfium::bind_to_library(p),
        None => pdfium_render::prelude::Pdfium::bind_to_system_library(),
    }
    .map_err(|e| PdfError::PdfiumNotFound {
        reason: e.to_string(),
    })?;

    Ok(pdfium_render::prelude::Pdfium::new(bindings))
}
