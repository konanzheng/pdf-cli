//! `crop` — trim page margins by setting each selected page's `/CropBox`.
//!
//! Two mutually exclusive modes:
//!
//! * `--box "left,bottom,right,top"` — an explicit rectangle in PDF points, with
//!   the origin at the **bottom-left** corner of the page (PDF user space, *not*
//!   image space, so a box that trims the top margin uses a *small* `top` value).
//! * `--auto` — render each page with pdfium, find the ink bounding box, expand it
//!   by `--margin` points and use that as the crop box. Pages with no detectable
//!   ink are reported in `blank_pages` and left untouched.
//!
//! The operation is deliberately **non-destructive**: only `/CropBox` is written.
//! `/MediaBox` — and therefore every content stream, image and glyph outside the
//! new crop — stays byte-for-byte in the file, so the trim can be undone later by
//! widening the crop box again (or by running `pdf-cli crop --box "<original
//! media box>"`). Consumers that honour `/CropBox` (every mainstream viewer and
//! printer) see the trimmed page; consumers that do not still see a valid page.
//!
//! Validation is strict rather than silent: an inverted or non-positive box, a
//! box that misses the page entirely, or a non-numeric value is an error naming
//! the offender. Emitting a page whose crop box is empty would produce a valid
//! PDF with nothing on it, which is far harder to debug than an error message.

use crate::error::PdfError;
use crate::io;
use crate::output::CommandResult;
use lopdf::{Document, Object, ObjectId};
use serde_json::json;
use std::path::Path;

pub struct CropArgs<'a> {
    pub input: &'a str,
    pub output: &'a str,
    pub box_spec: Option<&'a str>,
    pub auto: bool,
    pub margin: f32,
    pub pages: Option<&'a str>,
    pub password: Option<&'a str>,
    /// Explicit pdfium.dll location; when `None`, the usual places are probed.
    pub pdfium_path: Option<&'a str>,
}

/// Human-readable description of the `--box` format, reused in every error.
const BOX_FORMAT: &str =
    "four numbers 'left,bottom,right,top' in PDF points with the origin at the bottom-left corner";
const BOX_EXAMPLE: &str = "--box \"50,700,545,800\"";

/// Fallback page size (A4) when a page carries no usable `/MediaBox` anywhere in
/// its ancestry. Matches the fallback used by `pagenum`.
const FALLBACK_MEDIA: (f32, f32) = (595.28, 841.89);

/// A rectangle in PDF user space, always normalised so that
/// `left <= right` and `bottom <= top`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Box4 {
    left: f32,
    bottom: f32,
    right: f32,
    top: f32,
}

impl Box4 {
    fn width(&self) -> f32 {
        self.right - self.left
    }

    fn height(&self) -> f32 {
        self.top - self.bottom
    }

    /// Overlap of two rectangles, or `None` when they do not intersect.
    fn intersect(&self, other: &Box4) -> Option<Box4> {
        let left = self.left.max(other.left);
        let bottom = self.bottom.max(other.bottom);
        let right = self.right.min(other.right);
        let top = self.top.min(other.top);
        if right > left && top > bottom {
            Some(Box4 {
                left,
                bottom,
                right,
                top,
            })
        } else {
            None
        }
    }

    fn as_object(&self) -> Object {
        // Always `Real` so the four numbers are written in one consistent form.
        Object::Array(vec![
            Object::Real(self.left),
            Object::Real(self.bottom),
            Object::Real(self.right),
            Object::Real(self.top),
        ])
    }

    fn describe(&self) -> String {
        format!(
            "[{}, {}, {}, {}]",
            r2(self.left),
            r2(self.bottom),
            r2(self.right),
            r2(self.top)
        )
    }
}

/// One page that will be (or would be) cropped.
struct PageCrop {
    page: u32,
    page_id: ObjectId,
    original: Box4,
    crop: Box4,
}

