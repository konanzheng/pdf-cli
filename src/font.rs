//! TrueType font embedding for text we draw into a PDF ourselves.
//!
//! The previous page-number implementation used the base-14 font Helvetica with
//! `WinAnsiEncoding`, which can only address Latin-1. Any CJK glyph in the
//! template (`第 {n} 页`) therefore came out as mojibake.
//!
//! This module fixes that properly:
//!
//! 1. Locate a system font that actually contains the needed characters.
//! 2. Map each character to its glyph id with `ttf-parser`.
//! 3. Build a [`GlyphRemapper`] so `subsetter` can shrink the font to just those
//!    glyphs (a full CJK font is 10-20 MB; the subset is a few KB).
//! 4. Emit a `/Type0` + `/Identity-H` font object, where the "character code" in
//!    the content stream is the *CID*, which `subsetter` makes equal to the new
//!    glyph id.
//!
//! `subsetter` deliberately drops the `cmap` table, so nothing else may depend on
//! Unicode lookup: the PDF side must be Identity-H with `/CIDToGIDMap /Identity`.
//! That is exactly what [`EmbeddedFont`] produces.

use lopdf::{dictionary, Document, Object, Stream};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use subsetter::GlyphRemapper;
use ttf_parser::Face;

/// A font already subsetted and registered in a [`Document`].
pub struct EmbeddedFont {
    /// Object id of the Type0 font dictionary.
    pub font_id: lopdf::ObjectId,
    /// CI → new glyph id. Only characters in the original text are present.
    pub cids: std::collections::BTreeMap<char, u16>,
    /// Advance width per new glyph id, in 1/1000 em (PDF glyph space).
    pub widths: Vec<u16>,
}

impl EmbeddedFont {
    /// Encode `text` as Identity-H hex, using the subsetted glyph ids.
    ///
    /// Characters absent from the subset are skipped: the caller should have
    /// built the subset from the same text, so this only drops characters the
    /// chosen font genuinely cannot render.
    pub fn encode(&self, text: &str) -> String {
        let mut hex = String::with_capacity(text.len() * 4);
        for ch in text.chars() {
            if let Some(gid) = self.cids.get(&ch) {
                hex.push_str(&format!("{gid:04X}"));
            }
        }
        hex
    }

    /// `true` when every character of `text` can be encoded.
    pub fn covers(&self, text: &str) -> bool {
        text.chars().all(|c| self.cids.contains_key(&c))
    }

    /// Build the `/W` array for the subsetted glyphs so viewers advance correctly.
    fn width_array(&self) -> Object {
        // Compact form: [ cidFirst [ w1 w2 ... ] cidSecond [ ... ] ]
        let mut entries: Vec<Object> = Vec::new();
        let mut i = 0usize;
        while i < self.widths.len() {
            let start = i;
            let mut run: Vec<Object> = Vec::new();
            while i < self.widths.len() && run.len() < 100 {
                run.push(Object::Integer(self.widths[i] as i64));
                i += 1;
            }
            entries.push(Object::Integer(start as i64));
            entries.push(Object::Array(run));
        }
        Object::Array(entries)
    }
}

/// Candidate system fonts, ordered by how safe they are to subset.
///
/// `.ttc` collections are addressed by face index 0. Variable fonts are avoided:
/// `subsetter` only instantiates them behind its `variable-fonts` feature.
fn font_candidates() -> Vec<(PathBuf, u32)> {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_string());
    let fonts = Path::new(&windir).join("Fonts");
    ["simhei.ttf", "Deng.ttf", "msyh.ttc", "simsun.ttc", "msjh.ttc"]
        .iter()
        .map(|name| (fonts.join(name), 0))
        .collect()
}

