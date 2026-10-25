//! `organize`: reorder, duplicate and splice pages in one pass.
//!
//! This is a generalization of the page selection in [`crate::commands::pages`]:
//! `--pages` *selects* pages (it is a set, order and repeats are meaningless),
//! while `--order` describes a *sequence* in which repeats are the whole point.

use crate::commands::pages::copy_objects;
use crate::error::PdfError;
use crate::io;
use crate::output::CommandResult;
use lopdf::{Dictionary, Document, Object, ObjectId};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub struct OrganizeArgs<'a> {
    pub input: &'a str,
    pub output: &'a str,
    pub order: Option<&'a str>,
    pub insert: Option<&'a str>,
    pub password: Option<&'a str>,
}

/// Where one output page's content comes from.
enum Source {
    /// A page of the `--input` document (1-based).
    Input(u32),
    /// A page of the n-th loaded `--insert` file (1-based).
    Insert { doc_index: usize, page: u32 },
}

/// A loaded `--insert` target: where it goes in the page list, plus its content.
struct InsertDoc {
    /// 1-based position in the post-`--order` document to insert *before*.
    position: u32,
    /// The path exactly as the user wrote it (used for provenance output).
    file: String,
    doc: Document,
    pages: BTreeMap<u32, ObjectId>,
}

