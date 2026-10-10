use crate::error::PdfError;
use crate::output::CommandResult;
use image::codecs::jpeg::JpegEncoder;
use lopdf::{dictionary, Document, Object, ObjectId, Stream};
use std::collections::HashSet;
use std::path::Path;

// =====================================================================
// watermark-add — 添加文本水印
// =====================================================================

/// 把 /F1 字体引用合并进页面的 Resources，**保留**已有的 XObject / 其他字体 / 属性。
///
/// 之前的实现直接覆盖整个 Resources，导致页面上原有的图像（XObject）被丢弃。
fn merge_font_resource(doc: &mut Document, page_id: ObjectId, font_id: ObjectId) {
    let existing = doc
        .get_object(page_id)
        .ok()
        .and_then(|o| o.as_dict().ok())
        .and_then(|d| d.get(b"Resources").ok().cloned());

    let mut res_dict = match existing {
        Some(Object::Dictionary(d)) => d,
        Some(Object::Reference(id)) => doc
            .get_object(id)
            .ok()
            .and_then(|o| o.as_dict().ok())
            .cloned()
            .unwrap_or_else(lopdf::Dictionary::new),
        _ => lopdf::Dictionary::new(),
    };

    let mut font_dict = match res_dict.get(b"Font").ok() {
        Some(Object::Dictionary(d)) => d.clone(),
        Some(Object::Reference(id)) => doc
            .get_object(*id)
            .ok()
            .and_then(|o| o.as_dict().ok())
            .cloned()
            .unwrap_or_else(lopdf::Dictionary::new),
        _ => lopdf::Dictionary::new(),
    };
    font_dict.set("F1", Object::Reference(font_id));
    res_dict.set("Font", Object::Dictionary(font_dict));

    if let Ok(page_dict) = doc.get_object_mut(page_id).and_then(|o| o.as_dict_mut()) {
        page_dict.set("Resources", Object::Dictionary(res_dict));
    }
}

pub fn add(
    input: &str,
    text: &str,
    output: &str,
    opacity: f32,
    rotation: f32,
    font_size: f32,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    if !Path::new(input).exists() {
        return Err(PdfError::FileNotFound {
            path: input.to_string(),
        });
    }

    if dry_run {
        return Ok(CommandResult::ok(
            "watermark-add",
            Some(format!(
                "[dry-run] Would add watermark '{}' to {}",
                text, input
            )),
            serde_json::json!({"dry_run": true, "text": text, "input": input}),
        ));
    }

    let mut doc = Document::load(input).map_err(|e| PdfError::ParseFailed {
        reason: e.to_string(),
    })?;

    let pages = doc.get_pages();
    let page_count = pages.len();

    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });

    for (_, page_id) in pages {
        let cos_r = format!("{:.4}", rotation.to_radians().cos());
        let sin_r = format!("{:.4}", rotation.to_radians().sin());
        let neg_sin_r = format!("{:.4}", -rotation.to_radians().sin());
        let content = format!(
            "q\n{opacity} g\nBT\n/F1 {fs} Tf\n{cos_r} {sin_r} {neg_sin_r} {cos_r} 0 0 Tm\n({txt}) Tj\nET\nQ\n",
            opacity = opacity,
            fs = font_size,
            cos_r = cos_r,
            sin_r = sin_r,
            neg_sin_r = neg_sin_r,
            txt = text.replace('(', "\\(").replace(')', "\\)"),
        );

        let stream = Stream::new(lopdf::Dictionary::new(), content.into_bytes());
        let stream_id = doc.add_object(stream);

        // 合并字体资源（保留页面原有的 XObject 等），再挂上水印内容流
        merge_font_resource(&mut doc, page_id, font_id);

        if let Ok(page_dict) = doc.get_object_mut(page_id).and_then(|o| o.as_dict_mut()) {
            let existing = page_dict
                .get(b"Contents")
                .ok()
                .and_then(|o| o.as_array().ok())
                .cloned();
            match existing {
                Some(mut arr) => {
                    arr.push(Object::Reference(stream_id));
                    page_dict.set("Contents", Object::Array(arr));
                }
                None => {
                    let original = page_dict.get(b"Contents").ok().cloned();
                    let mut arr = vec![Object::Reference(stream_id)];
                    if let Some(orig) = original {
                        arr.push(orig);
                    }
                    page_dict.set("Contents", Object::Array(arr));
                }
            }
        }
    }

    doc.save(output).map_err(|e| PdfError::Operation {
        reason: e.to_string(),
    })?;

    Ok(CommandResult::ok(
        "watermark-add",
        Some(format!(
            "Added '{}' watermark to {} pages → {}",
            text, page_count, output
        )),
        serde_json::json!({"output": output, "text": text, "pages": page_count}),
    ))
}

