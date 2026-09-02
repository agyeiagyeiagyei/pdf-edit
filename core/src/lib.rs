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

    /// Page object ids in document order (1-based position ↔ index + 1).
    pub fn page_order(&self) -> Vec<lopdf::ObjectId> {
        let pages = self.doc.get_pages();
        let mut v: Vec<_> = pages.into_iter().collect();
        v.sort_by_key(|(num, _)| *num);
        v.into_iter().map(|(_, id)| id).collect()
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

    /// Add `delta` degrees to a page's current rotation (its own /Rotate, default 0).
    pub fn rotate_page_by(&mut self, page: u32, delta: i64) -> Result<(), lopdf::Error> {
        let pages = self.doc.get_pages();
        let Some(&id) = pages.get(&page) else {
            return Err(lopdf::Error::PageNumberNotFound(page));
        };
        let dict = self.doc.get_dictionary_mut(id)?;
        let current = dict
            .get(b"Rotate")
            .ok()
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);
        dict.set("Rotate", lopdf::Object::Integer((current + delta).rem_euclid(360)));
        Ok(())
    }

    /// Move page `from` (1-based) to position `to` (1-based).
    /// Flattens the page tree onto the root Pages node, first materializing
    /// inheritable attributes (MediaBox, CropBox, Rotate, Resources) onto
    /// each page so reparenting can't change rendering.
    pub fn move_page(&mut self, from: u32, to: u32) -> Result<(), lopdf::Error> {
        let mut order: Vec<lopdf::ObjectId> = {
            let pages = self.doc.get_pages();
            let mut v: Vec<_> = pages.into_iter().collect();
            v.sort_by_key(|(num, _)| *num);
            v.into_iter().map(|(_, id)| id).collect()
        };
        if from < 1 || to < 1 || from as usize > order.len() || to as usize > order.len() {
            return Err(lopdf::Error::PageNumberNotFound(from.max(to)));
        }
        let id = order.remove((from - 1) as usize);
        order.insert((to - 1) as usize, id);
        self.flatten_page_tree(order)
    }

    fn flatten_page_tree(&mut self, order: Vec<lopdf::ObjectId>) -> Result<(), lopdf::Error> {
        const INHERITABLE: [&[u8]; 4] = [b"MediaBox", b"CropBox", b"Rotate", b"Resources"];

        // materialize inheritable attributes onto each page (borrow scope ends before mutation)
        let mut materialized: Vec<(lopdf::ObjectId, Vec<(Vec<u8>, lopdf::Object)>)> = Vec::new();
        for &page_id in &order {
            let mut found: Vec<(Vec<u8>, lopdf::Object)> = Vec::new();
            let mut current = Some(page_id);
            while let Some(id) = current {
                let dict = self.doc.get_dictionary(id)?;
                for key in INHERITABLE {
                    if !found.iter().any(|(k, _)| k == key) {
                        if let Ok(val) = dict.get(key) {
                            found.push((key.to_vec(), val.clone()));
                        }
                    }
                }
                current = dict.get(b"Parent").ok().and_then(|o| o.as_reference().ok());
            }
            materialized.push((page_id, found));
        }

        let catalog = self.doc.catalog()?;
        let pages_id = catalog.get(b"Pages")?.as_reference()?;

        for (page_id, attrs) in materialized {
            let dict = self.doc.get_dictionary_mut(page_id)?;
            for (key, val) in attrs {
                dict.set(key, val);
            }
            dict.set("Parent", lopdf::Object::Reference(pages_id));
        }

        let kids: Vec<lopdf::Object> = order.iter().map(|&id| id.into()).collect();
        let count = kids.len() as i64;
        let pages_dict = self.doc.get_dictionary_mut(pages_id)?;
        pages_dict.set("Kids", lopdf::Object::Array(kids));
        pages_dict.set("Count", lopdf::Object::Integer(count));
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

    fn make_three_page_pdf() -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let mut kids = vec![];
        for _ in 0..3 {
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            });
            kids.push(page_id.into());
        }
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids,
                "Count" => 3,
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
    fn move_page_reorders() {
        let bytes = make_three_page_pdf();
        let mut doc = PdfDoc::load(&bytes).unwrap();
        let before = doc.page_order();
        doc.move_page(1, 3).unwrap();
        let after = doc.page_order();
        assert_eq!(after, vec![before[1], before[2], before[0]]);
        // saved document still parses with same order
        let saved = doc.save().unwrap();
        let reloaded = PdfDoc::load(&saved).unwrap();
        assert_eq!(reloaded.page_order(), after);
        assert!(doc.move_page(0, 1).is_err());
        assert!(doc.move_page(1, 4).is_err());
    }

    #[test]
    fn move_page_materializes_inherited_mediabox() {
        // nested tree: root Pages → [nodeA → [p1], nodeB(mediabox) → [p2]]
        let mut doc = Document::with_version("1.5");
        let root_id = doc.new_object_id();
        let node_a = doc.new_object_id();
        let node_b = doc.new_object_id();
        let p1 = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => node_a,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        let p2 = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => node_b,
        });
        doc.objects.insert(
            node_a,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Parent" => root_id,
                "Kids" => vec![p1.into()], "Count" => 1,
            }),
        );
        doc.objects.insert(
            node_b,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Parent" => root_id,
                "Kids" => vec![p2.into()], "Count" => 1,
                "MediaBox" => vec![0.into(), 0.into(), 300.into(), 400.into()],
            }),
        );
        doc.objects.insert(
            root_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![node_a.into(), node_b.into()],
                "Count" => 2,
            }),
        );
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => root_id });
        doc.trailer.set("Root", catalog_id);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();

        let mut doc = PdfDoc::load(&out).unwrap();
        doc.move_page(2, 1).unwrap();
        let saved = doc.save().unwrap();
        let reloaded = Document::load_mem(&saved).unwrap();
        // p2 must now carry the mediabox it previously inherited from node_b
        let dict = reloaded.get_dictionary(p2).unwrap();
        let mb = dict.get(b"MediaBox").unwrap().as_array().unwrap();
        assert_eq!(mb[2].as_i64().unwrap(), 300);
    }
}
