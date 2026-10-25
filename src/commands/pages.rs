use crate::error::PdfError;
use crate::io;
use crate::output::CommandResult;
use lopdf::{Dictionary, Document, Object, ObjectId};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn parse_page_range(spec: &str, total: usize) -> Result<Vec<u32>, PdfError> {
    let mut result = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.contains('-') {
            let parts: Vec<&str> = part.splitn(2, '-').collect();
            let start: usize = parts[0].parse().map_err(|_| PdfError::InvalidPageRange {
                range: spec.to_string(),
            })?;
            let end: usize = parts[1].parse().map_err(|_| PdfError::InvalidPageRange {
                range: spec.to_string(),
            })?;
            for p in start..=end {
                if p < 1 || p > total {
                    return Err(PdfError::InvalidPageRange {
                        range: spec.to_string(),
                    });
                }
                result.push(p as u32);
            }
        } else {
            let p: usize = part.parse().map_err(|_| PdfError::InvalidPageRange {
                range: spec.to_string(),
            })?;
            if p < 1 || p > total {
                return Err(PdfError::InvalidPageRange {
                    range: spec.to_string(),
                });
            }
            result.push(p as u32);
        }
    }
    Ok(result)
}

pub fn copy_objects(dst: &mut Document, src: &Document) -> BTreeMap<ObjectId, ObjectId> {
    let mut map: BTreeMap<ObjectId, ObjectId> = BTreeMap::new();
    for (&old_id, obj) in &src.objects {
        let new_id: ObjectId = dst.add_object(obj.clone());
        map.insert(old_id, new_id);
    }
    for &new_id in map.values() {
        if let Ok(obj) = dst.get_object_mut(new_id) {
            remap_refs(obj, &map);
        }
    }
    map
}

pub fn remap_refs(obj: &mut Object, map: &BTreeMap<ObjectId, ObjectId>) {
    match obj {
        Object::Reference(id) => {
            if let Some(new) = map.get(id) {
                *id = *new;
            }
        }
        Object::Array(arr) => {
            for o in arr.iter_mut() {
                remap_refs(o, map);
            }
        }
        Object::Dictionary(d) => {
            for (_, v) in d.iter_mut() {
                remap_refs(v, map);
            }
        }
        Object::Stream(s) => {
            for (_, v) in s.dict.iter_mut() {
                remap_refs(v, map);
            }
        }
        _ => {}
    }
}

/// Read `obj` as a dictionary, following at most one level of indirection.
fn dict_of<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Dictionary> {
    match obj {
        Object::Dictionary(d) => Some(d),
        Object::Reference(id) => doc.get_object(*id).ok().and_then(|o| o.as_dict().ok()),
        _ => None,
    }
}

/// Walk a PDF name tree (`/Names` + `/Kids`) collecting name -> destination pairs.
fn collect_named_dests(
    doc: &Document,
    node: &Dictionary,
    out: &mut BTreeMap<Vec<u8>, Object>,
    depth: usize,
) {
    if depth > 32 {
        return;
    }
    if let Ok(names) = node.get(b"Names").and_then(Object::as_array) {
        let mut i = 0;
        while i + 1 < names.len() {
            if let (Ok(key), Some(value)) = (names[i].as_str(), names.get(i + 1)) {
                out.entry(key.to_vec()).or_insert_with(|| value.clone());
            }
            i += 2;
        }
    }
    if let Ok(kids) = node.get(b"Kids").and_then(Object::as_array) {
        for kid in kids {
            if let Some(child) = dict_of(doc, kid) {
                collect_named_dests(doc, child, out, depth + 1);
            }
        }
    }
}

