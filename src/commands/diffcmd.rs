//! `diff` command: compare two PDFs by normalized text and/or rendered pixels.
//!
//! Two independent comparison strategies are available:
//!
//! * `text` — extracts the text layer page by page, normalizes it (whitespace
//!   runs collapse to a single space so PDF line-wrap artifacts do not count as
//!   changes) and reports added/removed fragments from a word-level LCS diff.
//! * `render` — rasterizes the requested pages at a fixed width and reports the
//!   fraction of pixels that differ, optionally writing a per-page mask image.
//! * `both` — runs `text` first and only falls back to `render` for pages whose
//!   text compares equal, so image-only edits are still caught cheaply.
//!
//! The command always succeeds (exit code 0) when both inputs are readable; the
//! caller is expected to read `details.different`.

use crate::error::PdfError;
use crate::io;
use crate::output::CommandResult;

pub struct DiffArgs<'a> {
    pub input: &'a str,
    pub input2: &'a str,
    pub mode: &'a str,
    pub threshold: f32,
    pub output: Option<&'a str>,
    pub pages: Option<&'a str>,
    pub width: u32,
    pub password: Option<&'a str>,
    pub pdfium_path: Option<&'a str>,
}

/// Maximum number of fragments reported per page, per direction.
const MAX_FRAGMENTS: usize = 20;
/// Maximum number of characters kept from a single reported fragment.
const MAX_FRAGMENT_CHARS: usize = 200;
/// Words per comparison chunk.
///
/// PDF text extraction inserts newlines wherever the renderer wrapped a line, so
/// a real change shifts every following newline. Comparing word windows instead
/// of raw lines keeps the diff readable without depending on the layout.
const CHUNK_WORDS: usize = 20;
/// Hard cap on the number of chunks fed to the LCS table (bounds memory).
const MAX_DIFF_CHUNKS: usize = 1000;
/// Per-channel difference (0-255) that still counts as "identical" in render mode.
const CHANNEL_TOLERANCE: u8 = 16;

/// The comparison outcome for a single page.
#[derive(serde::Serialize)]
struct PageDiff {
    page: u32,
    status: &'static str,
    /// `null` when the page was decided by text, the pixel fraction otherwise.
    changed_ratio: Option<f64>,
    added: Vec<String>,
    removed: Vec<String>,
}

/// One page rasterized to tightly packed 8-bit RGB.
struct RenderedPage {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

impl RenderedPage {
    fn pixel(&self, x: u32, y: u32) -> Option<[u8; 3]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let idx = (y as usize * self.width as usize + x as usize) * 3;
        let px = self.rgb.get(idx..idx + 3)?;
        Some([px[0], px[1], px[2]])
    }
}

