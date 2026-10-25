use crate::error::PdfError;
use crate::io;
use crate::output::CommandResult;
use lopdf::{Document, Object};

/// Stream compression profiles.
///
/// lopdf's `compress()` only re-applies Flate to uncompressed streams, so it
/// cannot express "how hard should we try". We add the parts it cannot do:
/// dropping metadata, stripping thumbnails, and optionally dropping the
/// structures that are only useful for interactive editing.
struct Profile {
    /// Strip the document Info dictionary (title/author/producer/...).
    drop_info: bool,
    /// Strip /Thumb entries from pages.
    drop_thumbnails: bool,
    /// Strip /Metadata XMP packets.
    drop_metadata: bool,
    /// Strip /Annots from pages (loses links, form widgets, comments).
    drop_annotations: bool,
    /// Strip /Outlines (bookmarks).
    drop_outlines: bool,
}

impl Profile {
    fn for_level(level: &str) -> Result<Self, PdfError> {
        // Everything starts with the parts that are always safe.
        let base = Profile {
            drop_info: true,
            drop_thumbnails: true,
            drop_metadata: false,
            drop_annotations: false,
            drop_outlines: false,
        };
        match level.to_ascii_lowercase().as_str() {
            "low" => Ok(Profile {
                drop_info: false,
                drop_thumbnails: false,
                ..base
            }),
            "medium" => Ok(base),
            "extreme" | "high" => Ok(Profile {
                drop_metadata: true,
                drop_annotations: true,
                drop_outlines: true,
                ..base
            }),
            other => Err(PdfError::Operation {
                reason: format!(
                    "Unknown compression level \"{other}\"; expected one of: low, medium, extreme"
                ),
            }),
        }
    }
}

pub fn compress(
    input: &str,
    output: &str,
    level: &str,
    password: Option<&str>,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    let profile = Profile::for_level(level)?;
    let input_size = if io::is_stdio(input) {
        0
    } else {
        std::fs::metadata(input).map(|m| m.len()).unwrap_or(0)
    };

    if dry_run {
        return Ok(CommandResult::ok(
            "compress",
            Some(format!(
                "[dry-run] Would compress {} ({:.1} KB) → {} at level {}",
                input,
                input_size as f64 / 1024.0,
                output,
                level
            )),
            serde_json::json!({
                "dry_run": true,
                "input": input,
                "output": output,
                "input_size": input_size,
                "level": level,
            }),
        ));
    }

    let mut doc = io::load_doc(input, password)?;

    // Drop the page thumbnails and annotations that inflate the file.
    let page_ids: Vec<(u32, lopdf::ObjectId)> =
        doc.get_pages().into_iter().collect();

    for (_, page_id) in &page_ids {
        let Ok(page) = doc.get_dictionary_mut(*page_id) else {
            continue;
        };
        if profile.drop_thumbnails {
            page.remove(b"Thumb");
        }
        if profile.drop_annotations {
            page.remove(b"Annots");
        }
    }

    if profile.drop_info {
        let info_ids: Vec<lopdf::ObjectId> = doc
            .trailer
            .get(b"Info")
            .ok()
            .and_then(|o| o.as_reference().ok())
            .into_iter()
            .collect();
        for id in info_ids {
            doc.objects.remove(&id);
        }
        doc.trailer.remove(b"Info");
    }

    if profile.drop_metadata || profile.drop_outlines {
        if let Ok(root_id) = doc.trailer.get(b"Root").and_then(|o| o.as_reference()) {
            if let Ok(catalog) = doc.get_dictionary_mut(root_id) {
                if profile.drop_metadata {
                    catalog.remove(b"Metadata");
                }
                if profile.drop_outlines {
                    catalog.remove(b"Outlines");
                }
            }
        }
    }

    doc.prune_objects();
    doc.compress();

    // lopdf cannot write to stdout, so `-` is only supported for real files.
    if io::is_stdio(output) {
        let mut buf = Vec::new();
        doc.save_to(&mut buf).map_err(|e| PdfError::Operation {
            reason: format!("Failed to serialize PDF: {e}"),
        })?;
        io::write_bytes(output, &buf)?;
    } else {
        doc.save(output).map_err(|e| PdfError::Operation {
            reason: format!("Failed to write {output}: {e}"),
        })?;
    }

    let output_size = if io::is_stdio(output) {
        0
    } else {
        std::fs::metadata(output).map(|m| m.len()).unwrap_or(0)
    };
    let ratio = if input_size > 0 {
        (1.0 - output_size as f64 / input_size as f64) * 100.0
    } else {
        0.0
    };

    Ok(CommandResult::ok(
        "compress",
        Some(format!(
            "Compressed ({level}): {:.1} KB → {:.1} KB (saved {:.0}%)",
            input_size as f64 / 1024.0,
            output_size as f64 / 1024.0,
            ratio
        )),
        serde_json::json!({
            "input": input,
            "output": output,
            "level": level,
            "input_size": input_size,
            "output_size": output_size,
            "saved_percent": format!("{:.1}", ratio),
            "dropped": {
                "info": profile.drop_info,
                "thumbnails": profile.drop_thumbnails,
                "metadata": profile.drop_metadata,
                "annotations": profile.drop_annotations,
                "outlines": profile.drop_outlines,
            }
        }),
    ))
}

/// Keep the unused import warning away while documenting intent.
#[allow(dead_code)]
fn _assert_object_in_scope(_: &Object) {}

#[allow(dead_code)]
fn _document_type(_: &Document) {}