/// Pick the first available candidate font that covers every character in `text`.
///
/// `requested` lets the caller force a specific font file (`--font`).
fn resolve_font(text: &str, requested: Option<&str>) -> Result<(Vec<u8>, u32), String> {
    let candidates: Vec<(PathBuf, u32)> = match requested {
        Some(p) => vec![(PathBuf::from(p), 0)],
        None => font_candidates(),
    };

    let mut tried: Vec<String> = Vec::new();
    for (path, index) in candidates {
        let Ok(data) = std::fs::read(&path) else {
            tried.push(format!("{} (not found)", path.display()));
            continue;
        };
        let Ok(face) = Face::parse(&data, index) else {
            tried.push(format!("{} (unreadable)", path.display()));
            continue;
        };
        if text.chars().all(|c| c.is_whitespace() || face.glyph_index(c).is_some()) {
            return Ok((data, index));
        }
        tried.push(format!("{} (missing glyphs)", path.display()));
    }

    Err(format!(
        "No font with the required glyphs found. Tried: {}. \
         Pass --font <path to a .ttf/.ttc> to use a specific font.",
        tried.join(", ")
    ))
}

/// Subset `font_data` down to the glyphs needed by `text` and register the result
/// as a Type0 font in `doc`.
///
/// The caller writes text as `<hex> Tj` using [`EmbeddedFont::encode`].
pub fn embed(
    doc: &mut Document,
    text: &str,
    requested_font: Option<&str>,
    font_name: &str,
) -> Result<EmbeddedFont, crate::error::PdfError> {
    let (font_data, face_index) = resolve_font(text, requested_font)
        .map_err(|reason| crate::error::PdfError::Operation { reason })?;

    let face = Face::parse(&font_data, face_index).map_err(|e| {
        crate::error::PdfError::Operation {
            reason: format!("Failed to parse font: {e}"),
        }
    })?;
    let units_per_em = face.units_per_em() as f32;

    // Collect the glyph ids we actually need. Always include glyph 0 (.notdef).
    let mut wanted: Vec<u16> = vec![0];
    let mut char_to_old: std::collections::BTreeMap<char, u16> =
        std::collections::BTreeMap::new();
    for ch in text.chars() {
        if ch.is_whitespace() && ch != ' ' {
            // Tabs/newlines never reach here, but normalise rather than skip.
        }
        let Some(gid) = face.glyph_index(ch) else {
            continue;
        };
        let gid = gid.0;
        if !char_to_old.values().any(|v| *v == gid) {
            wanted.push(gid);
        }
        char_to_old.insert(ch, gid);
    }

    wanted.sort_unstable();
    wanted.dedup();

    let mut remapper = GlyphRemapper::new();
    for gid in &wanted {
        remapper.remap(*gid);
    }

    let sub = subsetter::subset(&font_data, face_index, &remapper).map_err(|e| {
        crate::error::PdfError::Operation {
            reason: format!("Failed to subset font: {e}"),
        }
    })?;

    // Old gid → new gid, per the remapper. New gids are the CIDs we write.
    let mut cids: std::collections::BTreeMap<char, u16> = std::collections::BTreeMap::new();
    for (ch, old_gid) in &char_to_old {
        if let Some(new_gid) = remapper.get(*old_gid) {
            cids.insert(*ch, new_gid);
        }
    }

    // Advance widths, indexed by new glyph id.
    let max_new_gid = cids.values().copied().max().unwrap_or(0);
    let mut widths = vec![0u16; max_new_gid as usize + 1];
    for (ch, new_gid) in &cids {
        if let Some(old_gid) = char_to_old.get(ch) {
            if let Some(adv) = face.glyph_hor_advance(ttf_parser::GlyphId(*old_gid)) {
                widths[*new_gid as usize] =
                    ((adv as f32) * 1000.0 / units_per_em).round() as u16;
            }
        }
    }

    let embedded = EmbeddedFont {
        font_id: (0, 0), // replaced below
        cids,
        widths,
    };

    // FontFile2 stream carries the subsetted TrueType program.
    let font_file_id = doc.add_object(Stream::new(
        dictionary! { "Length1" => sub.len() as i64 },
        sub,
    ));

    let descriptor_id = doc.add_object(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => Object::Name(font_name.as_bytes().to_vec()),
        "Flags" => 4i64, // Symbolic
        "FontBBox" => Object::Array(vec![
            Object::Integer(-1000), Object::Integer(-1000),
            Object::Integer(3000), Object::Integer(3000),
        ]),
        "ItalicAngle" => 0i64,
        "Ascent" => 880i64,
        "Descent" => -120i64,
        "CapHeight" => 700i64,
        "StemV" => 80i64,
        "FontFile2" => Object::Reference(font_file_id),
    });

    // A /ToUnicode CMap is what lets text extractors -- and therefore any AI
    // reading the output -- recover the original characters. Without it the
    // Identity-H byte codes are opaque and `pdf-cli extract` fails with
    // `missing required dictionary key "ToUnicode"`.
    let to_unicode = build_to_unicode_cmap(&embedded.cids);
    let cmap_id = doc.add_object(Stream::new(
        dictionary! { "Length" => to_unicode.len() as i64 },
        to_unicode,
    ));

    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => Object::Name(font_name.as_bytes().to_vec()),
        "Encoding" => "Identity-H",
        "ToUnicode" => Object::Reference(cmap_id),
        "DescendantFonts" => Object::Array(vec![Object::Dictionary(dictionary! {
            "Type" => "Font",
            "Subtype" => "CIDFontType2",
            "BaseFont" => Object::Name(font_name.as_bytes().to_vec()),
            "CIDSystemInfo" => Object::Dictionary(dictionary! {
                "Registry" => Object::string_literal("Adobe"),
                "Ordering" => Object::string_literal("Identity"),
                "Supplement" => 0i64,
            }),
            "FontDescriptor" => Object::Reference(descriptor_id),
            "DW" => 1000i64,
            "W" => embedded.width_array(),
            "CIDToGIDMap" => "Identity",
        })]),
    });

    Ok(EmbeddedFont {
        font_id,
        ..embedded
    })
}