/// Compare two PDFs and report what changed.
///
/// `mode` is `text`, `render` or `both` (case-insensitive). The returned
/// [`CommandResult`] always succeeds when both documents are readable; the
/// caller inspects `details.different` instead of the exit code. `args.output`,
/// when set, receives `diff_p{page:03}.png` mask images for render-compared
/// pages (white = identical, red = different). `dry_run` performs the whole
/// comparison but writes no masks and creates no directory.
pub fn run(args: DiffArgs<'_>, dry_run: bool) -> Result<CommandResult, PdfError> {
    let mode = args.mode.trim().to_ascii_lowercase();
    if !matches!(mode.as_str(), "text" | "render" | "both") {
        return Err(PdfError::Operation {
            reason: format!(
                "Invalid mode '{}'. Valid modes: text, render, both.",
                args.mode
            ),
        });
    }

    if io::is_stdio(args.input) && io::is_stdio(args.input2) {
        return Err(PdfError::Operation {
            reason: "Only one of --input and --input2 can be '-' (stdin)".into(),
        });
    }

    if mode != "text" && args.width == 0 {
        return Err(PdfError::Operation {
            reason: "--width must be greater than 0".into(),
        });
    }

    let bytes1 = io::read_bytes(args.input)?;
    let bytes2 = io::read_bytes(args.input2)?;

    // Structural validation first: this is where PASSWORD_REQUIRED,
    // WRONG_PASSWORD and PARSE_FAILED surface, before the expensive work.
    let doc1 = io::load_doc_bytes(&bytes1, args.password)?;
    let doc2 = io::load_doc_bytes(&bytes2, args.password)?;
    let total1 = doc1.get_pages().len() as u32;
    let total2 = doc2.get_pages().len() as u32;

    let pages = io::parse_page_range(args.pages, total1.max(total2))?;
    if pages.is_empty() {
        return Err(PdfError::InvalidPageRangeReason {
            spec: args.pages.unwrap_or("(all)").to_string(),
            reason: "No pages to compare".into(),
        });
    }

    let mut page_diffs: Vec<PageDiff> = Vec::new();
    // Pages the text pass left equal and that still need a pixel comparison.
    let mut deferred: Vec<u32> = Vec::new();

    if mode != "render" {
        let texts1 = extract_texts(&doc1, total1)?;
        let texts2 = extract_texts(&doc2, total2)?;

        for &page in &pages {
            let idx = (page - 1) as usize;
            let left = texts1.get(idx);
            let right = texts2.get(idx);

            match (left, right) {
                (None, Some(right)) => page_diffs.push(PageDiff {
                    page,
                    status: "added",
                    changed_ratio: None,
                    added: fragments(right),
                    removed: Vec::new(),
                }),
                (Some(left), None) => page_diffs.push(PageDiff {
                    page,
                    status: "removed",
                    changed_ratio: None,
                    added: Vec::new(),
                    removed: fragments(left),
                }),
                (Some(left), Some(right)) => {
                    if left == right {
                        deferred.push(page);
                    } else {
                        let (added, removed) = diff_chunks(&chunk_words(left), &chunk_words(right));
                        page_diffs.push(PageDiff {
                            page,
                            status: "changed",
                            changed_ratio: None,
                            added,
                            removed,
                        });
                    }
                }
                // Not reachable: the page list is clamped to the larger page
                // count, so at least one side always has the page.
                (None, None) => {}
            }
        }
    }

    let mut masks: Vec<String> = Vec::new();

    if mode != "text" {
        let render_list: Vec<u32> = if mode == "render" {
            pages.clone()
        } else {
            deferred.clone()
        };

        if !render_list.is_empty() {
            let pdfium = io::bind_pdfium(args.pdfium_path)?;
            let rendered1: Vec<Option<RenderedPage>> =
                render_pages(&pdfium, &bytes1, args.password, &render_list, args.width)?;
            let rendered2: Vec<Option<RenderedPage>> =
                render_pages(&pdfium, &bytes2, args.password, &render_list, args.width)?;

            let mask_dir: Option<std::path::PathBuf> = match args.output {
                Some(dir) if !dry_run => {
                    std::fs::create_dir_all(dir).map_err(|e| PdfError::Operation {
                        reason: format!("Failed to create output directory {dir}: {e}"),
                    })?;
                    Some(std::path::PathBuf::from(dir))
                }
                _ => None,
            };

            for (slot, &page) in render_list.iter().enumerate() {
                let left = rendered1.get(slot).and_then(|p| p.as_ref());
                let right = rendered2.get(slot).and_then(|p| p.as_ref());

                match (left, right) {
                    (None, Some(_)) => page_diffs.push(PageDiff {
                        page,
                        status: "added",
                        changed_ratio: Some(1.0),
                        added: Vec::new(),
                        removed: Vec::new(),
                    }),
                    (Some(_), None) => page_diffs.push(PageDiff {
                        page,
                        status: "removed",
                        changed_ratio: Some(1.0),
                        added: Vec::new(),
                        removed: Vec::new(),
                    }),
                    (Some(left), Some(right)) => {
                        let want_mask = mask_dir.is_some();
                        let (ratio, mask) = compare_pixels(left, right, want_mask);
                        if ratio > f64::from(args.threshold) {
                            if let (Some(dir), Some(buffer)) = (mask_dir.as_ref(), mask) {
                                let path = dir.join(format!("diff_p{page:03}.png"));
                                write_mask(&path, left, right, buffer)?;
                                masks.push(path.to_string_lossy().into_owned());
                            }
                            page_diffs.push(PageDiff {
                                page,
                                status: "changed",
                                changed_ratio: Some(ratio),
                                added: Vec::new(),
                                removed: Vec::new(),
                            });
                        }
                    }
                    (None, None) => {}
                }
            }
        }
    }

    page_diffs.sort_by_key(|d| d.page);

    let compared = pages.len() as u32;
    let changed = count_status(&page_diffs, "changed");
    let added = count_status(&page_diffs, "added");
    let removed = count_status(&page_diffs, "removed");
    let different = !page_diffs.is_empty();
    let unchanged = compared.saturating_sub(changed + added + removed);

    let differing: Vec<u32> = page_diffs.iter().map(|d| d.page).collect();
    let summary = build_summary(compared, changed, added, removed, &differing);

    let mut details = serde_json::json!({
        "input": input_label(args.input),
        "input2": input_label(args.input2),
        "mode": mode,
        "different": different,
        "pages_compared": compared,
        "pages_changed": changed,
        "pages_added": added,
        "pages_removed": removed,
        "unchanged_pages": unchanged,
        "threshold": f64::from(args.threshold),
        "pages": page_diffs,
        "masks": masks,
    });

    if dry_run {
        if let Some(obj) = details.as_object_mut() {
            obj.insert("dry_run".into(), serde_json::json!(true));
        }
    }

    Ok(CommandResult::ok("diff", Some(summary), details))
}

