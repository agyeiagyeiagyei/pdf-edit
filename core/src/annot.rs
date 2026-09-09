//! PDF annotation objects: markup (highlight/underline/strikeout), ink, and
//! text notes, written as real /Annots entries. Coordinates here are PDF user
//! space: bottom-left origin, y up, in points.

use lopdf::{Document, Object, Stream, dictionary};

#[derive(Clone, Debug, PartialEq)]
pub struct AnnotInfo {
    pub subtype: String,
    pub rect: [f32; 4],
    pub quads: Vec<[f32; 4]>,
    pub color: [f32; 3],
    pub contents: String,
    pub ink: Vec<Vec<(f32, f32)>>,
}

fn num(o: &Object) -> f32 {
    match o {
        Object::Integer(i) => *i as f32,
        Object::Real(r) => *r,
        _ => 0.0,
    }
}

fn color_obj(c: [f32; 3]) -> Object {
    Object::Array(c.iter().map(|v| Object::Real(*v)).collect())
}

fn pdf_string(s: &str) -> Object {
    if s.is_ascii() {
        Object::string_literal(s)
    } else {
        let mut bytes: Vec<u8> = vec![0xFE, 0xFF];
        for unit in s.encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        Object::String(bytes, lopdf::StringFormat::Literal)
    }
}

fn quad_points(quads: &[[f32; 4]]) -> (Object, [f32; 4]) {
    // input quads: [x, y, w, h] bottom-left origin. Spec order: TL TR BL BR.
    let mut flat = Vec::with_capacity(quads.len() * 8);
    let mut bb = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for &[x, y, w, h] in quads {
        for (px, py) in [(x, y + h), (x + w, y + h), (x, y), (x + w, y)] {
            flat.push(Object::Real(px));
            flat.push(Object::Real(py));
        }
        bb[0] = bb[0].min(x);
        bb[1] = bb[1].min(y);
        bb[2] = bb[2].max(x + w);
        bb[3] = bb[3].max(y + h);
    }
    (Object::Array(flat), bb)
}

fn rect_obj(bb: [f32; 4]) -> Object {
    Object::Array(bb.iter().map(|v| Object::Real(*v)).collect())
}

fn add_annot(doc: &mut Document, page: u32, annot: lopdf::Dictionary) -> Result<(), lopdf::Error> {
    let pages = doc.get_pages();
    let Some(&page_id) = pages.get(&page) else {
        return Err(lopdf::Error::PageNumberNotFound(page));
    };
    let annot_id = doc.add_object(Object::Dictionary(annot));
    let page_dict = doc.get_dictionary_mut(page_id)?;
    match page_dict.get_mut(b"Annots") {
        Ok(Object::Array(arr)) => arr.push(Object::Reference(annot_id)),
        _ => {
            page_dict.set("Annots", Object::Array(vec![Object::Reference(annot_id)]));
        }
    }
    Ok(())
}

pub fn add_markup(
    doc: &mut Document,
    page: u32,
    subtype: &str, // "Highlight" | "Underline" | "StrikeOut"
    quads: &[[f32; 4]],
    color: [f32; 3],
    contents: &str,
) -> Result<(), lopdf::Error> {
    let (qp, bb) = quad_points(quads);
    let mut annot = dictionary! {
        "Type" => "Annot",
        "Subtype" => Object::Name(subtype.as_bytes().to_vec()),
        "Rect" => rect_obj(bb),
        "QuadPoints" => qp,
        "C" => color_obj(color),
        "F" => 4,
        "CA" => 1.0,
    };
    if !contents.is_empty() {
        annot.set("Contents", pdf_string(contents));
    }
    add_annot(doc, page, annot)
}

pub fn add_ink(
    doc: &mut Document,
    page: u32,
    strokes: &[Vec<(f32, f32)>],
    color: [f32; 3],
    width: f32,
) -> Result<(), lopdf::Error> {
    let mut bb = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for stroke in strokes {
        for &(x, y) in stroke {
            bb[0] = bb[0].min(x);
            bb[1] = bb[1].min(y);
            bb[2] = bb[2].max(x);
            bb[3] = bb[3].max(y);
        }
    }
    let pad = width + 2.0;
    bb[0] -= pad;
    bb[1] -= pad;
    bb[2] += pad;
    bb[3] += pad;

    let ink_list = Object::Array(
        strokes
            .iter()
            .map(|s| {
                Object::Array(
                    s.iter()
                        .flat_map(|&(x, y)| vec![Object::Real(x), Object::Real(y)])
                        .collect(),
                )
            })
            .collect(),
    );

    // appearance stream so the ink renders in every viewer
    let mut content = format!(
        "{} {} {} RG {} w 1 J 1 j\n",
        color[0], color[1], color[2], width
    );
    for stroke in strokes {
        if stroke.is_empty() {
            continue;
        }
        content.push_str(&format!("{} {} m\n", stroke[0].0, stroke[0].1));
        for &(x, y) in &stroke[1..] {
            content.push_str(&format!("{x} {y} l\n"));
        }
        content.push_str("S\n");
    }
    let ap_stream = Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "FormType" => 1,
            "BBox" => rect_obj(bb),
        },
        content.into_bytes(),
    );

    let annot = dictionary! {
        "Type" => "Annot",
        "Subtype" => "Ink",
        "Rect" => rect_obj(bb),
        "InkList" => ink_list,
        "C" => color_obj(color),
        "F" => 4,
        "BS" => dictionary! { "W" => width },
    };
    let pages = doc.get_pages();
    let Some(&page_id) = pages.get(&page) else {
        return Err(lopdf::Error::PageNumberNotFound(page));
    };
    let ap_id = doc.add_object(Object::Stream(ap_stream));
    let mut annot = annot;
    annot.set(
        "AP",
        Object::Dictionary(dictionary! { "N" => Object::Reference(ap_id) }),
    );
    let annot_id = doc.add_object(Object::Dictionary(annot));
    let page_dict = doc.get_dictionary_mut(page_id)?;
    match page_dict.get_mut(b"Annots") {
        Ok(Object::Array(arr)) => arr.push(Object::Reference(annot_id)),
        _ => {
            page_dict.set("Annots", Object::Array(vec![Object::Reference(annot_id)]));
        }
    }
    Ok(())
}