/// Cache of a resolved font path so repeated calls do not re-scan the font dir.
#[allow(dead_code)]
fn cached_font() -> &'static Option<PathBuf> {
    static CACHE: OnceLock<Option<PathBuf>> = OnceLock::new();
    CACHE.get_or_init(|| None)
}

/// Build a `/ToUnicode` CMap mapping each subset CID back to its Unicode scalar.
///
/// The CIDs written into content streams are the subsetted glyph ids, which are
/// contiguous from 0. A `bfchar` entry per glyph is the simplest correct form;
/// with a few dozen glyphs this stays tiny.
fn build_to_unicode_cmap(cids: &std::collections::BTreeMap<char, u16>) -> Vec<u8> {
    let mut out = String::new();
    out.push_str("/CIDInit /ProcSet findresource begin\n");
    out.push_str("12 dict begin\nbegincmap\n");
    out.push_str("/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n");
    out.push_str("/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n");
    out.push_str("1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n");

    // Collect (cid, unicode) pairs, sorted by CID for deterministic output.
    let mut pairs: Vec<(u16, u32)> = cids
        .iter()
        .map(|(ch, cid)| (*cid, *ch as u32))
        .collect();
    pairs.sort_unstable();

    // `bfchar` sections are limited to 100 entries each.
    for chunk in pairs.chunks(100) {
        out.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (cid, uni) in chunk {
            if *uni <= 0xFFFF {
                out.push_str(&format!("<{cid:04X}> <{uni:04X}>\n"));
            } else {
                // Characters outside the BMP are written as UTF-16 surrogate pairs.
                let v = *uni - 0x1_0000;
                let hi = 0xD800 + ((v >> 10) as u16);
                let lo = 0xDC00 + ((v & 0x3FF) as u16);
                out.push_str(&format!("<{cid:04X}> <{hi:04X}{lo:04X}>\n"));
            }
        }
        out.push_str("endbfchar\n");
    }

    out.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    out.into_bytes()
}