/// Resolve a bookmark destination to a 1-based page number in `doc`.
///
/// Handles the three shapes PDFs actually use: an explicit destination array
/// (`[page /Fit]`), a bare page number, and a named destination (looked up in
/// the caller-supplied table, which includes both `/Dests` and `/Names /Dests`).
fn resolve_dest_page(
    doc: &Document,
    dest: &Object,
    named: &BTreeMap<Vec<u8>, Object>,
    page_num_by_id: &BTreeMap<ObjectId, u32>,
    page_count: usize,
    depth: usize,
) -> Option<u32> {
    if depth > 16 {
        return None;
    }
    match dest {
        Object::Array(arr) => {
            let first = arr.first()?;
            if let Ok(id) = first.as_reference() {
                if let Some(n) = page_num_by_id.get(&id) {
                    return Some(*n);
                }
                let target = doc.get_object(id).ok()?;
                return resolve_dest_page(doc, target, named, page_num_by_id, page_count, depth + 1);
            }
            if let Ok(n) = first.as_i64() {
                // The spec says page numbers here are 0-based; a few producers
                // write 1-based ones, so accept a small 1-based fallback.
                if n >= 0 && (n as usize) < page_count {
                    return Some(n as u32 + 1);
                }
                if n >= 1 && (n as usize) <= page_count {
                    return Some(n as u32);
                }
            }
            None
        }
        Object::Reference(id) => {
            if let Some(n) = page_num_by_id.get(id) {
                return Some(*n);
            }
            let target = doc.get_object(*id).ok()?;
            if let Ok(d) = target.as_dict().and_then(|d| d.get(b"D")) {
                return resolve_dest_page(doc, d, named, page_num_by_id, page_count, depth + 1);
            }
            resolve_dest_page(doc, target, named, page_num_by_id, page_count, depth + 1)
        }
        Object::Dictionary(d) => {
            let d = d.get(b"D").ok()?;
            resolve_dest_page(doc, d, named, page_num_by_id, page_count, depth + 1)
        }
        Object::String(key, _) | Object::Name(key) => {
            let target = named.get(key)?;
            resolve_dest_page(doc, target, named, page_num_by_id, page_count, depth + 1)
        }
        _ => None,
    }
}

/// A bookmark read out of a source document, before it is rebuilt in the merge.
struct RawOutline {
    title: Object,
    /// 1-based page number in the *source* document; `None` if unresolvable.
    page: Option<u32>,
    /// `true` when the source bookmark was expanded (`/Count` >= 0).
    open: bool,
    children: Vec<RawOutline>,
}

/// A bookmark ready to be written into the merged document.
#[derive(Clone)]
struct BuildNode {
    title: Object,
    /// Destination array pointing at a page of the merged document.
    dest: Object,
    open: bool,
    children: Vec<BuildNode>,
}

fn dest_array(page: ObjectId) -> Object {
    Object::Array(vec![Object::Reference(page), Object::Name(b"Fit".to_vec())])
}

/// Walk one sibling chain of an outline (`/First` then `/Next`), recursing into
/// `/First` children. `seen` guards against malformed cyclic trees.
fn walk_outline(
    doc: &Document,
    start: &Object,
    named: &BTreeMap<Vec<u8>, Object>,
    page_num_by_id: &BTreeMap<ObjectId, u32>,
    page_count: usize,
    depth: usize,
    out: &mut Vec<RawOutline>,
    seen: &mut BTreeSet<ObjectId>,
) {
    if depth > 32 {
        return;
    }
    let mut cursor = Some(start.clone());
    let mut guard = 0usize;
    while let Some(obj) = cursor {
        guard += 1;
        if guard > 10_000 {
            break;
        }
        if let Ok(id) = obj.as_reference() {
            if !seen.insert(id) {
                break;
            }
        }
        let dict = match dict_of(doc, &obj) {
            Some(d) => d,
            None => break,
        };
        let title = dict
            .get(b"Title")
            .cloned()
            .unwrap_or_else(|_| lopdf::text_string("Untitled"));
        let dest = dict.get(b"Dest").ok().or_else(|| {
            dict.get(b"A")
                .ok()
                .and_then(|a| dict_of(doc, a))
                .and_then(|action| action.get(b"D").ok())
        });
        let page = dest.and_then(|d| {
            resolve_dest_page(doc, d, named, page_num_by_id, page_count, 0)
        });
        // A negative /Count means the item starts collapsed.
        let open = dict
            .get(b"Count")
            .ok()
            .and_then(|c| c.as_i64().ok())
            .map(|c| c >= 0)
            .unwrap_or(true);
        let mut children = Vec::new();
        if let Ok(child_first) = dict.get(b"First") {
            let child_first = child_first.clone();
            walk_outline(
                doc,
                &child_first,
                named,
                page_num_by_id,
                page_count,
                depth + 1,
                &mut children,
                seen,
            );
        }
        out.push(RawOutline {
            title,
            page,
            open,
            children,
        });
        cursor = dict.get(b"Next").ok().cloned();
    }
}