pub fn run(args: CropArgs<'_>, dry_run: bool) -> Result<CommandResult, PdfError> {
    // --- flag validation: exactly one mode -------------------------------
    match (args.box_spec, args.auto) {
        (Some(_), true) => Err(PdfError::Operation {
            reason: format!(
                "Ambiguous crop mode: both --box and --auto were given. Pass exactly one of \
                 them — {BOX_EXAMPLE} for an explicit rectangle, or --auto --margin 10 to \
                 crop to the detected content."
            ),
        }),
        (None, false) => Err(PdfError::MissingArg {
            arg: "--box or --auto".into(),
            example: format!(
                "pdf-cli crop --input in.pdf --output out.pdf {BOX_EXAMPLE}\n  \
                 or: pdf-cli crop --input in.pdf --output out.pdf --auto --margin 10"
            ),
        }),
        // The box string is parsed before the document is opened, so a typo in
        // the flag never costs a parse.
        (Some(spec), false) => run_box(args, spec, dry_run),
        (None, true) => run_auto(args, dry_run),
    }
}

/// `--box` mode: one rectangle applied to every selected page.
fn run_box(args: CropArgs<'_>, spec: &str, dry_run: bool) -> Result<CommandResult, PdfError> {
    let requested = parse_box(spec)?;
    let mut doc = io::load_doc(args.input, args.password)?;
    let page_map = doc.get_pages();
    let selected = select_pages(&page_map, args.pages)?;

    let mut crops: Vec<PageCrop> = Vec::with_capacity(selected.len());
    for page in selected {
        let page_id = page_id_of(&page_map, page)?;
        let original = visible_box(&doc, page_id, page);
        let media = media_box(&doc, page_id);

        // The crop box must land somewhere on the page. A box that misses it
        // entirely would render an empty page, so that is an error, not a blank.
        let crop = match requested.intersect(&media) {
            Some(clamped) => clamped,
            None => {
                return Err(PdfError::Operation {
                    reason: format!(
                        "--box \"{spec}\" does not overlap page {page}: the requested box {} \
                         lies outside that page's media box {} ({} x {} points).",
                        requested.describe(),
                        media.describe(),
                        r2(media.width()),
                        r2(media.height())
                    ),
                })
            }
        };

        crops.push(PageCrop {
            page,
            page_id,
            original,
            crop,
        });
    }

    finish(&mut doc, &args, crops, "box", Vec::new(), dry_run)
}

