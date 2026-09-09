use lopdf::{Document, Object, Stream, dictionary};
use pdf_edit_core::PdfDoc;
use pdf_edit_core::text::search;

fn make_text_pdf(text: &str) -> Vec<u8> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let content = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
        "Contents" => content_id,
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

#[test]
fn markup_roundtrip() {
    let bytes = make_text_pdf("Hello World");
    let mut doc = PdfDoc::load(&bytes).unwrap();
    doc.add_markup(1, "Highlight", &[[70.0, 715.0, 80.0, 14.0]], [1.0, 1.0, 0.0], "note text")
        .unwrap();
    let annots = doc.annotations(1);
    assert_eq!(annots.len(), 1);
    assert_eq!(annots[0].subtype, "Highlight");
    assert_eq!(annots[0].quads, vec![[70.0, 715.0, 80.0, 14.0]]);
    assert_eq!(annots[0].contents, "note text");

    let saved = doc.save().unwrap();
    let reloaded = PdfDoc::load(&saved).unwrap();
    assert_eq!(reloaded.annotations(1).len(), 1);
}

#[test]
fn ink_and_note_roundtrip() {
    let bytes = make_text_pdf("Hello World");
    let mut doc = PdfDoc::load(&bytes).unwrap();
    doc.add_ink(
        1,
        &[vec![(100.0, 700.0), (120.0, 690.0), (140.0, 705.0)]],
        [1.0, 0.0, 0.0],
        2.0,
    )
    .unwrap();
    doc.add_note(1, 300.0, 600.0, "Grüße — non-ascii").unwrap();
    let saved = doc.save().unwrap();
    let reloaded = PdfDoc::load(&saved).unwrap();
    let annots = reloaded.annotations(1);
    assert_eq!(annots.len(), 2);
    let ink = annots.iter().find(|a| a.subtype == "Ink").unwrap();
    assert_eq!(ink.ink.len(), 1);
    assert_eq!(ink.ink[0].len(), 3);
    let note = annots.iter().find(|a| a.subtype == "Text").unwrap();
    assert_eq!(note.contents, "Grüße — non-ascii");
}

#[test]
fn replace_covers_and_inserts_text() {
    let bytes = make_text_pdf("Hello World");
    let hits = search(&bytes, "World").unwrap();
    assert_eq!(hits.len(), 1, "expected one hit, got {hits:?}");
    let mut doc = PdfDoc::load(&bytes).unwrap();
    let (in_place, overlay) = doc.find_and_replace(&bytes, "World", &hits, "Moon").unwrap();
    assert_eq!((in_place, overlay), (1, 0));
    let saved = doc.save().unwrap();
    // in-place rewrite: extraction shows exactly the replaced text
    let spans = pdf_edit_core::text::text_spans(&saved, 0).unwrap();
    let all: String = spans.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(all, "Hello Moon");
}