/// Copy raw bookmarks into build nodes, remapping page numbers to merged pages.
///
/// A bookmark whose destination cannot be resolved still gets copied, pointed at
/// `fallback` (the first page of the file it came from) rather than inventing a
/// page number.
fn build_nodes_from_raw<F>(
    items: &[RawOutline],
    resolve: &F,
    fallback: Option<ObjectId>,
    depth: usize,
) -> Vec<BuildNode>
where
    F: Fn(u32) -> Option<ObjectId>,
{
    if depth > 32 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for item in items {
        let page = item.page.and_then(resolve).or(fallback);
        let Some(page) = page else { continue };
        let children = build_nodes_from_raw(&item.children, resolve, fallback, depth + 1);
        out.push(BuildNode {
            title: item.title.clone(),
            dest: dest_array(page),
            open: item.open,
            children,
        });
    }
    out
}

/// Write a sibling chain of bookmarks into `dst`, returning `(first, last,
/// descendant_count)` for the parent's `/First` / `/Last` / `/Count` keys.
///
/// `/Count` is signed the way PDF readers expect: positive means the top level
/// is expanded, negative means it is collapsed.
fn build_outline_chain(
    dst: &mut Document,
    nodes: &[BuildNode],
    parent: ObjectId,
    counter: &mut usize,
) -> Option<(ObjectId, ObjectId, i64)> {
    if nodes.is_empty() {
        return None;
    }
    let ids: Vec<ObjectId> = (0..nodes.len()).map(|_| dst.new_object_id()).collect();
    let mut descendants = 0i64;
    for (i, node) in nodes.iter().enumerate() {
        let sub = build_outline_chain(dst, &node.children, ids[i], counter);
        let mut dict = Dictionary::new();
        dict.set("Title", node.title.clone());
        dict.set("Parent", Object::Reference(parent));
        dict.set("Dest", node.dest.clone());
        if i > 0 {
            dict.set("Prev", Object::Reference(ids[i - 1]));
        }
        if let Some(next) = ids.get(i + 1) {
            dict.set("Next", Object::Reference(*next));
        }
        if let Some((first, last, count)) = sub {
            dict.set("First", Object::Reference(first));
            dict.set("Last", Object::Reference(last));
            dict.set(
                "Count",
                Object::Integer(if node.open { count } else { -count }),
            );
            descendants += 1 + count;
        } else {
            descendants += 1;
        }
        *counter += 1;
        dst.set_object(ids[i], dict);
    }
    Some((ids[0], *ids.last()?, descendants))
}

/// The document's `/Info /Title`, if it has a non-empty one.
fn meta_title(doc: &Document) -> Option<String> {
    let info = doc.trailer.get(b"Info").ok()?;
    let info = dict_of(doc, info)?;
    let title = info.get(b"Title").ok()?;
    let decoded = lopdf::decode_text_string(title).ok()?;
    let decoded = decoded.trim().to_string();
    if decoded.is_empty() {
        None
    } else {
        Some(decoded)
    }
}

/// Everything the merge needs to know about one input file's bookmarks.
struct SourceOutline {
    title: String,
    first_page: Option<ObjectId>,
    /// Bookmarks already remapped onto merged page ids.
    children: Vec<BuildNode>,
    had_outline: bool,
}