// =====================================================================
// watermark-remove — 去除「烧录进图像」的水印（尽力版）
//
// 适用范围：页面为整页扫描图/栅格图（Image XObject），水印是渲染进像素的
// 浅色（低饱和、高亮度）内容。支持两种图像编码：
//   - DCTDecode (JPEG)：直接解码
//   - FlateDecode / 无 Filter (raw 8-bit RGB/Gray)：先解压再解析
// 统一解码为 RGB8，按亮度阈值 + 饱和度上限把「浅灰」像素涂白，再编码回 JPEG。
//
// 不适用：矢量 / 文本层水印（无独立对象可剥离）。
// =====================================================================

/// 解析页码范围，如 "1-3,5,7-9"。
fn parse_pages(spec: &str, total: u32) -> Result<Vec<u32>, PdfError> {
    let mut out = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            let s: u32 = a
                .trim()
                .parse()
                .map_err(|_| PdfError::InvalidPageRange { range: spec.into() })?;
            let e: u32 = b
                .trim()
                .parse()
                .map_err(|_| PdfError::InvalidPageRange { range: spec.into() })?;
            for p in s..=e {
                if p < 1 || p > total {
                    return Err(PdfError::InvalidPageRange { range: spec.into() });
                }
                out.push(p);
            }
        } else {
            let p: u32 = part
                .parse()
                .map_err(|_| PdfError::InvalidPageRange { range: spec.into() })?;
            if p < 1 || p > total {
                return Err(PdfError::InvalidPageRange { range: spec.into() });
            }
            out.push(p);
        }
    }
    Ok(out)
}

/// 该 Filter 名是否被支持（DCTDecode / FlateDecode）。
fn filter_supported(filter: Option<&Object>) -> bool {
    let is_ok = |n: &[u8]| n == b"DCTDecode" || n == b"FlateDecode";
    match filter {
        None => true, // 无 Filter = 原始未压缩流
        Some(Object::Name(n)) => is_ok(n.as_slice()),
        Some(Object::Array(a)) => a
            .iter()
            .any(|o| matches!(o.as_name(), Ok(n) if is_ok(n))),
        _ => false,
    }
}

fn filter_is_dct(filter: Option<&Object>) -> bool {
    match filter {
        Some(Object::Name(n)) => n.as_slice() == b"DCTDecode",
        Some(Object::Array(a)) => a
            .iter()
            .any(|o| matches!(o.as_name(), Ok(n) if n == b"DCTDecode")),
        _ => false,
    }
}

/// 是否可作为「烧录水印」候选图像处理：8 位、RGB/Gray、支持的 Filter。
fn is_supported_image(stream: &Stream) -> bool {
    let bpc = stream
        .dict
        .get(b"BitsPerComponent")
        .ok()
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(8);
    if bpc != 8 {
        return false;
    }
    let cs = stream
        .dict
        .get(b"ColorSpace")
        .ok()
        .and_then(|o| o.as_name().ok());
    if !matches!(cs, Some(b"DeviceRGB") | Some(b"DeviceGray")) {
        return false;
    }
    filter_supported(stream.dict.get(b"Filter").ok())
}

