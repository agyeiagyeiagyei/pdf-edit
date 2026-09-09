//! Find & replace.
//!
//! Primary path is in-place: content-stream text operators (Tj / TJ) are
//! decoded via each font's ToUnicode CMap (or WinAnsi for simple fonts), the
//! match is rewritten inside the string operand, and the stream is rebuilt.
//! This keeps the text layer, copy, and re-search correct.
//!
//! Fallback is overlay: when the active font cannot encode the replacement
//! (e.g. a subset font missing the needed glyphs), the hit's spans are
//! covered with white and the replacement is drawn with an embedded subset
//! font (Liberation Sans, OFL). The original text then stays underneath —
//! visual replacement only, not redaction.
//!
//! Geometry (SearchHit spans) is display space: top-left origin, y down.

use crate::text::SearchHit;
use lopdf::content::Content;
use lopdf::{Document, Object, Stream, dictionary};
use std::collections::HashMap;

static FONT_TTF: &[u8] = include_bytes!("../assets/OverlaySans.ttf");

// ---------------------------------------------------------------- cp1252 ---

fn to_cp1252(c: char) -> Option<u8> {
    match c {
        '\u{20}'..='\u{7E}' => Some(c as u8),
        '\u{A0}'..='\u{FF}' => Some(c as u8),
        '\u{2013}' => Some(0x96),
        '\u{2014}' => Some(0x97),
        '\u{2018}' => Some(0x91),
        '\u{2019}' => Some(0x92),
        '\u{201A}' => Some(0x82),
        '\u{201C}' => Some(0x93),
        '\u{201D}' => Some(0x94),
        '\u{201E}' => Some(0x84),
        '\u{2026}' => Some(0x85),
        _ => None,
    }
}

fn cp1252_to_unicode(code: u8) -> Option<char> {
    match code {
        0x20..=0x7E => Some(code as char),
        0xA0..=0xFF => Some(code as char),
        0x96 => Some('\u{2013}'),
        0x97 => Some('\u{2014}'),
        0x91 => Some('\u{2018}'),
        0x92 => Some('\u{2019}'),
        0x82 => Some('\u{201A}'),
        0x93 => Some('\u{201C}'),
        0x94 => Some('\u{201D}'),
        0x84 => Some('\u{201E}'),
        0x85 => Some('\u{2026}'),
        _ => None,
    }
}

// -------------------------------------------------------- ToUnicode CMaps ---

/// Bidirectional char-code map for one font.
struct CodeMap {
    code_len: usize, // bytes per char code (1 or 2)
    decode: HashMap<u32, String>,
    encode: HashMap<char, u32>,
}

impl CodeMap {
    fn decode_bytes(&self, bytes: &[u8]) -> Option<String> {
        let mut out = String::new();
        for chunk in bytes.chunks(self.code_len) {
            if chunk.len() != self.code_len {
                return None;
            }
            let code = if self.code_len == 1 {
                chunk[0] as u32
            } else {
                u16::from_be_bytes([chunk[0], chunk[1]]) as u32
            };
            match self.decode.get(&code) {
                Some(s) => out.push_str(s),
                None => return None,
            }
        }
        Some(out)
    }

    fn encode_str(&self, text: &str) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        for c in text.chars() {
            let code = *self.encode.get(&c)?;
            if self.code_len == 1 {
                out.push(code as u8);
            } else {
                out.extend_from_slice(&(code as u16).to_be_bytes());
            }
        }
        Some(out)
    }
}