pub fn merge(input: &str, output: &str, dry_run: bool) -> Result<CommandResult, PdfError> {
    let files: Vec<&str> = input.split(',').map(|s| s.trim()).collect();
    if files.is_empty() {
        return Err(PdfError::MissingArg {
            arg: "input".into(),
            example: "pdf-cli merge --input a.pdf,b.pdf --output merged.pdf".into(),
        });
    }
    for f in &files {
        if !Path::new(f).exists() {
            return Err(PdfError::FileNotFound {
                path: f.to_string(),
            });
        }
    }

    if dry_run {
        return Ok(CommandResult::ok(
            "merge",
            Some(format!(
                "[dry-run] Would merge {} files into {}",
                files.len(),
                output
            )),
            serde_json::json!({"dry_run": true, "input_count": files.len(), "output": output}),
        ));
    }

    let mut merged = Document::with_version("1.5");
    let mut all_page_ids: Vec<ObjectId> = Vec::new();
    let mut sources: Vec<SourceOutline> = Vec::new();

    for (index, file) in files.iter().enumerate() {
        let doc = Document::load(file).map_err(|e| PdfError::ParseFailed {
            reason: format!("{}: {}", file, e),
        })?;
        let map = copy_objects(&mut merged, &doc);

        // Remember where this file's pages landed so bookmark destinations can
        // be rewritten as merged page references. Source destinations reference
        // *source* page ids, so keep both directions of the mapping.
        let mut file_pages: Vec<(u32, ObjectId, ObjectId)> = Vec::new();
        for (num, page_id) in doc.get_pages() {
            if let Some(new_id) = map.get(&page_id) {
                all_page_ids.push(*new_id);
                file_pages.push((num, page_id, *new_id));
            }
        }
        let file_page_ids: BTreeMap<u32, ObjectId> =
            file_pages.iter().map(|(n, _, new)| (*n, *new)).collect();
        let page_num_by_id: BTreeMap<ObjectId, u32> =
            file_pages.iter().map(|(n, src, _)| (*src, *n)).collect();
        let first_merged_page = file_pages.first().map(|(_, _, new)| *new);

        let stem = Path::new(file)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("File {}", index + 1));
        let title = meta_title(&doc).unwrap_or(stem);

        let mut named: BTreeMap<Vec<u8>, Object> = BTreeMap::new();
        if let Ok(catalog) = doc.catalog() {
            if let Ok(dests) = catalog.get(b"Dests").and_then(Object::as_dict) {
                for (key, value) in dests.iter() {
                    named.insert(key.clone(), value.clone());
                }
            }
            if let Ok(names) = catalog.get(b"Names").and_then(Object::as_dict) {
                collect_named_dests(&doc, names, &mut named, 0);
            }
        }

        let page_count = file_pages.len();
        let mut raw: Vec<RawOutline> = Vec::new();
        let mut had_outline = false;
        if let Ok(catalog) = doc.catalog() {
            if let Ok(root_ref) = catalog.get(b"Outlines") {
                if let Some(root) = dict_of(&doc, root_ref) {
                    if let Ok(first) = root.get(b"First") {
                        had_outline = true;
                        let first = first.clone();
                        let mut seen = BTreeSet::new();
                        walk_outline(
                            &doc,
                            &first,
                            &named,
                            &page_num_by_id,
                            page_count,
                            0,
                            &mut raw,
                            &mut seen,
                        );
                    }
                }
            }
        }

        // Unresolvable destinations fall back to this file's first page.
        let resolve = |n: u32| file_page_ids.get(&n).copied();
        let children = build_nodes_from_raw(&raw, &resolve, first_merged_page, 0);
        sources.push(SourceOutline {
            title,
            first_page: first_merged_page,
            children,
            had_outline,
        });
    }

    let mut pages_dict = lopdf::Dictionary::new();
    pages_dict.set("Type", Object::Name(b"Pages".to_vec()));
    pages_dict.set("Count", Object::Integer(all_page_ids.len() as i64));
    let kids: Vec<Object> = all_page_ids
        .iter()
        .map(|id| Object::Reference(*id))
        .collect();
    pages_dict.set("Kids", Object::Array(kids));
    let pages_id: ObjectId = merged.add_object(pages_dict);

    for page_id in &all_page_ids {
        if let Ok(page_dict) = merged
            .get_object_mut(*page_id)
            .and_then(|o| o.as_dict_mut())
        {
            page_dict.set("Parent", Object::Reference(pages_id));
        }
    }

    // Build the merged outline: one top-level node per input file, that file's
    // own bookmarks nested underneath it.
    let outline_merged = sources.iter().any(|s| s.had_outline);
    let mut outline_items = 0usize;
    let mut outline_root: Option<ObjectId> = None;

    if outline_merged {
        let root_id = merged.new_object_id();
        let mut top: Vec<BuildNode> = Vec::new();
        for source in &sources {
            let Some(first_page) = source.first_page else {
                continue;
            };
            top.push(BuildNode {
                title: lopdf::text_string(&source.title),
                dest: dest_array(first_page),
                // Per-file bookmarks start expanded so the merged tree is
                // immediately navigable.
                open: true,
                children: source.children.clone(),
            });
        }
        if !top.is_empty() {
            let mut counter = 0usize;
            if let Some((first, last, count)) =
                build_outline_chain(&mut merged, &top, root_id, &mut counter)
            {
                let mut root = Dictionary::new();
                root.set("Type", Object::Name(b"Outlines".to_vec()));
                root.set("First", Object::Reference(first));
                root.set("Last", Object::Reference(last));
                // Positive => the top level starts expanded.
                root.set("Count", Object::Integer(count.abs()));
                merged.set_object(root_id, root);
                outline_items = counter;
                outline_root = Some(root_id);
            }
        }
    }

    let mut catalog = lopdf::Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    if let Some(root) = outline_root {
        catalog.set("Outlines", Object::Reference(root));
    }
    let catalog_id: ObjectId = merged.add_object(catalog);
    merged.trailer.set("Root", Object::Reference(catalog_id));

    merged
        .save(output)
        .map_err(|e: std::io::Error| PdfError::Operation {
            reason: e.to_string(),
        })?;

    let outline_note = if outline_merged {
        format!(", {} bookmarks", outline_items)
    } else {
        String::new()
    };

    Ok(CommandResult::ok(
        "merge",
        Some(format!(
            "Merged {} files → {} ({} pages{})",
            files.len(),
            output,
            all_page_ids.len(),
            outline_note
        )),
        serde_json::json!({
            "output": output,
            "files_merged": files.len(),
            "total_pages": all_page_ids.len(),
            "outline_merged": outline_merged,
            "outline_items": outline_items,
        }),
    ))
}

