use crate::error::PdfError;
use crate::output::CommandResult;
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use std::collections::BTreeMap;
use std::path::Path;

const A4_W: f32 = 595.28;
const A4_H: f32 = 841.89;
const HALF_H: f32 = A4_H / 2.0;
const MARGIN: f32 = 18.0;

fn num(obj: &Object) -> f32 {
    match obj {
        Object::Integer(v) => *v as f32,
        Object::Real(v) => *v as f32,
        _ => 0.0,
    }
}

fn media_box_size(dict: &Dictionary) -> (f32, f32) {
    if let Ok(mb) = dict.get(b"MediaBox").and_then(|o| o.as_array()) {
        if mb.len() >= 4 {
            let x0 = num(&mb[0]);
            let y0 = num(&mb[1]);
            let x1 = num(&mb[2]);
            let y1 = num(&mb[3]);
            let w = (x1 - x0).abs();
            let h = (y1 - y0).abs();
            if w > 0.0 && h > 0.0 {
                return (w, h);
            }
        }
    }
    (A4_W, HALF_H)
}

fn content_bytes(doc: &Document, contents: &Object) -> Vec<u8> {
    match contents {
        Object::Reference(id) => match doc.get_object(*id) {
            Ok(Object::Stream(s)) => s.content.clone(),
            _ => Vec::new(),
        },
        Object::Array(arr) => {
            let mut out = Vec::new();
            for o in arr {
                out.extend(content_bytes(doc, o));
                out.push(b'\n');
            }
            out
        }
        _ => Vec::new(),
    }
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

fn copy_objects(dst: &mut Document, src: &Document) -> BTreeMap<ObjectId, ObjectId> {
    let mut map: BTreeMap<ObjectId, ObjectId> = BTreeMap::new();
    for (&old_id, obj) in &src.objects {
        let new_id = dst.add_object(obj.clone());
        map.insert(old_id, new_id);
    }
    for &new_id in map.values() {
        if let Ok(obj) = dst.get_object_mut(new_id) {
            remap_refs(obj, &map);
        }
    }
    map
}

fn pdf_first_page_to_form(
    dst: &mut Document,
    src_path: &str,
) -> Result<(ObjectId, f32, f32), PdfError> {
    let src = Document::load(src_path).map_err(|e| PdfError::ParseFailed {
        reason: format!("{}: {}", src_path, e),
    })?;
    let pages = src.get_pages();
    let (&_num, &page_id) = pages.iter().next().ok_or_else(|| PdfError::Operation {
        reason: format!("{} has no pages", src_path),
    })?;

    let page_dict = src
        .get_object(page_id)
        .and_then(|o| o.as_dict())
        .map_err(|e| PdfError::ParseFailed {
            reason: e.to_string(),
        })?
        .clone();

    let (w, h) = media_box_size(&page_dict);

    let map = copy_objects(dst, &src);
    let new_page_id = *map.get(&page_id).ok_or_else(|| PdfError::Operation {
        reason: "internal: page id not mapped".into(),
    })?;

    let new_page_dict = dst
        .get_object(new_page_id)
        .and_then(|o| o.as_dict())
        .map_err(|e| PdfError::Operation {
            reason: e.to_string(),
        })?
        .clone();

    let contents = new_page_dict
        .get(b"Contents")
        .map(|o| o.clone())
        .unwrap_or(Object::Array(vec![]));
    let resources = new_page_dict
        .get(b"Resources")
        .map(|o| o.clone())
        .unwrap_or(Object::Dictionary(Dictionary::new()));

    let bytes = content_bytes(dst, &contents);

    let mut form_dict = Dictionary::new();
    form_dict.set("Type", Object::Name(b"XObject".to_vec()));
    form_dict.set("Subtype", Object::Name(b"Form".to_vec()));
    form_dict.set("FormType", Object::Integer(1));
    form_dict.set(
        "BBox",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Real(w),
            Object::Real(h),
        ]),
    );
    form_dict.set("Resources", resources);

    let form_id = dst.add_object(Stream::new(form_dict, bytes));
    Ok((form_id, w, h))
}

fn image_to_xobject(dst: &mut Document, path: &str) -> Result<(ObjectId, f32, f32), PdfError> {
    let img = image::open(path).map_err(|e| PdfError::Operation {
        reason: format!("Cannot open image {}: {}", path, e),
    })?;
    let rgb = img.to_rgb8();
    let (iw, ih) = rgb.dimensions();
    let raw = rgb.into_raw();

    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"XObject".to_vec()));
    d.set("Subtype", Object::Name(b"Image".to_vec()));
    d.set("Width", Object::Integer(iw as i64));
    d.set("Height", Object::Integer(ih as i64));
    d.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
    d.set("BitsPerComponent", Object::Integer(8));

    let mut s = Stream::new(d, raw);
    let _ = s.compress();
    let id = dst.add_object(s);
    Ok((id, iw as f32, ih as f32))
}