/// Parse a ToUnicode CMap stream (bfchar + bfrange, hex tokens).
fn parse_tounicode(data: &[u8]) -> Option<CodeMap> {
    let text = String::from_utf8_lossy(data).into_owned();
    let mut decode = HashMap::new();
    let mut encode = HashMap::new();
    let mut code_len = 1usize;

    let hex_val = |tok: &str| -> Option<Vec<u8>> {
        let t = tok.trim().trim_start_matches('<').trim_end_matches('>');
        if t.is_empty() || t.len() % 2 != 0 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        Some(
            (0..t.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&t[i..i + 2], 16).unwrap())
                .collect(),
        )
    };
    let utf16 = |bytes: &[u8]| -> String {
        if bytes.len() % 2 != 0 {
            return String::new();
        }
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    };

    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i] {
            "begincodespacerange" => {
                // preceding two tokens are <lo> <hi>
                if i >= 2 {
                    if let Some(hi) = hex_val(tokens[i - 1]) {
                        code_len = hi.len().max(1);
                    }
                }
            }
            "beginbfchar" => {
                let n: usize = tokens[i - 1].parse().unwrap_or(0);
                for k in 0..n {
                    let src = tokens.get(i + 1 + k * 2)?;
                    let dst = tokens.get(i + 2 + k * 2)?;
                    if let (Some(s), Some(d)) = (hex_val(src), hex_val(dst)) {
                        let code = s.iter().fold(0u32, |a, &b| (a << 8) | b as u32);
                        let uni = utf16(&d);
                        if !uni.is_empty() {
                            decode.insert(code, uni.clone());
                            code_len = code_len.max(s.len());
                            if uni.chars().count() == 1 {
                                encode.insert(uni.chars().next().unwrap(), code);
                            }
                        }
                    }
                }
                i += 1 + n * 2;
            }
            "beginbfrange" => {
                let n: usize = tokens[i - 1].parse().unwrap_or(0);
                let mut j = i + 1;
                for _ in 0..n {
                    let lo = hex_val(tokens.get(j)?)?;
                    let hi = hex_val(tokens.get(j + 1)?)?;
                    let lo_v = lo.iter().fold(0u32, |a, &b| (a << 8) | b as u32);
                    let hi_v = hi.iter().fold(0u32, |a, &b| (a << 8) | b as u32);
                    code_len = code_len.max(lo.len());
                    let dst_tok = tokens.get(j + 2)?;
                    if dst_tok.starts_with('[') {
                        // array form: <lo> <hi> [<d1> <d2> ...]
                        let mut k = j + 2;
                        let mut code = lo_v;
                        while k < tokens.len() && !tokens[k].ends_with(']') {
                            let cleaned = tokens[k].trim_start_matches('[');
                            if let Some(d) = hex_val(cleaned) {
                                let uni = utf16(&d);
                                if !uni.is_empty() {
                                    decode.insert(code, uni.clone());
                                    if uni.chars().count() == 1 {
                                        encode.insert(uni.chars().next().unwrap(), code);
                                    }
                                }
                            }
                            code += 1;
                            k += 1;
                        }
                        if k < tokens.len() {
                            let cleaned = tokens[k].trim_end_matches(']');
                            if let Some(d) = hex_val(cleaned) {
                                let uni = utf16(&d);
                                if !uni.is_empty() {
                                    decode.insert(code, uni.clone());
                                    if uni.chars().count() == 1 {
                                        encode.insert(uni.chars().next().unwrap(), code);
                                    }
                                }
                            }
                        }
                        j = k + 1;
                    } else if let Some(d0) = hex_val(dst_tok) {
                        // sequential form: <lo> <hi> <dstStart>
                        let mut dst_units: Vec<u16> = d0
                            .chunks_exact(2)
                            .map(|c| u16::from_be_bytes([c[0], c[1]]))
                            .collect();
                        let Some(&last0) = dst_units.last() else {
                            j += 3;
                            continue;
                        };
                        for (off, code) in (lo_v..=hi_v).enumerate() {
                            let mut units = dst_units.clone();
                            let last = units.last_mut().unwrap();
                            *last = last0 + off as u16;
                            let uni = String::from_utf16_lossy(&units);
                            decode.insert(code, uni.clone());
                            if uni.chars().count() == 1 {
                                encode.insert(uni.chars().next().unwrap(), code);
                            }
                        }
                        dst_units.clear();
                        j += 3;
                    } else {
                        j += 3;
                    }
                }
                i = j;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    if decode.is_empty() {
        return None;
    }
    Some(CodeMap {
        code_len,
        decode,
        encode,
    })
}

/// WinAnsi map for simple fonts without a ToUnicode.
fn winansi_map() -> CodeMap {
    let mut decode = HashMap::new();
    let mut encode = HashMap::new();
    for code in 0u8..=255 {
        if let Some(c) = cp1252_to_unicode(code) {
            decode.insert(code as u32, c.to_string());
            encode.insert(c, code as u32);
        }
    }
    CodeMap {
        code_len: 1,
        decode,
        encode,
    }
}

/// Per-font char maps for a page's /Resources /Font entries.
fn page_code_maps(doc: &Document, page_id: lopdf::ObjectId) -> HashMap<String, CodeMap> {
    let mut maps = HashMap::new();
    // walk up for inherited Resources
    let mut current = Some(page_id);
    let mut resources: Option<lopdf::Dictionary> = None;
    while let Some(id) = current {
        if let Ok(dict) = doc.get_dictionary(id) {
            if let Ok(res) = dict.get(b"Resources") {
                resources = match res {
                    Object::Dictionary(d) => Some(d.clone()),
                    Object::Reference(r) => doc.get_dictionary(*r).ok().cloned(),
                    _ => None,
                };
                if resources.is_some() {
                    break;
                }
            }
            current = dict.get(b"Parent").ok().and_then(|o| o.as_reference().ok());
        } else {
            break;
        }
    }
    let Some(resources) = resources else { return maps };
    let font_dict = resources
        .get(b"Font")
        .ok()
        .and_then(|o| match o {
            Object::Dictionary(d) => Some(d.clone()),
            Object::Reference(r) => doc.get_dictionary(*r).ok().cloned(),
            _ => None,
        })
        .unwrap_or_else(|| dictionary! {});
    for (name, font_ref) in font_dict.iter() {
        let font = match font_ref {
            Object::Dictionary(d) => Some(d.clone()),
            Object::Reference(r) => doc.get_dictionary(*r).ok().cloned(),
            _ => None,
        };
        let Some(font) = font else { continue };
        let name = String::from_utf8_lossy(name).into_owned();
        // ToUnicode first
        if let Ok(tu) = font.get(b"ToUnicode") {
            let stream = match tu {
                Object::Stream(s) => Some(s.clone()),
                Object::Reference(r) => doc
                    .get_object(*r)
                    .ok()
                    .and_then(|o| o.as_stream().ok().cloned()),
                _ => None,
            };
            if let Some(stream) = stream {
                let data = stream.decompressed_content().unwrap_or(stream.content.clone());
                if let Some(map) = parse_tounicode(&data) {
                    maps.insert(name, map);
                    continue;
                }
            }
        }
        // simple fonts with WinAnsiEncoding
        let is_simple = font
            .get(b"Subtype")
            .ok()
            .and_then(|o| o.as_name().ok())
            .map(|n| n != b"Type0")
            .unwrap_or(false);
        if is_simple {
            let enc_is_winansi = match font.get(b"Encoding") {
                Ok(Object::Name(n)) => n == b"WinAnsiEncoding",
                Err(_) => true, // default for standard-14 simple fonts is close enough
                _ => false,
            };
            if enc_is_winansi {
                maps.insert(name, winansi_map());
            }
        }
    }
    maps
}

// --------------------------------------------------------------- overlay ---

fn pdf_escape(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &b in bytes {
        match b {
            b'(' | b')' | b'\\' => {
                out.push('\\');
                out.push(b as char);
            }
            0x20..=0x7E => out.push(b as char),
            _ => out.push_str(&format!("\\{:03o}", b)),
        }
    }
    out
}

struct FontMetrics {
    widths: Vec<f32>,
}

fn font_metrics() -> FontMetrics {
    let face = ttf_parser::Face::parse(FONT_TTF, 0).expect("overlay font parses");
    let upem = face.units_per_em() as f32;
    let mut widths = vec![0.0f32; 256];
    for code in 0u8..=255 {
        let adv = cp1252_to_unicode(code)
            .and_then(|c| face.glyph_index(c))
            .and_then(|gid| face.glyph_hor_advance(gid))
            .map(|a| a as f32 / upem * 1000.0)
            .unwrap_or(500.0);
        widths[code as usize] = adv;
    }
    FontMetrics { widths }
}

fn embed_font(
    doc: &mut Document,
    page_id: lopdf::ObjectId,
    metrics: &FontMetrics,
) -> Result<String, lopdf::Error> {
    let font_file_id = doc.add_object(Object::Stream(Stream::new(
        dictionary! { "Length1" => Object::Integer(FONT_TTF.len() as i64) },
        FONT_TTF.to_vec(),
    )));
    let face =
        ttf_parser::Face::parse(FONT_TTF, 0).map_err(|_| lopdf::Error::Unimplemented("font"))?;
    let bbox = face.global_bounding_box();
    let descriptor_id = doc.add_object(Object::Dictionary(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => "PdfEditOverlaySans",
        "Flags" => 32,
        "FontBBox" => vec![
            Object::Integer(bbox.x_min as i64),
            Object::Integer(bbox.y_min as i64),
            Object::Integer(bbox.x_max as i64),
            Object::Integer(bbox.y_max as i64),
        ],
        "Ascent" => face.ascender() as i64,
        "Descent" => face.descender() as i64,
        "CapHeight" => face.capital_height().unwrap_or(700) as i64,
        "StemV" => 80,
        "ItalicAngle" => 0,
        "FontFile2" => Object::Reference(font_file_id),
    }));

    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /PdfEditOverlay def\n/CMapType 2 def\n1 begincodespacerange\n<00> <FF>\nendcodespacerange\n",
    );
    let entries: Vec<(u8, char)> = (0u8..=255)
        .filter_map(|c| cp1252_to_unicode(c).map(|u| (c, u)))
        .collect();
    for chunk in entries.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (code, uni) in chunk {
            cmap.push_str(&format!("<{:02X}> <{:04X}>\n", code, *uni as u32));
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let touni_id = doc.add_object(Object::Stream(Stream::new(dictionary! {}, cmap.into_bytes())));

    let widths: Vec<Object> = metrics.widths[32..=255]
        .iter()
        .map(|w| Object::Integer(*w as i64))
        .collect();
    let font_id = doc.add_object(Object::Dictionary(dictionary! {
        "Type" => "Font",
        "Subtype" => "TrueType",
        "BaseFont" => "PdfEditOverlaySans",
        "Encoding" => "WinAnsiEncoding",
        "FirstChar" => 32,
        "LastChar" => 255,
        "Widths" => Object::Array(widths),
        "FontDescriptor" => Object::Reference(descriptor_id),
        "ToUnicode" => Object::Reference(touni_id),
    }));

    let mut resources = {
        let mut found: Option<lopdf::Dictionary> = None;
        let mut current = Some(page_id);
        while let Some(id) = current {
            let dict = doc.get_dictionary(id)?;
            if let Ok(res) = dict.get(b"Resources") {
                let rd = match res {
                    Object::Dictionary(d) => Some(d.clone()),
                    Object::Reference(r) => doc.get_dictionary(*r).ok().cloned(),
                    _ => None,
                };
                if let Some(rd) = rd {
                    found = Some(rd);
                    break;
                }
            }
            current = dict.get(b"Parent").ok().and_then(|o| o.as_reference().ok());
        }
        found.unwrap_or_else(|| dictionary! {})
    };
    let mut font_dict = resources
        .get(b"Font")
        .ok()
        .and_then(|o| match o {
            Object::Dictionary(d) => Some(d.clone()),
            Object::Reference(r) => doc.get_dictionary(*r).ok().cloned(),
            _ => None,
        })
        .unwrap_or_else(|| dictionary! {});
    let mut name = "XZovl".to_string();
    let mut i = 1;
    while font_dict.has(name.as_bytes()) {
        name = format!("XZovl{i}");
        i += 1;
    }
    font_dict.set(name.as_bytes(), Object::Reference(font_id));
    resources.set("Font", Object::Dictionary(font_dict));
    doc.get_dictionary_mut(page_id)?
        .set("Resources", Object::Dictionary(resources));
    Ok(name)
}