pub fn split(
    input: &str,
    output_dir: &str,
    ranges: Option<&str>,
    every: Option<u32>,
    odd_even: bool,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    // Exactly one grouping strategy may be requested.
    let mut requested: Vec<&str> = Vec::new();
    if ranges.is_some() {
        requested.push("--ranges");
    }
    if every.is_some() {
        requested.push("--every");
    }
    if odd_even {
        requested.push("--odd-even");
    }
    if requested.len() > 1 {
        return Err(PdfError::Operation {
            reason: format!(
                "Choose only one of --ranges, --every, --odd-even (got {})",
                requested.join(" and ")
            ),
        });
    }

    if let Some(0) = every {
        return Err(PdfError::Operation {
            reason: "`--every` must be at least 1".into(),
        });
    }

    let doc = io::load_doc_path(input)?;
    let total = doc.get_pages().len();

    let strategy = if ranges.is_some() {
        "ranges"
    } else if every.is_some() {
        "every"
    } else if odd_even {
        "odd-even"
    } else {
        "per-page"
    };

    let groups: Vec<Vec<u32>> = if let Some(r) = ranges {
        r.split(',')
            .map(|part| parse_page_range(part.trim(), total))
            .collect::<Result<Vec<_>, _>>()?
    } else if let Some(n) = every {
        let n = n as usize;
        (0..total)
            .step_by(n)
            .map(|start| {
                let end = (start + n).min(total);
                ((start + 1)..=end).map(|p| p as u32).collect()
            })
            .collect()
    } else if odd_even {
        let odds: Vec<u32> = (1..=total).filter(|p| p % 2 == 1).map(|p| p as u32).collect();
        let evens: Vec<u32> = (1..=total).filter(|p| p % 2 == 0).map(|p| p as u32).collect();
        // Never emit an empty file for a document with no pages of that parity.
        let mut groups = Vec::new();
        if !odds.is_empty() {
            groups.push(odds);
        }
        if !evens.is_empty() {
            groups.push(evens);
        }
        groups
    } else {
        (1..=total).map(|p| vec![p as u32]).collect()
    };

    let pages_per_file: Vec<usize> = groups.iter().map(|g| g.len()).collect();

    if dry_run {
        let mut details = serde_json::json!({
            "dry_run": true,
            "strategy": strategy,
            "output_files": groups.len(),
            "output_dir": output_dir,
            "pages_per_file": pages_per_file,
            "pages_per_group": groups,
        });
        if let Some(n) = every {
            details["every"] = serde_json::json!(n);
        }
        return Ok(CommandResult::ok(
            "split",
            Some(format!(
                "[dry-run] Would split into {} files in {}",
                groups.len(),
                output_dir
            )),
            details,
        ));
    }

    std::fs::create_dir_all(output_dir).map_err(|e: std::io::Error| PdfError::Operation {
        reason: e.to_string(),
    })?;

    let stem = Path::new(input)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");

    let mut outputs = Vec::new();
    for (i, pages) in groups.iter().enumerate() {
        let mut new_doc = doc.clone();
        let all: Vec<u32> = new_doc.get_pages().keys().copied().collect();
        let to_delete: Vec<u32> = all.iter().filter(|p| !pages.contains(p)).copied().collect();
        if !to_delete.is_empty() {
            new_doc.delete_pages(&to_delete);
        }
        let out_path = format!("{}/{}_{}.pdf", output_dir, stem, i + 1);
        new_doc
            .save(&out_path)
            .map_err(|e: std::io::Error| PdfError::Operation {
                reason: e.to_string(),
            })?;
        outputs.push(out_path);
    }

    let mut details = serde_json::json!({
        "output_dir": output_dir,
        "strategy": strategy,
        "files": outputs,
        "pages_per_file": pages_per_file,
    });
    if let Some(n) = every {
        details["every"] = serde_json::json!(n);
    }

    Ok(CommandResult::ok(
        "split",
        Some(format!(
            "Split into {} files in {}",
            outputs.len(),
            output_dir
        )),
        details,
    ))
}