/// `-` is reported as `stdin` so the JSON stays readable for an agent.
fn input_label(input: &str) -> String {
    if io::is_stdio(input) {
        "stdin".to_string()
    } else {
        input.to_string()
    }
}

fn count_status(diffs: &[PageDiff], status: &str) -> u32 {
    diffs.iter().filter(|d| d.status == status).count() as u32
}

/// One-line human summary, e.g. `1 of 3 pages changed (page 2)`.
fn build_summary(
    compared: u32,
    changed: u32,
    added: u32,
    removed: u32,
    differing: &[u32],
) -> String {
    let total = changed + added + removed;
    if total == 0 {
        let noun = if compared == 1 { "page" } else { "pages" };
        return format!("identical ({compared} {noun})");
    }

    let verb = if added + removed == 0 {
        "changed"
    } else {
        "differ"
    };
    let mut out = format!("{total} of {compared} pages {verb}");

    if !differing.is_empty() {
        let noun = if differing.len() == 1 { "page" } else { "pages" };
        let list: Vec<String> = differing.iter().map(|p| p.to_string()).collect();
        out.push_str(&format!(" ({noun} {})", list.join(", ")));
    }

    let mut notes = Vec::new();
    if added > 0 {
        notes.push(format!("{added} added"));
    }
    if removed > 0 {
        notes.push(format!("{removed} removed"));
    }
    if !notes.is_empty() {
        out.push_str(&format!("; {}", notes.join(", ")));
    }

    out
}

// ---------------------------------------------------------------------------
// Text comparison
// ---------------------------------------------------------------------------

/// Extract one normalized string per page.
///
/// Uses the project's own `lopdf` document rather than `pdf-extract`'s
/// whole-document helpers: those emit no page-separator character, so splitting
/// their output would have to guess page boundaries. `extract_text(&[n])` gives
/// exactly one page, matching how `extract` reports page text.
fn extract_texts(doc: &lopdf::Document, total: u32) -> Result<Vec<String>, PdfError> {
    let mut out: Vec<String> = Vec::with_capacity(total as usize);
    for page in 1..=total {
        let text = doc
            .extract_text(&[page])
            .map_err(|e| PdfError::Operation {
                reason: format!("Failed to extract text from page {page}: {e}"),
            })?;
        out.push(normalize_text(&text));
    }
    Ok(out)
}

fn usable_password(password: Option<&str>) -> Option<&str> {
    password.filter(|p| !p.is_empty())
}

/// Collapse every whitespace run to one space and trim, leaving case intact.
///
/// Extracted PDF text breaks lines at the same column the renderer wrapped, so
/// two visually identical pages often differ in newline placement. Normalizing
/// whitespace removes those artifacts; case is *not* folded because a real
/// capitalization change is a real change.
fn normalize_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn chunk_words(text: &str) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    words
        .chunks(CHUNK_WORDS)
        .map(|chunk| chunk.join(" "))
        .collect()
}

/// Every chunk of `text`, trimmed to the report limit.
fn fragments(text: &str) -> Vec<String> {
    chunk_words(text)
        .iter()
        .take(MAX_FRAGMENTS)
        .map(|s| truncate_fragment(s))
        .collect()
}

fn truncate_fragment(fragment: &str) -> String {
    if fragment.chars().count() <= MAX_FRAGMENT_CHARS {
        return fragment.to_string();
    }
    let mut out: String = fragment.chars().take(MAX_FRAGMENT_CHARS).collect();
    out.push('…');
    out
}

/// Word-window LCS diff. Returns `(added, removed)` fragments in document order.
fn diff_chunks(old: &[String], new: &[String]) -> (Vec<String>, Vec<String>) {
    let n = old.len().min(MAX_DIFF_CHUNKS);
    let m = new.len().min(MAX_DIFF_CHUNKS);
    let stride = m + 1;

    // lcs[i * stride + j] = LCS length of old[..i] and new[..j]
    let mut lcs = vec![0u32; (n + 1) * stride];
    for i in 1..=n {
        for j in 1..=m {
            lcs[i * stride + j] = if old[i - 1] == new[j - 1] {
                lcs[(i - 1) * stride + (j - 1)] + 1
            } else {
                lcs[(i - 1) * stride + j].max(lcs[i * stride + (j - 1)])
            };
        }
    }

    let mut added: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    let (mut i, mut j) = (n, m);

    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old[i - 1] == new[j - 1] {
            i -= 1;
            j -= 1;
        } else if i > 0 && (j == 0 || lcs[(i - 1) * stride + j] >= lcs[i * stride + (j - 1)]) {
            removed.push(old[i - 1].clone());
            i -= 1;
        } else {
            added.push(new[j - 1].clone());
            j -= 1;
        }
    }

    added.reverse();
    removed.reverse();
    added.truncate(MAX_FRAGMENTS);
    removed.truncate(MAX_FRAGMENTS);

    (added, removed)
}