/// Reorder and/or splice the pages of a PDF into a new document.
///
/// # Page-order grammar (`--order`)
///
/// A comma-separated list of page numbers and ranges, all 1-based:
/// `3,1-2,3`, `5-`, `-2`, `1,1,1`. `N-M` is inclusive, `N-` runs to the last page,
/// `-M` starts at page 1.
///
/// Unlike `--pages`, which is parsed by [`io::parse_page_range`] and *de-duplicates*
/// (it selects a set of pages), this parser preserves **both order and duplicates**:
/// the output document contains exactly the listed pages, in the listed order.
/// Pages that are not listed are dropped. A repeated page becomes a second, distinct
/// page object that shares the same content streams and resources — never two page
/// objects claiming the same object id. An out-of-range page is an error naming the
/// offending page and the document's page count.
///
/// # Inserts (`--insert`)
///
/// `"position:file[,position:file...]"`. `position` is a 1-based index in the
/// *post-order* document before which the other document's pages are spliced;
/// `position == total + 1` appends at the end.
///
/// The file part is split on the **first** `:` only, because Windows paths carry
/// their own drive-letter colon (`--insert "2:C:\docs\b.pdf"` must yield position 2
/// and file `C:\docs\b.pdf`). Splitting on every colon, or on the last one, silently
/// mangles every drive-lettered path.
///
/// When several pairs are given they are applied **sorted by position descending**,
/// so a low insert position is still valid after the later pages have been spliced
/// in. Ties keep the user's listed order (the pair listed first ends up first).
/// `--order` is applied before any insert.
///
/// # Output
///
/// [`OrganizeArgs::output`] is required. With `dry_run` the complete page plan is
/// computed and returned but nothing is written. `details["pages_in"]` is the page
/// count of the input document (not the length of `--order`); `details["pages"]` is
/// the full per-output-page provenance map.
pub fn run(args: OrganizeArgs<'_>, dry_run: bool) -> Result<CommandResult, PdfError> {
    if args.order.is_none() && args.insert.is_none() {
        return Err(PdfError::MissingArg {
            arg: "--order or --insert".into(),
            example: "pdf-cli organize --input in.pdf --output out.pdf --order \"3,1-2\"  \
                      (or: pdf-cli organize --input in.pdf --output out.pdf --insert \"2:b.pdf\")"
                .into(),
        });
    }

    let main_doc = io::load_doc(args.input, args.password)?;
    let main_pages: BTreeMap<u32, ObjectId> = main_doc.get_pages();
    let pages_in = main_pages.len() as u32;
    if pages_in == 0 {
        return Err(PdfError::Operation {
            reason: format!(
                "{} has no pages; nothing to organize. Try: pdf-cli info --input {}",
                args.input, args.input
            ),
        });
    }

    // `--order` first: it defines the page sequence that insert positions refer to.
    let order: Vec<u32> = match args.order {
        Some(spec) => parse_order(spec, pages_in)?,
        None => (1..=pages_in).collect(),
    };
    let base_total = order.len() as u32;

    let mut insert_docs: Vec<InsertDoc> = Vec::new();
    if let Some(spec) = args.insert {
        for (position, file) in parse_insert_specs(spec)? {
            // Positions are indices into the post-order document, which has
            // `base_total` pages and therefore `base_total + 1` insert slots.
            if position > base_total + 1 {
                return Err(PdfError::InvalidPageRangeReason {
                    spec: format!("{position}:{file}"),
                    reason: format!(
                        "Insert position {position} is out of range (the ordered document has \
                         {base_total} page(s), so positions 1-{} are valid)",
                        base_total + 1
                    ),
                });
            }
            let doc = io::load_doc(&file, args.password)?;
            let pages: BTreeMap<u32, ObjectId> = doc.get_pages();
            if pages.is_empty() {
                return Err(PdfError::Operation {
                    reason: format!(
                        "Insert file {file} has no pages; nothing to splice in. \
                         Try: pdf-cli info --input {file}"
                    ),
                });
            }
            insert_docs.push(InsertDoc {
                position,
                file,
                doc,
                pages,
            });
        }
    }

    // --- Build the page plan: a flat list of (source document, source page). ---
    let mut plan: Vec<Source> = order.iter().map(|p| Source::Input(*p)).collect();

    // Apply inserts from the highest position downwards so that each remaining
    // (lower) insert position still points at the same slot.
    let mut apply_order: Vec<usize> = (0..insert_docs.len()).collect();
    apply_order.sort_by(|&a, &b| {
        insert_docs[b]
            .position
            .cmp(&insert_docs[a].position)
            .then(b.cmp(&a))
    });
    for idx in apply_order {
        let at = (insert_docs[idx].position - 1) as usize;
        let spliced: Vec<Source> = insert_docs[idx]
            .pages
            .keys()
            .map(|page| Source::Insert {
                doc_index: idx,
                page: *page,
            })
            .collect();
        plan.splice(at..at, spliced);
    }

    let pages_out = plan.len() as u32;

    // Provenance is plain data, so it is identical for the real run and dry-run.
    let mut provenance: Vec<serde_json::Value> = Vec::with_capacity(plan.len());
    for (i, entry) in plan.iter().enumerate() {
        let (source, source_page) = match entry {
            Source::Input(page) => (args.input.to_string(), *page),
            Source::Insert { doc_index, page } => {
                (insert_docs[*doc_index].file.clone(), *page)
            }
        };
        provenance.push(serde_json::json!({
            "out_index": i + 1,
            "source": source,
            "source_page": source_page,
        }));
    }

    let mut insert_summary: Vec<(u32, String, usize)> = insert_docs
        .iter()
        .map(|d| (d.position, d.file.clone(), d.pages.len()))
        .collect();
    insert_summary.sort();
    let inserts_applied: Vec<serde_json::Value> = insert_summary
        .iter()
        .map(|(position, file, pages)| {
            serde_json::json!({
                "position": position,
                "file": file,
                "pages": pages,
            })
        })
        .collect();

    let mut details = serde_json::json!({
        "input": args.input,
        "output": args.output,
        "order_applied": order,
        "inserts_applied": inserts_applied,
        "pages_in": pages_in,
        "pages_out": pages_out,
        "pages": provenance,
    });
    if dry_run {
        details["dry_run"] = serde_json::json!(true);
        return Ok(CommandResult::ok(
            "organize",
            Some(args.output.to_string()),
            details,
        ));
    }

    // --- Materialize the plan into a fresh document. ---
    // `copy_objects` deep-copies every object and remaps all references inside the
    // copies, so the page dicts we pull out of the map already point at the new
    // object ids. Do not remap again: the fresh document hands out ids that can
    // collide with the source ids, and a second pass would corrupt the references.
    let mut out = Document::with_version(main_doc.version.clone());
    let main_map = copy_objects(&mut out, &main_doc);

    let mut insert_maps: Vec<BTreeMap<ObjectId, ObjectId>> = Vec::with_capacity(insert_docs.len());
    for ins in &insert_docs {
        insert_maps.push(copy_objects(&mut out, &ins.doc));
    }

    // A page object may only occur once in the page tree, so repeated pages get a
    // fresh (shallow) copy of the page dict that shares the content objects.
    let mut used_pages: BTreeSet<ObjectId> = BTreeSet::new();
    let mut kid_ids: Vec<ObjectId> = Vec::with_capacity(plan.len());

    for entry in &plan {
        let (source_file, source_page, source_id) = match entry {
            Source::Input(page) => {
                let old_id = *main_pages.get(page).ok_or_else(|| PdfError::Operation {
                    reason: format!(
                        "Internal error: page {page} is missing from {} after loading. \
                         Try: pdf-cli info --input {}",
                        args.input, args.input
                    ),
                })?;
                // Always go through `copy_objects`' map: the fresh document re-numbers
                // objects, so a source id is NOT a valid id in the output document.
                let new_id = *main_map.get(&old_id).ok_or_else(|| PdfError::Operation {
                    reason: format!(
                        "Internal error: page {page} of {} was not copied. \
                         Try: pdf-cli info --input {}",
                        args.input, args.input
                    ),
                })?;
                (args.input, *page, new_id)
            }
            Source::Insert { doc_index, page } => {
                let ins = &insert_docs[*doc_index];
                let old_id = *ins.pages.get(page).ok_or_else(|| PdfError::Operation {
                    reason: format!(
                        "Internal error: page {page} is missing from insert file {}. \
                         Try: pdf-cli info --input {}",
                        ins.file, ins.file
                    ),
                })?;
                let new_id = *insert_maps[*doc_index].get(&old_id).ok_or_else(|| {
                    PdfError::Operation {
                        reason: format!(
                            "Internal error: page {page} of insert file {} was not copied. \
                             Try: pdf-cli info --input {}",
                            ins.file, ins.file
                        ),
                    }
                })?;
                (ins.file.as_str(), *page, new_id)
            }
        };

        let page_id = if used_pages.insert(source_id) {
            source_id
        } else {
            let obj = out
                .get_object(source_id)
                .map_err(|e| PdfError::Operation {
                    reason: format!(
                        "Failed to duplicate page {source_page} of {source_file}: {e}. \
                         Try: pdf-cli info --input {source_file}"
                    ),
                })?
                .clone();
            out.add_object(obj)
        };
        kid_ids.push(page_id);
    }

    // Rebuild the page tree exactly like `pages::merge` does: one flat /Pages node,
    // with every page's /Parent pointing at it.
    let mut pages_dict = Dictionary::new();
    pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
    pages_dict.set("Count", Object::Integer(kid_ids.len() as i64));
    let kids: Vec<Object> = kid_ids.iter().map(|id| Object::Reference(*id)).collect();
    pages_dict.set("Kids", Object::Array(kids));
    let pages_id: ObjectId = out.add_object(pages_dict);

    for page_id in &kid_ids {
        if let Ok(page_dict) = out
            .get_object_mut(*page_id)
            .and_then(|o| o.as_dict_mut())
        {
            page_dict.set("Parent", Object::Reference(pages_id));
        }
    }

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id: ObjectId = out.add_object(catalog);
    out.trailer.set("Root", Object::Reference(catalog_id));

    // Keep the input document's metadata (title/author/...) when present.
    if let Ok(info_id) = main_doc
        .trailer
        .get(b"Info")
        .and_then(|o| o.as_reference())
    {
        if let Some(new_id) = main_map.get(&info_id) {
            out.trailer.set("Info", Object::Reference(*new_id));
        }
    }

    // Drops the now-unreachable copies of the old catalog, page tree and any page
    // that did not make it into `--order`.
    out.prune_objects();

    if io::is_stdio(args.output) {
        let mut buf = Vec::new();
        out.save_to(&mut buf).map_err(|e| PdfError::Operation {
            reason: format!("Failed to serialize the organized PDF: {e}"),
        })?;
        io::write_bytes(args.output, &buf)?;
    } else {
        ensure_parent_dir(args.output)?;
        out.save(args.output)
            .map_err(|e| PdfError::Operation {
                reason: format!("Failed to write {}: {e}", args.output),
            })?;
    }

    Ok(CommandResult::ok(
        "organize",
        Some(args.output.to_string()),
        details,
    ))
}