fn append_content(
    doc: &mut Document,
    page_id: lopdf::ObjectId,
    bytes: Vec<u8>,
) -> Result<(), lopdf::Error> {
    let stream_id = doc.add_object(Object::Stream(Stream::new(dictionary! {}, bytes)));
    let page_dict = doc.get_dictionary_mut(page_id)?;
    match page_dict.get_mut(b"Contents") {
        Ok(Object::Array(arr)) => arr.push(Object::Reference(stream_id)),
        Ok(Object::Reference(r)) => {
            let old = *r;
            page_dict.set(
                "Contents",
                Object::Array(vec![Object::Reference(old), Object::Reference(stream_id)]),
            );
        }
        _ => {
            page_dict.set("Contents", Object::Reference(stream_id));
        }
    }
    Ok(())
}

/// Inverse display transform per 0-based page index as PDF `cm` operands.
fn display_inverse_transforms(pdf_bytes: &[u8]) -> Result<Vec<[f64; 6]>, String> {
    use hayro::hayro_interpret::hayro_syntax::Pdf;
    use hayro::hayro_interpret::util::TransformExt;
    let pdf = Pdf::new(pdf_bytes.to_vec()).map_err(|e| format!("parse: {e:?}"))?;
    Ok(pdf
        .pages()
        .iter()
        .map(|page| {
            let inv = page.initial_transform(true).to_kurbo().inverse();
            let c = inv.as_coeffs();
            [c[0], c[1], c[2], c[3], c[4], c[5]]
        })
        .collect())
}

