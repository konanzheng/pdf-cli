use crate::error::PdfError;
use crate::output::CommandResult;
use lopdf::{Document, Object, ObjectId};
use std::collections::BTreeMap;
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

fn copy_objects(dst: &mut Document, src: &Document) -> BTreeMap<ObjectId, ObjectId> {
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

fn remap_refs(obj: &mut Object, map: &BTreeMap<ObjectId, ObjectId>) {
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

    for file in &files {
        let doc = Document::load(file).map_err(|e| PdfError::ParseFailed {
            reason: format!("{}: {}", file, e),
        })?;
        let map = copy_objects(&mut merged, &doc);
        for (_, page_id) in doc.get_pages() {
            if let Some(new_id) = map.get(&page_id) {
                all_page_ids.push(*new_id);
            }
        }
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

    let mut catalog = lopdf::Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id: ObjectId = merged.add_object(catalog);
    merged.trailer.set("Root", Object::Reference(catalog_id));

    merged
        .save(output)
        .map_err(|e: std::io::Error| PdfError::Operation {
            reason: e.to_string(),
        })?;

    Ok(CommandResult::ok(
        "merge",
        Some(format!(
            "Merged {} files → {} ({} pages)",
            files.len(),
            output,
            all_page_ids.len()
        )),
        serde_json::json!({"output": output, "files_merged": files.len(), "total_pages": all_page_ids.len()}),
    ))
}

pub fn split(
    input: &str,
    output_dir: &str,
    ranges: Option<&str>,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    let doc = Document::load(input).map_err(|e| PdfError::ParseFailed {
        reason: e.to_string(),
    })?;
    let total = doc.get_pages().len();

    let groups: Vec<Vec<u32>> = if let Some(r) = ranges {
        r.split(',')
            .map(|part| parse_page_range(part.trim(), total))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        (1..=total).map(|p| vec![p as u32]).collect()
    };

    if dry_run {
        return Ok(CommandResult::ok(
            "split",
            Some(format!(
                "[dry-run] Would split into {} files in {}",
                groups.len(),
                output_dir
            )),
            serde_json::json!({"dry_run": true, "output_files": groups.len(), "output_dir": output_dir}),
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

    Ok(CommandResult::ok(
        "split",
        Some(format!(
            "Split into {} files in {}",
            outputs.len(),
            output_dir
        )),
        serde_json::json!({"output_dir": output_dir, "files": outputs}),
    ))
}

pub fn delete_pages(
    input: &str,
    pages_spec: &str,
    output: &str,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    let mut doc = Document::load(input).map_err(|e| PdfError::ParseFailed {
        reason: e.to_string(),
    })?;
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

    let mut doc = Document::load(input).map_err(|e| PdfError::ParseFailed {
        reason: e.to_string(),
    })?;
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