/// `--auto` mode: render each page and crop to the ink bounding box plus margin.
fn run_auto(args: CropArgs<'_>, dry_run: bool) -> Result<CommandResult, PdfError> {
    if args.margin < 0.0 || !args.margin.is_finite() {
        return Err(PdfError::Operation {
            reason: format!(
                "--margin must be a zero or positive number of points, got {}",
                args.margin
            ),
        });
    }

    let bytes = io::read_bytes(args.input)?;
    let mut doc = io::load_doc_bytes(&bytes, args.password)?;
    let page_map = doc.get_pages();
    let selected = select_pages(&page_map, args.pages)?;

    let pdfium = bind_pdfium(args.pdfium_path)?;
    let rendered = pdfium
        .load_pdf_from_byte_vec(bytes, args.password)
        .map_err(|e| PdfError::ParseFailed {
            reason: format!("pdfium could not open {}: {e}", args.input),
        })?;
    let rendered_pages = rendered.pages();

    let mut crops: Vec<PageCrop> = Vec::with_capacity(selected.len());
    let mut blank_pages: Vec<u32> = Vec::new();

    for page in selected {
        let page_id = page_id_of(&page_map, page)?;
        let original = visible_box(&doc, page_id, page);
        let media = media_box(&doc, page_id);
        let rotation = page_rotation(&doc, page_id);

        let index = (page - 1) as u16;
        let pdfium_page = rendered_pages
            .get(index)
            .map_err(|e| PdfError::Operation {
                reason: format!("pdfium could not read page {page}: {e}"),
            })?;

        // Render at ~2 px per point: enough resolution to detect a hairline rule
        // without turning a 500-page document into a multi-gigabyte scan.
        let (display_w, _display_h) = display_size(&original, rotation);
        let target_width = (display_w * 2.0).round().clamp(400.0, 2400.0) as i32;
        let config = pdfium_render::prelude::PdfRenderConfig::new()
            .set_target_width(target_width);

        let bitmap = pdfium_page
            .render_with_config(&config)
            .map_err(|e| PdfError::Operation {
                reason: format!("failed to render page {page}: {e}"),
            })?;

        let width_px = bitmap.width() as u32;
        let height_px = bitmap.height() as u32;
        let rgba = bitmap.as_rgba_bytes();

        let ink = match ink_bounds(&rgba, width_px, height_px) {
            Some(bounds) => bounds,
            None => {
                blank_pages.push(page);
                continue;
            }
        };

        let mapping = Mapping {
            base: original,
            rotation,
            width_px: width_px as f32,
            height_px: height_px as f32,
        };

        // Pixel edges, not pixel centres: the ink covers [min, max + 1) pixels.
        let (x1, y1) = mapping.to_user(ink.0 as f32, ink.1 as f32);
        let (x2, y2) = mapping.to_user(ink.2 as f32 + 1.0, ink.3 as f32 + 1.0);
        let detected = Box4 {
            left: x1.min(x2),
            bottom: y1.min(y2),
            right: x1.max(x2),
            top: y1.max(y2),
        };

        let padded = Box4 {
            left: detected.left - args.margin,
            bottom: detected.bottom - args.margin,
            right: detected.right + args.margin,
            top: detected.top + args.margin,
        };

        // Never leave the media box, and never produce an empty crop box.
        let crop = match padded.intersect(&media) {
            Some(clamped) if clamped.width() > 0.0 && clamped.height() > 0.0 => clamped,
            _ => {
                blank_pages.push(page);
                continue;
            }
        };

        crops.push(PageCrop {
            page,
            page_id,
            original,
            crop,
        });
    }

    // A document whose pages are all blank is not an error: every page is simply
    // reported in `blank_pages` and the output is an unmodified copy.
    finish(&mut doc, &args, crops, "auto", blank_pages, dry_run)
}

/// Write `/CropBox` for every crop (unless drying out) and build the result.
fn finish(
    doc: &mut Document,
    args: &CropArgs<'_>,
    crops: Vec<PageCrop>,
    mode: &str,
    blank_pages: Vec<u32>,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    if !dry_run {
        for crop in &crops {
            let dict = doc
                .get_dictionary_mut(crop.page_id)
                .map_err(|e| PdfError::Operation {
                    reason: format!("page {} has no page dictionary: {e}", crop.page),
                })?;
            dict.set("CropBox", crop.crop.as_object());
        }

        if io::is_stdio(args.output) {
            let mut buffer = Vec::new();
            doc.save_to(&mut buffer)
                .map_err(|e| PdfError::Operation {
                    reason: format!("failed to serialise the cropped PDF: {e}"),
                })?;
            io::write_bytes(args.output, &buffer)?;
        } else {
            if let Some(parent) = Path::new(args.output).parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).map_err(|e: std::io::Error| {
                        PdfError::Operation {
                            reason: format!(
                                "failed to create output directory {}: {e}",
                                parent.display()
                            ),
                        }
                    })?;
                }
            }
            doc.save(args.output)
                .map_err(|e: std::io::Error| PdfError::Operation {
                    reason: format!("failed to write {}: {e}", args.output),
                })?;
        }
    }

    let pages: Vec<serde_json::Value> = crops
        .iter()
        .map(|crop| {
            json!({
                "page": crop.page,
                "crop_box": {
                    "left": r2(crop.crop.left),
                    "bottom": r2(crop.crop.bottom),
                    "right": r2(crop.crop.right),
                    "top": r2(crop.crop.top),
                },
                "original_size": {
                    "width": r2(crop.original.width()),
                    "height": r2(crop.original.height()),
                },
                "cropped_size": {
                    "width": r2(crop.crop.width()),
                    "height": r2(crop.crop.height()),
                },
            })
        })
        .collect();

    let mut details = json!({
        "input": args.input,
        "output": args.output,
        "mode": mode,
        "margin": r2(args.margin),
        "pages_affected": crops.len(),
        "blank_pages": blank_pages,
        "pages": pages,
    });

    if dry_run {
        details["dry_run"] = json!(true);
    }

    Ok(CommandResult::ok(
        "crop",
        Some(args.output.to_string()),
        details,
    ))
}

