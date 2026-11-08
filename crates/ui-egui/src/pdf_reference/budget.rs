//! Conservative retained-object accounting for PDF loading, separate from the stream/output caps.
//! This bounds admitted object storage; it is not an operating-system RSS or parser-scratch limit.
use lopdf::{Dictionary, Object, ObjectId};
use std::cell::RefCell;

const MAX_BYTES: usize = 128 * 1024 * 1024;
const MAX_NODES: usize = 1_000_000;
const MAX_OBJECTS: usize = 65_536;
const MAX_PENDING: usize = 65_536;
#[derive(Default)]
struct Budget {
    bytes: usize,
    nodes: usize,
    objects: usize,
    exhausted: bool,
}
thread_local! { static LOAD: RefCell<Budget> = RefCell::new(Budget::default()); }
pub(super) fn reset() -> Result<(), String> {
    LOAD.with(|budget| {
        let mut budget = budget.try_borrow_mut().map_err(|_| "PDF loading budget is already in use")?;
        *budget = Budget::default();
        Ok(())
    })
}
pub(super) fn exhausted() -> bool {
    LOAD.with(|budget| budget.try_borrow().map_or(true, |budget| budget.exhausted))
}
impl Budget {
    fn charge(&mut self, bytes: usize) -> bool {
        let Some(total) = self.bytes.checked_add(bytes).filter(|total| *total <= MAX_BYTES) else { return false };
        self.bytes = total;
        true
    }
    fn dictionary<'a>(&mut self, dictionary: &'a Dictionary, depth: usize, pending: &mut Vec<(&'a Object, usize)>) -> bool {
        if dictionary.len() > MAX_PENDING || pending.len().saturating_add(dictionary.len()) > MAX_PENDING || !self.charge(dictionary.len().saturating_mul(256))
        {
            return false;
        }
        for (key, value) in dictionary.iter() {
            if !self.charge(key.capacity()) {
                return false;
            }
            pending.push((value, depth));
        }
        true
    }
    fn object(&mut self, object: &Object) -> bool {
        self.objects = self.objects.saturating_add(1);
        if self.objects > MAX_OBJECTS {
            return false;
        }
        let mut pending = vec![(object, 0_usize)];
        while let Some((object, depth)) = pending.pop() {
            self.nodes = self.nodes.saturating_add(1);
            if depth > 128 || self.nodes > MAX_NODES || !self.charge(std::mem::size_of::<Object>()) {
                return false;
            }
            let accepted = match object {
                Object::Name(bytes) | Object::String(bytes, _) => self.charge(bytes.capacity()),
                Object::Array(values) => {
                    if values.len() > MAX_PENDING
                        || pending.len().saturating_add(values.len()) > MAX_PENDING
                        || !self.charge(values.capacity().saturating_mul(std::mem::size_of::<Object>()))
                    {
                        return false;
                    }
                    pending.extend(values.iter().map(|value| (value, depth.saturating_add(1))));
                    true
                }
                Object::Dictionary(dictionary) => self.dictionary(dictionary, depth.saturating_add(1), &mut pending),
                Object::Stream(stream) => self.charge(stream.content.capacity()) && self.dictionary(&stream.dict, depth.saturating_add(1), &mut pending),
                _ => true,
            };
            if !accepted {
                return false;
            }
        }
        true
    }
}
pub(super) fn filter(id: ObjectId, object: &mut Object) -> Option<(ObjectId, Object)> {
    let accepted = LOAD.with(|budget| {
        let Ok(mut budget) = budget.try_borrow_mut() else { return false };
        if budget.exhausted {
            return false;
        }
        if !budget.object(object) {
            budget.exhausted = true;
            return false;
        }
        true
    });
    accepted.then(|| (id, object.clone()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn budget_rejects_oversized_storage_arrays_and_depth_without_recursive_walks() {
        let mut budget = Budget { bytes: MAX_BYTES, ..Default::default() };
        assert!(!budget.object(&Object::Integer(1)));
        let mut budget = Budget::default();
        assert!(!budget.object(&Object::Array(vec![Object::Null; MAX_PENDING + 1])));
        let mut object = Object::Null;
        for _ in 0..130 {
            object = Object::Array(vec![object]);
        }
        assert!(!Budget::default().object(&object));
    }
    #[test]
    fn exhausted_filter_cannot_publish_a_partially_filtered_document() {
        reset().unwrap();
        LOAD.with(|budget| budget.borrow_mut().objects = MAX_OBJECTS);
        let mut object = Object::Integer(4);
        assert!(filter((1, 0), &mut object).is_none());
        assert!(exhausted());
        let mut small = Object::Null;
        assert!(filter((2, 0), &mut small).is_none());
        reset().unwrap();
        assert!(!exhausted());
        assert!(filter((3, 0), &mut small).is_some());
    }
}