// ------------------------------------------------------------ in-place ---

/// Replace all occurrences of `query` with `replacement` in one page's
/// content streams. Returns hits it could NOT rewrite in place (their spans,
/// for overlay fallback).
fn replace_in_page_content(
    doc: &mut Document,
    page_id: lopdf::ObjectId,
    query: &str,
    replacement: &str,
) -> Result<usize, lopdf::Error> {
    let maps = page_code_maps(doc, page_id);
    if maps.is_empty() {
        return Ok(0);
    }
    let content_bytes = doc.get_page_content(page_id);
    let mut content = Content::decode(&content_bytes)?;
    let mut current_font: Option<String> = None;
    let mut replaced = 0usize;

    for op in content.operations.iter_mut() {
        match op.operator.as_str() {
            "Tf" => {
                if let Some(Object::Name(n)) = op.operands.first() {
                    current_font = Some(String::from_utf8_lossy(n).into_owned());
                }
            }
            "Tj" | "'" | "\"" => {
                let string_operand = op.operands.iter_mut().find(|o| {
                    matches!(o, Object::String(_, _))
                });
                if let Some(Object::String(bytes, fmt)) = string_operand {
                    if let Some(font) = current_font.as_ref().and_then(|f| maps.get(f)) {
                        if let Some(text) = font.decode_bytes(bytes) {
                            if text.contains(query) {
                                if let Some(enc_repl) = font.encode_str(replacement) {
                                    let mut new_bytes = Vec::new();
                                    let mut rest = text.clone();
                                    while let Some(pos) = rest.find(query) {
                                        let (before, after) = rest.split_at(pos);
                                        let after = &after[query.len()..];
                                        new_bytes.extend(font.encode_str(before).unwrap_or_default());
                                        new_bytes.extend(&enc_repl);
                                        rest = after.to_string();
                                        replaced += 1;
                                    }
                                    new_bytes.extend(font.encode_str(&rest).unwrap_or_default());
                                    *bytes = new_bytes;
                                    *fmt = lopdf::StringFormat::Literal;
                                }
                            }
                        }
                    }
                }
            }
            "TJ" => {
                if let Some(Object::Array(arr)) = op.operands.first_mut() {
                    if let Some(font) = current_font.as_ref().and_then(|f| maps.get(f)) {
                        for el in arr.iter_mut() {
                            if let Object::String(bytes, fmt) = el {
                                if let Some(text) = font.decode_bytes(bytes) {
                                    if text.contains(query) {
                                        if let Some(enc_repl) = font.encode_str(replacement) {
                                            let mut new_bytes = Vec::new();
                                            let mut rest = text.clone();
                                            while let Some(pos) = rest.find(query) {
                                                let (before, after) = rest.split_at(pos);
                                                let after = &after[query.len()..];
                                                new_bytes.extend(
                                                    font.encode_str(before).unwrap_or_default(),
                                                );
                                                new_bytes.extend(&enc_repl);
                                                rest = after.to_string();
                                                replaced += 1;
                                            }
                                            new_bytes
                                                .extend(font.encode_str(&rest).unwrap_or_default());
                                            *bytes = new_bytes;
                                            *fmt = lopdf::StringFormat::Literal;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if replaced > 0 {
        let encoded = content.encode()?;
        let stream_id = doc.add_object(Object::Stream(Stream::new(dictionary! {}, encoded)));
        doc.get_dictionary_mut(page_id)?
            .set("Contents", Object::Reference(stream_id));
    }
    Ok(replaced)
}

// ------------------------------------------------------------- public ---

/// Replace all `hits` (which must come from searching for `query`) with
/// `replacement`. In-place where the font permits; overlay otherwise.
/// Returns (in_place_count, overlay_count).
pub fn find_and_replace(
    doc: &mut Document,
    geometry_bytes: &[u8],
    query: &str,
    hits: &[SearchHit],
    replacement: &str,
) -> Result<(usize, usize), lopdf::Error> {
    let mut by_page: std::collections::BTreeMap<usize, Vec<&SearchHit>> = Default::default();
    for h in hits {
        by_page.entry(h.page).or_default().push(h);
    }

    let mut in_place_total = 0usize;
    let mut overlay_hits: Vec<SearchHit> = Vec::new();

    for (page_index, page_hits) in &by_page {
        let page_number = (*page_index + 1) as u32;
        let pages = doc.get_pages();
        let Some(&page_id) = pages.get(&page_number) else {
            continue;
        };
        let done = replace_in_page_content(doc, page_id, query, replacement)?;
        in_place_total += done;
        if done < page_hits.len() {
            // some occurrences couldn't be rewritten in place: approximate the
            // remainder proportionally for the overlay fallback
            let remaining = page_hits.len() - done;
            overlay_hits.extend(page_hits.iter().take(remaining).map(|h| (*h).clone()));
        }
    }

    let overlay_total = overlay_replace(doc, geometry_bytes, &overlay_hits, replacement)?;
    Ok((in_place_total, overlay_total))
}

/// Overlay-replace specific hits: cover spans white, draw replacement with
/// the embedded font. Returns number of hits covered.
pub fn overlay_replace(
    doc: &mut Document,
    geometry_bytes: &[u8],
    hits: &[SearchHit],
    replacement: &str,
) -> Result<usize, lopdf::Error> {
    if hits.is_empty() {
        return Ok(0);
    }
    let transforms =
        display_inverse_transforms(geometry_bytes).map_err(|_| lopdf::Error::Unimplemented("geom"))?;
    let metrics = font_metrics();
    let enc: Vec<u8> = replacement
        .chars()
        .map(|c| to_cp1252(c).unwrap_or(b'?'))
        .collect();
    let escaped = pdf_escape(&enc);
    let nat_w_1000: f32 = enc.iter().map(|&b| metrics.widths[b as usize]).sum();

    let mut by_page: std::collections::BTreeMap<usize, Vec<&SearchHit>> = Default::default();
    for h in hits {
        by_page.entry(h.page).or_default().push(h);
    }

    let mut made = 0;
    for (page_index, page_hits) in by_page {
        let page_number = (page_index + 1) as u32;
        let pages = doc.get_pages();
        let Some(&page_id) = pages.get(&page_number) else {
            continue;
        };
        let font_name = embed_font(doc, page_id, &metrics)?;
        let [a, b, c, d, e, f] = transforms
            .get(page_index)
            .copied()
            .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

        let mut content = String::new();
        content.push_str(&format!("q {a:.6} {b:.6} {c:.6} {d:.6} {e:.6} {f:.6} cm\n"));
        for hit in &page_hits {
            for span in &hit.spans {
                content.push_str(&format!(
                    "1 1 1 rg {:.3} {:.3} {:.3} {:.3} re f\n",
                    span.x, span.y, span.w, span.h
                ));
                let size = span.h / 1.15;
                let nat_w_pt = nat_w_1000 / 1000.0 * size;
                let fitted = if nat_w_pt > span.w && nat_w_pt > 0.0 {
                    size * (span.w / nat_w_pt) * 0.98
                } else {
                    size
                };
                let baseline_y = span.y + 0.85 * size;
                content.push_str(&format!(
                    "0 0 0 rg BT /{font_name} {fitted:.3} Tf 1 0 0 1 {:.3} {:.3} Tm ({escaped}) Tj ET\n",
                    span.x, baseline_y
                ));
            }
        }
        content.push_str("Q\n");
        append_content(doc, page_id, content.into_bytes())?;
        made += page_hits.len();
    }
    Ok(made)
}