// ---------------------------------------------------------------------------
// argument parsing
// ---------------------------------------------------------------------------

/// Parse `"left,bottom,right,top"` into a validated, normalised [`Box4`].
fn parse_box(spec: &str) -> Result<Box4, PdfError> {
    let parts: Vec<&str> = spec.split(',').collect();
    if parts.len() != 4 {
        return Err(PdfError::Operation {
            reason: format!(
                "Invalid --box \"{spec}\": expected exactly 4 comma-separated values \
                 ({BOX_FORMAT}), got {}. Example: {BOX_EXAMPLE}",
                parts.len()
            ),
        });
    }

    let mut values = [0f32; 4];
    let names = ["left", "bottom", "right", "top"];
    for (index, raw) in parts.iter().enumerate() {
        let text = raw.trim();
        let value: f32 = text.parse().map_err(|_| PdfError::Operation {
            reason: format!(
                "Invalid --box \"{spec}\": value {} for {} is not a number (\"{}\"). \
                 Expected {BOX_FORMAT}. Example: {BOX_EXAMPLE}",
                index + 1,
                names[index],
                text
            ),
        })?;
        if !value.is_finite() {
            return Err(PdfError::Operation {
                reason: format!(
                    "Invalid --box \"{spec}\": {} must be a finite number, got \"{}\". \
                     Example: {BOX_EXAMPLE}",
                    names[index], text
                ),
            });
        }
        values[index] = value;
    }

    let [left, bottom, right, top] = values;

    if left < 0.0 || bottom < 0.0 {
        return Err(PdfError::Operation {
            reason: format!(
                "Invalid --box \"{spec}\": left ({}) and bottom ({}) must not be negative — \
                 the origin is the bottom-left corner of the page. Example: {BOX_EXAMPLE}",
                r2(left),
                r2(bottom)
            ),
        });
    }
    if left >= right {
        return Err(PdfError::Operation {
            reason: format!(
                "Invalid --box \"{spec}\": left ({}) must be less than right ({}). \
                 The order is left,bottom,right,top. Example: {BOX_EXAMPLE}",
                r2(left),
                r2(right)
            ),
        });
    }
    if bottom >= top {
        return Err(PdfError::Operation {
            reason: format!(
                "Invalid --box \"{spec}\": bottom ({}) must be less than top ({}). \
                 The order is left,bottom,right,top. Example: {BOX_EXAMPLE}",
                r2(bottom),
                r2(top)
            ),
        });
    }

    Ok(Box4 {
        left,
        bottom,
        right,
        top,
    })
}

/// Resolve `--pages` against the document, sorted ascending for stable output.
fn select_pages(
    page_map: &std::collections::BTreeMap<u32, ObjectId>,
    spec: Option<&str>,
) -> Result<Vec<u32>, PdfError> {
    let total = page_map.len() as u32;
    let mut pages = io::parse_page_range(spec, total)?;
    pages.sort_unstable();
    pages.dedup();
    Ok(pages)
}

fn page_id_of(
    page_map: &std::collections::BTreeMap<u32, ObjectId>,
    page: u32,
) -> Result<ObjectId, PdfError> {
    page_map
        .get(&page)
        .copied()
        .ok_or_else(|| PdfError::Operation {
            reason: format!("page {page} not found in the document"),
        })
}

// ---------------------------------------------------------------------------
// page box resolution
// ---------------------------------------------------------------------------

/// `(width, height)` of a rectangle, defaulting to A4 when nothing is inherited.
fn media_box(doc: &Document, page_id: ObjectId) -> Box4 {
    lookup_box(doc, page_id, b"MediaBox").unwrap_or(Box4 {
        left: 0.0,
        bottom: 0.0,
        right: FALLBACK_MEDIA.0,
        top: FALLBACK_MEDIA.1,
    })
}

