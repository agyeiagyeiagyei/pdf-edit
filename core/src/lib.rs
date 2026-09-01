//! Core PDF manipulation for pdf-edit. Pure Rust, testable natively.

use lopdf::Document;

pub struct PdfDoc {
    doc: Document,
}

impl PdfDoc {
    pub fn load(data: &[u8]) -> Result<Self, lopdf::Error> {
        Ok(Self {
            doc: Document::load_mem(data)?,
        })
    }

    pub fn page_count(&self) -> usize {
        self.doc.get_pages().len()
    }

    pub fn save(&mut self) -> Result<Vec<u8>, lopdf::Error> {
        let mut out = Vec::new();
        self.doc.save_to(&mut out)?;
        Ok(out)
    }

    pub fn delete_page(&mut self, page: u32) {
        self.doc.delete_pages(&[page]);
    }

    pub fn rotate_page(&mut self, page: u32, degrees: i64) -> Result<(), lopdf::Error> {
        let pages = self.doc.get_pages();
        let Some(&id) = pages.get(&page) else {
            return Err(lopdf::Error::PageNumberNotFound(page));
        };
        let dict = self.doc.get_dictionary_mut(id)?;
        dict.set("Rotate", lopdf::Object::Integer(degrees));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Document, Object, Stream};

    fn make_test_pdf() -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        let content_id = doc.add_object(Stream::new(dictionary! {}, vec![]));
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .set("Contents", content_id);
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
    fn roundtrip_preserves_page_count() {
        let bytes = make_test_pdf();
        let mut doc = PdfDoc::load(&bytes).unwrap();
        assert_eq!(doc.page_count(), 1);
        let saved = doc.save().unwrap();
        let reloaded = PdfDoc::load(&saved).unwrap();
        assert_eq!(reloaded.page_count(), 1);
    }

    #[test]
    fn delete_page_works() {
        let bytes = make_test_pdf();
        let mut doc = PdfDoc::load(&bytes).unwrap();
        doc.delete_page(1);
        assert_eq!(doc.page_count(), 0);
    }

    #[test]
    fn rotate_page_sets_rotation() {
        let bytes = make_test_pdf();
        let mut doc = PdfDoc::load(&bytes).unwrap();
        doc.rotate_page(1, 90).unwrap();
        assert!(doc.rotate_page(99, 90).is_err());
    }
}