/// 把图像流解码为 RGB8 原始像素。
fn decode_to_rgb(stream: &Stream) -> Option<(Vec<u8>, u32, u32)> {
    let filter = stream.dict.get(b"Filter").ok();
    let w = stream.dict.get(b"Width").ok()?.as_i64().ok()? as u32;
    let h = stream.dict.get(b"Height").ok()?.as_i64().ok()? as u32;
    let cs = stream
        .dict
        .get(b"ColorSpace")
        .ok()
        .and_then(|o| o.as_name().ok());

    if filter_is_dct(filter) {
        let img = image::load_from_memory(&stream.content).ok()?;
        let rgb = img.to_rgb8();
        let (iw, ih) = rgb.dimensions();
        return Some((rgb.into_raw(), iw, ih));
    }

    // FlateDecode / raw：解压得到原始像素
    let raw = stream
        .decompressed_content()
        .unwrap_or_else(|_| stream.content.clone());
    let px_count = (w as usize) * (h as usize);
    match cs {
        Some(b"DeviceRGB") => {
            if raw.len() < px_count * 3 {
                return None;
            }
            Some((raw[..px_count * 3].to_vec(), w, h))
        }
        Some(b"DeviceGray") => {
            if raw.len() < px_count {
                return None;
            }
            let mut rgb = Vec::with_capacity(px_count * 3);
            for &g in &raw[..px_count] {
                rgb.push(g);
                rgb.push(g);
                rgb.push(g);
            }
            Some((rgb, w, h))
        }
        _ => None,
    }
}

/// 对 RGB8 像素去水印：把「亮度 >= threshold 且 饱和度 <= max_sat」的像素涂白。
/// 返回被涂白的像素数。
fn whiteout_watermark(raw: &mut [u8], threshold: u8, max_sat: u8) -> u64 {
    let mut masked = 0u64;
    for px in raw.chunks_exact_mut(3) {
        let r = px[0] as i32;
        let g = px[1] as i32;
        let b = px[2] as i32;
        let lum = (r * 299 + g * 587 + b * 114) / 1000;
        let sat = r.max(g).max(b) - r.min(g).min(b);
        if lum >= threshold as i32 && sat <= max_sat as i32 {
            px[0] = 255;
            px[1] = 255;
            px[2] = 255;
            masked += 1;
        }
    }
    masked
}

/// 把处理后的 RGB8 编码为 JPEG 字节。
fn encode_jpeg(raw: &[u8], w: u32, h: u32, quality: u8) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = JpegEncoder::new_with_quality(&mut out, quality);
    enc.encode(raw, w, h, image::ExtendedColorType::Rgb8).ok()?;
    Some(out)
}

/// 收集一页 Resources 里引用的 Image XObject 对象号。
fn page_image_xobjects(doc: &Document, page_id: ObjectId) -> Vec<ObjectId> {
    let mut out = Vec::new();
    let page_dict = match doc.get_object(page_id).and_then(|o| o.as_dict()) {
        Ok(d) => d,
        Err(_) => return out,
    };
    let xobj_dict = page_dict
        .get(b"Resources")
        .ok()
        .and_then(|o| match o {
            Object::Dictionary(d) => Some(d.clone()),
            Object::Reference(id) => doc
                .get_object(*id)
                .ok()
                .and_then(|x| x.as_dict().ok())
                .cloned(),
            _ => None,
        })
        .and_then(|res| res.get(b"XObject").ok().cloned())
        .and_then(|o| match o {
            Object::Dictionary(d) => Some(d.clone()),
            Object::Reference(id) => doc
                .get_object(id)
                .ok()
                .and_then(|x| x.as_dict().ok())
                .cloned(),
            _ => None,
        });
    if let Some(d) = xobj_dict {
        for (_name, val) in d.iter() {
            if let Object::Reference(id) = val {
                out.push(*id);
            }
        }
    }
    out
}