/// The box a viewer actually shows: the existing `/CropBox` where present,
/// otherwise the `/MediaBox`. Used for the `original_size` report and as the
/// reference frame when mapping rendered pixels back to PDF points.
fn visible_box(doc: &Document, page_id: ObjectId, page: u32) -> Box4 {
    match lookup_box(doc, page_id, b"CropBox") {
        Some(crop) if crop.width() > 0.0 && crop.height() > 0.0 => crop,
        _ => {
            let media = media_box(doc, page_id);
            if media.width() > 0.0 && media.height() > 0.0 {
                media
            } else {
                let _ = page;
                Box4 {
                    left: 0.0,
                    bottom: 0.0,
                    right: FALLBACK_MEDIA.0,
                    top: FALLBACK_MEDIA.1,
                }
            }
        }
    }
}

/// Look `key` up on the page, then on its `/Parent` chain (the page tree inherits
/// `/MediaBox`, `/CropBox` and `/Rotate`), stopping after a bounded number of hops
/// so a malformed cyclic tree cannot hang the process.
fn lookup_box(doc: &Document, page_id: ObjectId, key: &[u8]) -> Option<Box4> {
    let mut current = page_id;
    for _ in 0..32 {
        let dict = doc.get_dictionary(current).ok()?;
        if let Ok(object) = dict.get(key) {
            if let Some(found) = box_from_object(object) {
                return Some(found);
            }
        }
        current = dict.get(b"Parent").ok().and_then(|p| p.as_reference().ok())?;
    }
    None
}

/// Interpret a PDF rectangle array, tolerating reversed corners.
fn box_from_object(object: &Object) -> Option<Box4> {
    let array = object.as_array().ok()?;
    if array.len() != 4 {
        return None;
    }
    let mut values = [0f32; 4];
    for (index, item) in array.iter().enumerate() {
        values[index] = item.as_float().ok()?;
    }
    let [x1, y1, x2, y2] = values;
    Some(Box4 {
        left: x1.min(x2),
        bottom: y1.min(y2),
        right: x1.max(x2),
        top: y1.max(y2),
    })
}

/// Effective `/Rotate` for a page, normalised to one of 0/90/180/270.
fn page_rotation(doc: &Document, page_id: ObjectId) -> i32 {
    let mut current = page_id;
    for _ in 0..32 {
        let Ok(dict) = doc.get_dictionary(current) else {
            return 0;
        };
        if let Ok(object) = dict.get(b"Rotate") {
            let raw = object
                .as_i64()
                .or_else(|_| object.as_float().map(|v| v as i64))
                .unwrap_or(0);
            let normalised = ((raw % 360) + 360) % 360;
            return match normalised {
                45..=134 => 90,
                135..=224 => 180,
                225..=314 => 270,
                _ => 0,
            };
        }
        match dict.get(b"Parent").ok().and_then(|p| p.as_reference().ok()) {
            Some(parent) => current = parent,
            None => return 0,
        }
    }
    0
}

/// Displayed page size in points, accounting for `/Rotate`.
fn display_size(base: &Box4, rotation: i32) -> (f32, f32) {
    if rotation == 90 || rotation == 270 {
        (base.height(), base.width())
    } else {
        (base.width(), base.height())
    }
}

// ---------------------------------------------------------------------------
// rendered-pixel → PDF-point mapping
// ---------------------------------------------------------------------------

/// Maps a pixel in a pdfium bitmap back into PDF user space.
///
/// The bitmap is in *display* space (top-left origin, `/Rotate` applied by
/// pdfium), the crop box must be in *unrotated* PDF user space, so this applies
/// the inverse rotation as well as the y-flip.
struct Mapping {
    base: Box4,
    rotation: i32,
    width_px: f32,
    height_px: f32,
}

impl Mapping {
    fn to_user(&self, px: f32, py: f32) -> (f32, f32) {
        let (display_w, display_h) = display_size(&self.base, self.rotation);

        // Pixel → display space in points, with the y axis flipped so that it
        // points up from the bottom of the page instead of down from the top.
        let u = px / self.width_px * display_w;
        let v = display_h - py / self.height_px * display_h;

        let w = self.base.width();
        let h = self.base.height();

        // Undo `/Rotate`, then re-add the box origin.
        let (x, y) = match self.rotation {
            90 => (w - v, u),
            180 => (w - u, h - v),
            270 => (v, h - u),
            _ => (u, v),
        };

        (self.base.left + x, self.base.bottom + y)
    }
}