// ---------------------------------------------------------------------------
// Render comparison
// ---------------------------------------------------------------------------

/// Rasterize `pages` of one document. `None` marks a page the document lacks.
fn render_pages(
    pdfium: &pdfium_render::prelude::Pdfium,
    bytes: &[u8],
    password: Option<&str>,
    pages: &[u32],
    width: u32,
) -> Result<Vec<Option<RenderedPage>>, PdfError> {
    use pdfium_render::prelude::*;

    let doc = pdfium
        .load_pdf_from_byte_slice(bytes, usable_password(password))
        .map_err(|e| PdfError::ParseFailed {
            reason: format!("pdfium could not open the document: {e}"),
        })?;

    let total = doc.pages().len() as u32;
    let config = PdfRenderConfig::new()
        .set_target_width(width as i32)
        .set_format(PdfBitmapFormat::BGR);

    let mut out: Vec<Option<RenderedPage>> = Vec::with_capacity(pages.len());

    for &page in pages {
        if page > total {
            out.push(None);
            continue;
        }
        let handle = doc
            .pages()
            .get((page - 1) as u16)
            .map_err(|e| PdfError::Operation {
                reason: format!("Cannot open page {page}: {e}"),
            })?;
        let bitmap = handle
            .render_with_config(&config)
            .map_err(|e| PdfError::Operation {
                reason: format!("Cannot render page {page}: {e}"),
            })?;
        let image = bitmap.as_image().to_rgb8();
        let (w, h) = image.dimensions();
        out.push(Some(RenderedPage {
            width: w,
            height: h,
            rgb: image.into_raw(),
        }));
    }

    Ok(out)
}

/// Fraction of differing pixels plus, optionally, an RGB mask buffer.
///
/// A pixel differs when any channel deviates by more than [`CHANNEL_TOLERANCE`],
/// which ignores rasterizer anti-aliasing noise. Pages of different sizes are
/// compared over the union of both areas; every pixel that exists in only one
/// page counts as different.
fn compare_pixels(
    left: &RenderedPage,
    right: &RenderedPage,
    want_mask: bool,
) -> (f64, Option<Vec<u8>>) {
    let width = left.width.max(right.width);
    let height = left.height.max(right.height);
    let total = u64::from(width) * u64::from(height);

    let mut mask = if want_mask {
        Some(vec![255u8; width as usize * height as usize * 3])
    } else {
        None
    };

    let mut differing: u64 = 0;

    for y in 0..height {
        for x in 0..width {
            let differs = match (left.pixel(x, y), right.pixel(x, y)) {
                (Some(a), Some(b)) => {
                    a[0].abs_diff(b[0]) > CHANNEL_TOLERANCE
                        || a[1].abs_diff(b[1]) > CHANNEL_TOLERANCE
                        || a[2].abs_diff(b[2]) > CHANNEL_TOLERANCE
                }
                _ => true,
            };

            if differs {
                differing += 1;
                if let Some(buf) = mask.as_mut() {
                    let idx = (y as usize * width as usize + x as usize) * 3;
                    buf[idx] = 255;
                    buf[idx + 1] = 0;
                    buf[idx + 2] = 0;
                }
            }
        }
    }

    let ratio = if total == 0 {
        0.0
    } else {
        differing as f64 / total as f64
    };
    // Round to 6 decimals so the JSON stays stable and readable.
    let ratio = (ratio * 1_000_000.0).round() / 1_000_000.0;

    (ratio, mask)
}

/// Save a white/red mask PNG. The buffer is sized for the union of both pages.
fn write_mask(
    path: &std::path::Path,
    left: &RenderedPage,
    right: &RenderedPage,
    buffer: Vec<u8>,
) -> Result<(), PdfError> {
    let width = left.width.max(right.width);
    let height = left.height.max(right.height);

    let image = image::ImageBuffer::<image::Rgb<u8>, Vec<u8>>::from_raw(width, height, buffer)
        .ok_or_else(|| PdfError::Operation {
            reason: format!("Mask buffer size does not match {}x{}", width, height),
        })?;

    image
        .save(path)
        .map_err(|e| PdfError::Operation {
            reason: format!("Failed to write {}: {e}", path.display()),
        })
}