/// 去除扫描/栅格页面上「烧录进图像」的水印。
pub fn remove(
    input: &str,
    output: &str,
    threshold: u8,
    max_sat: u8,
    pages_spec: Option<&str>,
    quality: u8,
    dry_run: bool,
) -> Result<CommandResult, PdfError> {
    if !Path::new(input).exists() {
        return Err(PdfError::FileNotFound {
            path: input.to_string(),
        });
    }
    if quality == 0 || quality > 100 {
        return Err(PdfError::Operation {
            reason: format!("Invalid quality: {}. Use 1-100.", quality),
        });
    }

    let mut doc = Document::load(input).map_err(|e| PdfError::ParseFailed {
        reason: e.to_string(),
    })?;
    let all_pages = doc.get_pages();
    let total = all_pages.len() as u32;

    let selected: Vec<u32> = match pages_spec {
        Some(spec) => parse_pages(spec, total)?,
        None => (1..=total).collect(),
    };

    if dry_run {
        return Ok(CommandResult::ok(
            "watermark-remove",
            Some(format!(
                "[dry-run] Would remove burned-in watermarks from {} page(s) of {} → {}",
                selected.len(),
                input,
                output
            )),
            serde_json::json!({
                "dry_run": true,
                "input": input,
                "output": output,
                "pages": selected,
                "threshold": threshold,
                "max_sat": max_sat,
                "quality": quality,
            }),
        ));
    }

    let mut seen: HashSet<ObjectId> = HashSet::new();
    let mut images_processed: u32 = 0;
    let mut images_skipped: u32 = 0;
    let mut total_masked: u64 = 0;

    for &p in &selected {
        let page_id = match all_pages.get(&p) {
            Some(id) => *id,
            None => continue,
        };
        for img_id in page_image_xobjects(&doc, page_id) {
            if seen.contains(&img_id) {
                continue;
            }
            seen.insert(img_id);

            let supported = matches!(
                doc.get_object(img_id),
                Ok(Object::Stream(s)) if is_supported_image(s)
            );
            if !supported {
                images_skipped += 1;
                continue;
            }

            // 解码为 RGB8
            let (mut raw, w, h) = match doc
                .get_object(img_id)
                .ok()
                .and_then(|o| o.as_stream().ok())
                .and_then(decode_to_rgb)
            {
                Some(v) => v,
                None => {
                    images_skipped += 1;
                    continue;
                }
            };

            let masked = whiteout_watermark(&mut raw, threshold, max_sat);
            let jpeg = match encode_jpeg(&raw, w, h, quality) {
                Some(j) => j,
                None => {
                    images_skipped += 1;
                    continue;
                }
            };

            if let Ok(Object::Stream(s)) = doc.get_object_mut(img_id) {
                s.content = jpeg;
                s.dict.set("Width", Object::Integer(w as i64));
                s.dict.set("Height", Object::Integer(h as i64));
                s.dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
                s.dict.set("BitsPerComponent", Object::Integer(8));
                s.dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
                // 关键：同步更新 Length，并清除旧的 DecodeParms，否则写出的流长度字段
                // 与实际内容不符，导致重新加载时对象边界错乱（图像"消失"）。
                s.dict.set("Length", Object::Integer(s.content.len() as i64));
                s.dict.remove(b"DecodeParms");
            }
            images_processed += 1;
            total_masked += masked;
        }
    }

    if images_processed == 0 {
        return Err(PdfError::Operation {
            reason: "No removable image watermarks found. This command only handles burned-in \
                     watermarks on raster/scanned pages (8-bit RGB/Gray image XObjects with \
                     DCTDecode or FlateDecode); it does NOT remove vector or text-layer watermarks."
                .into(),
        });
    }

    doc.save(output).map_err(|e| PdfError::Operation {
        reason: e.to_string(),
    })?;

    let in_size = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
    let out_size = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);

    Ok(CommandResult::ok(
        "watermark-remove",
        Some(format!(
            "Removed burned-in watermarks: {} image(s) processed, {} skipped → {} ({:.1} KB → {:.1} KB)",
            images_processed,
            images_skipped,
            output,
            in_size as f64 / 1024.0,
            out_size as f64 / 1024.0,
        )),
        serde_json::json!({
            "input": input,
            "output": output,
            "pages": selected.len(),
            "images_processed": images_processed,
            "images_skipped": images_skipped,
            "pixels_masked": total_masked,
            "threshold": threshold,
            "max_sat": max_sat,
            "quality": quality,
            "input_size": in_size,
            "output_size": out_size,
        }),
    ))
}