pub fn add_note(
    doc: &mut Document,
    page: u32,
    x: f32,
    y: f32,
    contents: &str,
) -> Result<(), lopdf::Error> {
    let annot = dictionary! {
        "Type" => "Annot",
        "Subtype" => "Text",
        "Rect" => rect_obj([x, y, x + 20.0, y + 20.0]),
        "C" => color_obj([1.0, 0.8, 0.2]),
        "F" => 4,
        "Name" => Object::Name(b"Comment".to_vec()),
        "Contents" => pdf_string(contents),
    };
    add_annot(doc, page, annot)
}

fn annot_from_dict(dict: &lopdf::Dictionary) -> AnnotInfo {
    let subtype = dict
        .get(b"Subtype")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_default();
    let rect_arr: Vec<f32> = dict
        .get(b"Rect")
        .ok()
        .and_then(|o| o.as_array().ok())
        .map(|a| a.iter().map(num).collect())
        .unwrap_or_default();
    let rect = if rect_arr.len() == 4 {
        [rect_arr[0], rect_arr[1], rect_arr[2], rect_arr[3]]
    } else {
        [0.0; 4]
    };
    let flat: Vec<f32> = dict
        .get(b"QuadPoints")
        .ok()
        .and_then(|o| o.as_array().ok())
        .map(|a| a.iter().map(num).collect())
        .unwrap_or_default();
    let mut quads = Vec::new();
    for chunk in flat.chunks_exact(8) {
        // TL TR BL BR back to [x, y, w, h]
        let (x0, y0) = (chunk[4], chunk[5]);
        quads.push([x0, y0, chunk[2] - x0, chunk[1] - y0]);
    }
    let carr: Vec<f32> = dict
        .get(b"C")
        .ok()
        .and_then(|o| o.as_array().ok())
        .map(|a| a.iter().map(num).collect())
        .unwrap_or_default();
    let color = if carr.len() == 3 {
        [carr[0], carr[1], carr[2]]
    } else {
        [1.0, 1.0, 0.0]
    };
    let contents = dict
        .get(b"Contents")
        .ok()
        .and_then(|o| match o {
            Object::String(bytes, _) => {
                if bytes.starts_with(&[0xFE, 0xFF]) {
                    let units: Vec<u16> = bytes[2..]
                        .chunks_exact(2)
                        .map(|c| u16::from_be_bytes([c[0], c[1]]))
                        .collect();
                    Some(String::from_utf16_lossy(&units))
                } else {
                    Some(String::from_utf8_lossy(bytes).into_owned())
                }
            }
            _ => None,
        })
        .unwrap_or_default();
    let ink = dict
        .get(b"InkList")
        .ok()
        .and_then(|o| o.as_array().ok())
        .map(|strokes| {
            strokes
                .iter()
                .filter_map(|s| s.as_array().ok())
                .map(|pts| {
                    pts.chunks_exact(2)
                        .map(|c| (num(&c[0]), num(&c[1])))
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default();
    AnnotInfo {
        subtype,
        rect,
        quads,
        color,
        contents,
        ink,
    }
}

pub fn list_annotations(doc: &Document, page: u32) -> Vec<AnnotInfo> {
    let pages = doc.get_pages();
    let Some(&page_id) = pages.get(&page) else {
        return Vec::new();
    };
    let Ok(page_dict) = doc.get_dictionary(page_id) else {
        return Vec::new();
    };
    let Ok(annots) = page_dict.get(b"Annots") else {
        return Vec::new();
    };
    let Ok(arr) = annots.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|o| match o {
            Object::Reference(id) => doc.get_dictionary(*id).ok(),
            Object::Dictionary(d) => Some(d),
            _ => None,
        })
        .map(annot_from_dict)
        .collect()
}

/// Remove annotation `index` (0-based, in /Annots order) from a page.
pub fn delete_annot(doc: &mut Document, page: u32, index: usize) -> Result<(), lopdf::Error> {
    let pages = doc.get_pages();
    let Some(&page_id) = pages.get(&page) else {
        return Err(lopdf::Error::PageNumberNotFound(page));
    };
    let page_dict = doc.get_dictionary_mut(page_id)?;
    if let Ok(Object::Array(arr)) = page_dict.get_mut(b"Annots") {
        if index < arr.len() {
            arr.remove(index);
        }
    }
    Ok(())
}
