use crate::error::PdfError;
use crate::io;
use crate::output::CommandResult;
use lopdf::{Object, Stream};
use std::path::Path;

/// 从 Filter 推断扩展名（仅对可直接写出的压缩格式有意义）。
fn ext_for_filter(filter: Option<&Object>, is_mask: bool) -> &'static str {
    if is_mask {
        return "png";
    }
    match filter {
        Some(Object::Name(n)) => match n.as_slice() {
            b"DCTDecode" => "jpg",
            b"JPXDecode" => "jp2",
            b"CCITTFaxDecode" => "tif",
            _ => "png",
        },
        Some(Object::Array(a)) => a
            .last()
            .and_then(|o| o.as_name().ok())
            .map(|n| match n {
                b"DCTDecode" => "jpg",
                b"JPXDecode" => "jp2",
                b"CCITTFaxDecode" => "tif",
                _ => "png",
            })
            .unwrap_or("png"),
        _ => "png",
    }
}

/// 判断是否为「原样写出」的压缩格式（无需解压）。
fn is_passthrough(ext: &str) -> bool {
    matches!(ext, "jpg" | "jp2")
}

/// 把已解压的原始像素编码为 PNG。
fn pixels_to_png(stream: &Stream, data: &[u8]) -> Option<Vec<u8>> {
    let dict = &stream.dict;
    let w = dict.get(b"Width").ok()?.as_i64().ok()? as u32;
    let h = dict.get(b"Height").ok()?.as_i64().ok()? as u32;
    let bpc = dict
        .get(b"BitsPerComponent")
        .ok()
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(8);

    if bpc != 8 {
        return None; // 只处理 8 位
    }

    let cs = dict.get(b"ColorSpace").ok().and_then(|o| o.as_name().ok());

    let dyn_img = match cs {
        Some(b"DeviceRGB") => {
            let buf = image::RgbImage::from_raw(w, h, data.to_vec())?;
            image::DynamicImage::ImageRgb8(buf)
        }
        Some(b"DeviceCMYK") => {
            let px_count = (w as usize) * (h as usize);
            if data.len() < px_count * 4 {
                return None;
            }
            let mut rgb = Vec::with_capacity(px_count * 3);
            for chunk in data.chunks_exact(4) {
                let c = chunk[0] as f32 / 255.0;
                let m = chunk[1] as f32 / 255.0;
                let y = chunk[2] as f32 / 255.0;
                let k = chunk[3] as f32 / 255.0;
                rgb.push((255.0 * (1.0 - c) * (1.0 - k)) as u8);
                rgb.push((255.0 * (1.0 - m) * (1.0 - k)) as u8);
                rgb.push((255.0 * (1.0 - y) * (1.0 - k)) as u8);
            }
            let buf = image::RgbImage::from_raw(w, h, rgb)?;
            image::DynamicImage::ImageRgb8(buf)
        }
        _ => {
            let buf = image::GrayImage::from_raw(w, h, data.to_vec())?;
            image::DynamicImage::ImageLuma8(buf)
        }
    };

    let mut out = std::io::Cursor::new(Vec::new());
    dyn_img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

pub fn run(
    input: &str,
    output_dir: &str,
    min_size: u32,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    if !Path::new(input).exists() {
        return Err(PdfError::FileNotFound {
            path: input.to_string(),
        });
    }

    let doc = io::load_doc_path(input)?;

    let pages = doc.get_pages();

    if dry_run {
        return Ok(CommandResult::ok(
            "extract-images",
            Some(format!(
                "[dry-run] Would extract embedded images from {} pages → {}",
                pages.len(),
                output_dir
            )),
            serde_json::json!({"dry_run": true, "pages": pages.len(), "output_dir": output_dir}),
        ));
    }

    std::fs::create_dir_all(output_dir).map_err(|e| PdfError::Operation {
        reason: e.to_string(),
    })?;

    let stem = Path::new(input)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("img");

    let mut extracted = 0u32;
    let mut files: Vec<String> = Vec::new();
    let mut skipped = 0u32;
    let mut seen: std::collections::HashSet<lopdf::ObjectId> = std::collections::HashSet::new();

    let mut page_nums: Vec<u32> = pages.keys().copied().collect();
    page_nums.sort();

    for page_num in page_nums {
        let page_id = match pages.get(&page_num) {
            Some(id) => *id,
            None => continue,
        };
        let page_dict = match doc.get_object(page_id).and_then(|o| o.as_dict()) {
            Ok(d) => d.clone(),
            Err(_) => continue,
        };

        let xobj_dict = match page_dict
            .get(b"Resources")
            .ok()
            .and_then(|o| match o {
                Object::Dictionary(d) => Some(d),
                Object::Reference(id) => doc.get_object(*id).ok().and_then(|x| x.as_dict().ok()),
                _ => None,
            })
            .and_then(|res| res.get(b"XObject").ok())
            .and_then(|o| match o {
                Object::Dictionary(d) => Some(d.clone()),
                Object::Reference(id) => doc
                    .get_object(*id)
                    .ok()
                    .and_then(|x| x.as_dict().ok())
                    .cloned(),
                _ => None,
            }) {
            Some(d) => d,
            None => continue,
        };

        for (_name, xref) in xobj_dict.iter() {
            let xobj_id = match xref {
                Object::Reference(id) => *id,
                _ => continue,
            };
            if seen.contains(&xobj_id) {
                continue;
            }

            let stream = match doc.get_object(xobj_id) {
                Ok(Object::Stream(s)) => s,
                _ => continue,
            };

            let subtype = stream
                .dict
                .get(b"Subtype")
                .ok()
                .and_then(|o| o.as_name().ok())
                .unwrap_or(b"");
            if subtype != b"Image" {
                continue;
            }

            let w = stream
                .dict
                .get(b"Width")
                .ok()
                .and_then(|o| o.as_i64().ok())
                .unwrap_or(0);
            let h = stream
                .dict
                .get(b"Height")
                .ok()
                .and_then(|o| o.as_i64().ok())
                .unwrap_or(0);

            if (w.max(h) as u32) < min_size {
                continue;
            }

            let is_mask = stream
                .dict
                .get(b"ImageMask")
                .ok()
                .and_then(|o| o.as_bool().ok())
                .unwrap_or(false);

            let filter = stream.dict.get(b"Filter").ok();
            let ext = ext_for_filter(filter, is_mask);

            // 关键：JPEG/JP2 可直接写出原始字节；其余必须先解压再编码 PNG。
            let data: Vec<u8> = if is_passthrough(ext) {
                stream.content.clone()
            } else {
                let pixels = stream
                    .decompressed_content()
                    .unwrap_or_else(|_| stream.content.clone());
                match pixels_to_png(stream, &pixels) {
                    Some(png) => png,
                    None => {
                        skipped += 1;
                        continue; // 无法解码的（如非 8 位 / CCITT）跳过，不写垃圾文件
                    }
                }
            };

            extracted += 1;
            let out_path = format!(
                "{}/{}_p{}_img{:03}.{}",
                output_dir, stem, page_num, extracted, ext
            );
            std::fs::write(&out_path, &data).map_err(|e| PdfError::Operation {
                reason: e.to_string(),
            })?;
            files.push(out_path);
            seen.insert(xobj_id);
        }
    }

    Ok(CommandResult::ok(
        "extract-images",
        Some(format!(
            "Extracted {} image(s) from {} → {}{}",
            extracted,
            input,
            output_dir,
            if skipped > 0 {
                format!(" ({} skipped: unsupported encoding)", skipped)
            } else {
                String::new()
            }
        )),
        serde_json::json!({
            "input": input,
            "output_dir": output_dir,
            "image_count": extracted,
            "skipped": skipped,
            "files": files,
        }),
    ))
}
