//! Validate physical page identity and declared counts before a repaired PDF is admitted.
use lopdf::{Document, ObjectId};
use std::collections::BTreeSet;
enum Work {
    Visit(ObjectId, Option<ObjectId>, usize),
    Count(usize, usize),
}
pub(super) fn pages(doc: &Document) -> Result<Vec<ObjectId>, String> {
    let root = doc
        .catalog()
        .and_then(|catalog| catalog.get(b"Pages"))
        .and_then(|object| object.as_reference())
        .map_err(|error| format!("Invalid PDF page root: {error}"))?;
    let mut work = vec![Work::Visit(root, None, 0)];
    let mut seen = BTreeSet::new();
    let mut pages = Vec::new();
    while let Some(next) = work.pop() {
        match next {
            Work::Count(before, expected) => {
                if pages.len().checked_sub(before) != Some(expected) {
                    return Err("PDF page-tree count disagrees with its physical pages".into());
                }
            }
            Work::Visit(id, parent, depth) => {
                if depth > 128 || seen.len() >= 8192 || !seen.insert(id) {
                    return Err("PDF page tree is cyclic, duplicated or too deeply nested".into());
                }
                let node = doc.get_dictionary(id).map_err(|error| format!("Cannot read PDF page tree: {error}"))?;
                if let Some(parent) = parent
                    && node.get(b"Parent").and_then(|object| object.as_reference()).ok() != Some(parent)
                {
                    return Err("PDF page has a missing or inconsistent parent identity".into());
                }
                match node.get(b"Type").and_then(|object| object.as_name()).map_err(|error| format!("Invalid PDF page type: {error}"))? {
                    b"Page" => {
                        if parent.is_none() || pages.len() >= 4096 {
                            return Err("PDF requires a bounded Pages root and at most 4096 pages".into());
                        }
                        pages.push(id);
                    }
                    b"Pages" => {
                        let count = node
                            .get(b"Count")
                            .and_then(|object| object.as_i64())
                            .ok()
                            .and_then(|count| usize::try_from(count).ok())
                            .filter(|count| (1..=4096).contains(count))
                            .ok_or("PDF page count must be 1–4096")?;
                        let children = node.get(b"Kids").and_then(|object| object.as_array()).map_err(|error| format!("Invalid PDF page children: {error}"))?;
                        if children.is_empty() || children.len() > 4096 || work.len().saturating_add(children.len()).saturating_add(1) > 16_384 {
                            return Err("PDF page-tree traversal exceeds its budget".into());
                        }
                        work.push(Work::Count(pages.len(), count));
                        for child in children.iter().rev() {
                            let id_child = child.as_reference().map_err(|_| "PDF page children must have distinct indirect identities")?;
                            work.push(Work::Visit(id_child, Some(id), depth.saturating_add(1)));
                        }
                    }
                    _ => return Err("Unsupported node in the PDF physical page tree".into()),
                }
            }
        }
    }
    if pages.is_empty() {
        return Err("PDF contains no physical pages".into());
    }
    Ok(pages)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Document {
        let mut doc = Document::new();
        doc.objects.insert((1, 0), lopdf::Object::Dictionary(lopdf::dictionary! {"Type"=>"Catalog","Pages"=>(2,0)}));
        doc.objects.insert((2, 0), lopdf::Object::Dictionary(lopdf::dictionary! {"Type"=>"Pages","Count"=>1,"Kids"=>vec![lopdf::Object::Reference((3,0))]}));
        doc.objects.insert((3, 0), lopdf::Object::Dictionary(lopdf::dictionary! {"Type"=>"Page","Parent"=>(2,0)}));
        doc.trailer.set("Root", (1, 0));
        doc
    }
    #[test]
    fn count_cycles_duplicates_and_wrong_parent_fail_without_a_recursive_walk() {
        let mut doc = fixture();
        assert_eq!(pages(&doc).unwrap(), [(3, 0)]);
        doc.get_dictionary_mut((2, 0)).unwrap().set("Count", 2);
        assert!(pages(&doc).is_err());
        doc.get_dictionary_mut((2, 0)).unwrap().set("Kids", vec![lopdf::Object::Reference((3, 0)), lopdf::Object::Reference((3, 0))]);
        assert!(pages(&doc).is_err());
        doc.get_dictionary_mut((2, 0)).unwrap().set("Kids", vec![lopdf::Object::Reference((2, 0))]);
        assert!(pages(&doc).is_err());
        let mut doc = fixture();
        doc.get_dictionary_mut((3, 0)).unwrap().set("Parent", (99, 0));
        assert!(pages(&doc).is_err());
    }
}