pub fn delete_pages(
    input: &str,
    pages_spec: &str,
    output: &str,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    let mut doc = io::load_doc_path(input)?;
    let total = doc.get_pages().len();
    let to_delete = parse_page_range(pages_spec, total)?;

    if to_delete.len() >= total {
        return Err(PdfError::Operation {
            reason: "Cannot delete all pages".into(),
        });
    }

    if dry_run {
        return Ok(CommandResult::ok(
            "delete-pages",
            Some(format!(
                "[dry-run] Would delete pages {:?} from {}",
                to_delete, input
            )),
            serde_json::json!({"dry_run": true, "pages_deleted": to_delete}),
        ));
    }

    doc.delete_pages(&to_delete);
    doc.save(output)
        .map_err(|e: std::io::Error| PdfError::Operation {
            reason: e.to_string(),
        })?;

    Ok(CommandResult::ok(
        "delete-pages",
        Some(format!("Deleted {} pages → {}", to_delete.len(), output)),
        serde_json::json!({"output": output, "pages_deleted": to_delete, "remaining": total - to_delete.len()}),
    ))
}

pub fn rotate(
    input: &str,
    angle: i32,
    pages_spec: Option<&str>,
    output: &str,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    if ![90, 180, 270].contains(&angle) {
        return Err(PdfError::Operation {
            reason: format!("Invalid angle: {}. Use 90, 180, or 270", angle),
        });
    }

    let mut doc = io::load_doc_path(input)?;
    let total = doc.get_pages().len();
    let pages = if let Some(spec) = pages_spec {
        parse_page_range(spec, total)?
    } else {
        (1..=total as u32).collect()
    };

    if dry_run {
        return Ok(CommandResult::ok(
            "rotate",
            Some(format!(
                "[dry-run] Would rotate {} pages by {}°",
                pages.len(),
                angle
            )),
            serde_json::json!({"dry_run": true, "pages": pages, "angle": angle}),
        ));
    }

    let page_map: BTreeMap<u32, ObjectId> = doc.get_pages().into_iter().collect();
    for page_num in &pages {
        if let Some(page_id) = page_map.get(page_num) {
            if let Ok(page_dict) = doc.get_object_mut(*page_id).and_then(|o| o.as_dict_mut()) {
                let current: i64 = page_dict
                    .get(b"Rotate")
                    .ok()
                    .and_then(|o| o.as_i64().ok())
                    .unwrap_or(0);
                let new_angle = (current + angle as i64).rem_euclid(360);
                page_dict.set("Rotate", Object::Integer(new_angle));
            }
        }
    }

    doc.save(output)
        .map_err(|e: std::io::Error| PdfError::Operation {
            reason: e.to_string(),
        })?;

    Ok(CommandResult::ok(
        "rotate",
        Some(format!(
            "Rotated {} pages by {}° → {}",
            pages.len(),
            angle,
            output
        )),
        serde_json::json!({"output": output, "pages": pages, "angle": angle}),
    ))
}