pub fn run(input: &str, output: &str, dry_run: bool) -> Result<CommandResult, PdfError> {
    let files: Vec<&str> = input
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if files.is_empty() {
        return Err(PdfError::MissingArg {
            arg: "input".into(),
            example: "pdf-cli invoice --input \"inv1.jpg,inv2.pdf\" --output invoice.pdf".into(),
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
            "invoice",
            Some(format!(
                "[dry-run] Would place {} item(s) onto A4 (2 per page) → {}",
                files.len(),
                output
            )),
            serde_json::json!({"dry_run": true, "item_count": files.len(), "output": output}),
        ));
    }

    let mut doc = Document::with_version("1.5");
    let mut page_ids: Vec<ObjectId> = Vec::new();
    let mut placed = 0usize;

    let mut idx = 0usize;
    while idx < files.len() {
        let mut xobjects: Vec<(String, ObjectId)> = Vec::new();
        let mut content = String::from("q\n");
        let mut placed_this_page = 0;

        for slot in 0..2 {
            if idx >= files.len() {
                break;
            }
            let path = files[idx];
            let ext = Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            let (xid, w0, h0) = match ext.as_str() {
                "pdf" => pdf_first_page_to_form(&mut doc, path)?,
                _ => image_to_xobject(&mut doc, path)?,
            };

            let avail_w = A4_W - 2.0 * MARGIN;
            let avail_h = HALF_H - 2.0 * MARGIN;
            let scale = (avail_w / w0).min(avail_h / h0);
            let dw = w0 * scale;
            let dh = h0 * scale;

            let region_y0 = if slot == 0 { HALF_H } else { 0.0 };
            let x = (A4_W - dw) / 2.0;
            let y = region_y0 + (HALF_H - dh) / 2.0;

            let name = format!("X{}", xobjects.len());
            content.push_str(&format!(
                "{:.2} 0 0 {:.2} {:.2} {:.2} cm\n/{} Do\n",
                dw, dh, x, y, name
            ));

            xobjects.push((name, xid));
            placed_this_page += 1;
            placed += 1;
            idx += 1;
        }
        content.push_str("Q\n");

        let content_id = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));

        let mut xobj_dict = Dictionary::new();
        for (name, xid) in &xobjects {
            xobj_dict.set(name.clone(), Object::Reference(*xid));
        }
        let resources = dictionary! {
            "XObject" => Object::Dictionary(xobj_dict),
            "ProcSet" => Object::Array(vec![
                Object::Name(b"PDF".to_vec()),
                Object::Name(b"ImageC".to_vec()),
                Object::Name(b"ImageB".to_vec()),
            ]),
        };

        let page = dictionary! {
            "Type" => "Page",
            "MediaBox" => Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Real(A4_W),
                Object::Real(A4_H),
            ]),
            "Resources" => Object::Dictionary(resources),
            "Contents" => Object::Reference(content_id),
        };
        page_ids.push(doc.add_object(page));

        if placed_this_page == 0 {
            break;
        }
    }

    let kids: Vec<Object> = page_ids.iter().map(|id| Object::Reference(*id)).collect();
    let pages_dict = dictionary! {
        "Type" => "Pages",
        "Kids" => Object::Array(kids),
        "Count" => Object::Integer(page_ids.len() as i64),
    };
    let pages_id = doc.add_object(pages_dict);

    for &pid in &page_ids {
        if let Ok(d) = doc.get_object_mut(pid).and_then(|o| o.as_dict_mut()) {
            d.set("Parent", Object::Reference(pages_id));
        }
    }

    let catalog = dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(pages_id),
    };
    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", Object::Reference(catalog_id));

    doc.save(output).map_err(|e| PdfError::Operation {
        reason: e.to_string(),
    })?;

    Ok(CommandResult::ok(
        "invoice",
        Some(format!(
            "Merged {} item(s) onto {} A4 page(s) (2-up) → {}",
            placed,
            page_ids.len(),
            output
        )),
        serde_json::json!({
            "output": output,
            "items": placed,
            "pages": page_ids.len(),
            "layout": "a4-2up-vertical",
        }),
    ))
}