/// Bounding box of "ink" pixels as `(min_x, min_y, max_x, max_y)`, or `None` for
/// an empty page.
///
/// A pixel counts as ink when any colour channel is darker than 245/255, which
/// keeps anti-aliased text and light grey rules while ignoring the white
/// background pdfium clears the bitmap with.
fn ink_bounds(rgba: &[u8], width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
    const INK_THRESHOLD: u8 = 245;
    let width = width as usize;
    let height = height as usize;
    if width == 0 || height == 0 || rgba.len() < width * height * 4 {
        return None;
    }

    let mut min_x = usize::MAX;
    let mut min_y = usize::MAX;
    let mut max_x = 0usize;
    let mut max_y = 0usize;

    for y in 0..height {
        let row = y * width * 4;
        for x in 0..width {
            let offset = row + x * 4;
            let is_ink = rgba[offset] <= INK_THRESHOLD
                || rgba[offset + 1] <= INK_THRESHOLD
                || rgba[offset + 2] <= INK_THRESHOLD;
            if is_ink {
                if x < min_x {
                    min_x = x;
                }
                if x > max_x {
                    max_x = x;
                }
                if y < min_y {
                    min_y = y;
                }
                max_y = y;
            }
        }
    }

    if min_x == usize::MAX {
        None
    } else {
        Some((min_x as u32, min_y as u32, max_x as u32, max_y as u32))
    }
}

// ---------------------------------------------------------------------------
// pdfium discovery
// ---------------------------------------------------------------------------

/// Bind pdfium for `--auto`.
///
/// An explicit `--pdfium-path` wins outright. When it is absent this probes the
/// usual locations: the system search path first, then a copy next to the
/// executable, then the working directory, then the repository's bundled build
/// used by the test suite.
fn bind_pdfium(explicit: Option<&str>) -> Result<pdfium_render::prelude::Pdfium, PdfError> {
    if let Some(path) = explicit {
        if path.is_empty() {
            return Err(PdfError::MissingArg {
                arg: "--pdfium-path".into(),
                example: "pdf-cli crop --input in.pdf --output out.pdf --auto \
                          --pdfium-path \"C:\\path\\to\\pdfium.dll\""
                    .into(),
            });
        }
        if !Path::new(path).is_file() {
            return Err(PdfError::FileNotFound {
                path: path.to_string(),
            });
        }
        return io::bind_pdfium(Some(path));
    }

    let library = format!("pdfium{}", std::env::consts::DLL_SUFFIX);

    let mut candidates: Vec<Option<String>> = vec![None];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(Some(dir.join(&library).to_string_lossy().into_owned()));
        }
    }
    candidates.push(Some(library.clone()));
    candidates.push(Some(
        Path::new("testdata")
            .join("pdfium")
            .join("bin")
            .join(&library)
            .to_string_lossy()
            .into_owned(),
    ));

    let mut reasons: Vec<String> = Vec::new();
    for candidate in candidates {
        if let Some(path) = &candidate {
            if !Path::new(path).is_file() {
                continue;
            }
        }
        match io::bind_pdfium(candidate.as_deref()) {
            Ok(pdfium) => return Ok(pdfium),
            Err(error) => reasons.push(match &candidate {
                Some(path) => format!("{path}: {error}"),
                None => format!("system search path: {error}"),
            }),
        }
    }

    Err(PdfError::PdfiumNotFound {
        reason: if reasons.is_empty() {
            "no pdfium library found on the system search path or next to the executable".into()
        } else {
            reasons.join("; ")
        },
    })
}

// ---------------------------------------------------------------------------
// small helpers
// ---------------------------------------------------------------------------

/// Round a reported value to two decimals so the JSON stays readable and stable
/// across platforms.
fn r2(value: f32) -> f64 {
    ((value as f64) * 100.0).round() / 100.0
}