/// Parse an `--order` spec into a 1-based page sequence, **keeping order and
/// duplicates**.
///
/// Grammar matches [`io::parse_page_range`] (`N`, `N-M`, `N-`, `-M`, comma
/// separated) but deliberately does not de-duplicate, because
/// `--order "1,1,1"` must produce three pages.
fn parse_order(spec: &str, total: u32) -> Result<Vec<u32>, PdfError> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Ok((1..=total).collect());
    }

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
                let p = order_page_num(part, spec)?;
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
                let start = if a.is_empty() { 1 } else { order_page_num(a, spec)? };
                let end = if b.is_empty() {
                    total
                } else {
                    order_page_num(b, spec)?
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

        // No `contains` check here: repeats are intentional in an order spec.
        pages.extend(start..=end);
    }

    if pages.is_empty() {
        return Err(PdfError::InvalidPageRangeReason {
            spec: spec.to_string(),
            reason: "Range matched no pages".into(),
        });
    }

    Ok(pages)
}

fn order_page_num(text: &str, spec: &str) -> Result<u32, PdfError> {
    text.parse::<u32>()
        .map_err(|_| PdfError::InvalidPageRangeReason {
            spec: spec.to_string(),
            reason: format!("\"{text}\" is not a page number"),
        })
}

/// Parse `--insert` into `(position, file)` pairs.
///
/// Pairs are comma-separated `position:file`. The pair is split on the **first**
/// colon only: Windows paths contain their own drive-letter colon, so
/// `2:C:\docs\b.pdf` must parse as position `2` and file `C:\docs\b.pdf`.
fn parse_insert_specs(spec: &str) -> Result<Vec<(u32, String)>, PdfError> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(PdfError::MissingArg {
            arg: "--insert".into(),
            example: "pdf-cli organize --input in.pdf --output out.pdf --insert \"2:b.pdf\"".into(),
        });
    }

    let mut pairs: Vec<(u32, String)> = Vec::new();
    for raw in spec.split(',') {
        let pair = raw.trim();
        if pair.is_empty() {
            return Err(PdfError::InvalidPageRangeReason {
                spec: spec.to_string(),
                reason: "Empty segment (stray comma)".into(),
            });
        }

        // First colon only — never `rsplit_once`, or `C:\...` loses its drive.
        let (position_text, file) =
            pair.split_once(':')
                .ok_or_else(|| PdfError::InvalidPageRangeReason {
                    spec: pair.to_string(),
                    reason: "Expected 'position:file', e.g. \"2:b.pdf\" or \"2:C:\\docs\\b.pdf\""
                        .into(),
                })?;

        let position = order_page_num(position_text.trim(), pair)?;
        if position < 1 {
            return Err(PdfError::InvalidPageRangeReason {
                spec: pair.to_string(),
                reason: "Insert positions are 1-based; position 1 inserts before the first page"
                    .into(),
            });
        }

        let file = file.trim();
        if file.is_empty() {
            return Err(PdfError::InvalidPageRangeReason {
                spec: pair.to_string(),
                reason: "Missing file after ':' — expected 'position:file'".into(),
            });
        }

        pairs.push((position, file.to_string()));
    }

    Ok(pairs)
}

/// Create the output directory when it does not exist yet (contract §5.7).
fn ensure_parent_dir(output: &str) -> Result<(), PdfError> {
    let path = Path::new(output);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| PdfError::Operation {
                reason: format!(
                    "Failed to create output directory {}: {e}",
                    parent.display()
                ),
            })?;
        }
    }
    Ok(())
}
