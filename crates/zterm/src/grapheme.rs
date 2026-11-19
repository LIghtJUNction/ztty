//! Bounded storage for complete extended grapheme clusters.
//! The ordinary scalar path keeps Copy cells and does not allocate.
use crate::screen::Cell;
use alloc::{collections::BTreeMap, sync::Arc, vec, vec::Vec};

/// An individual retained cluster never grows without bound.
pub const MAX_GRAPHEME_BYTES: usize = 256;
const MAX_ENTRIES: usize = 16_384;
const MAX_TEXT_BYTES: usize = 1024 * 1024;
const ARC_HEADER: usize = 2 * core::mem::size_of::<usize>();

#[derive(Default)]
pub(crate) struct Graphemes {
    slots: Vec<Option<Arc<str>>>,
    free: Vec<u32>,
    index: BTreeMap<Arc<str>, u32>,
    bytes: usize,
}

impl Graphemes {
    pub(crate) fn get(&self, id: u32) -> Option<&str> {
        id.checked_sub(1)
            .and_then(|index| self.slots.get(index as usize))
            .and_then(Option::as_deref)
    }

    pub(crate) fn intern(&mut self, text: &str) -> Option<u32> {
        if let Some(id) = self.index.get(text) {
            return Some(*id);
        }
        let bytes = text.len().checked_add(ARC_HEADER)?;
        if text.len() > MAX_GRAPHEME_BYTES
            || self.index.len() == MAX_ENTRIES
            || self.bytes.checked_add(bytes)? > MAX_TEXT_BYTES
        {
            return None;
        }
        let text: Arc<str> = Arc::from(text);
        let index = if let Some(index) = self.free.pop() {
            self.slots[index as usize] = Some(text.clone());
            index
        } else {
            let index = self.slots.len() as u32;
            self.slots.push(Some(text.clone()));
            index
        };
        self.bytes += bytes;
        let id = index + 1;
        self.index.insert(text, id);
        Some(id)
    }

    /// Only the owning Terminal may reclaim IDs, after examining all retained
    /// primary, alternate and history cells. Live IDs are never renumbered.
    pub(crate) fn collect(&mut self, ids: impl Iterator<Item = u32>) {
        let mut live = vec![false; self.slots.len()];
        for id in ids {
            if let Some(index) = id.checked_sub(1)
                && let Some(slot) = live.get_mut(index as usize)
            {
                *slot = true;
            }
        }
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if !live[index]
                && let Some(text) = slot.take()
            {
                self.index.remove(text.as_ref());
                self.bytes -= text.len() + ARC_HEADER;
                self.free.push(index as u32);
            }
        }
    }

    pub(crate) fn chars<'a>(&'a self, cell: &Cell) -> CellChars<'a> {
        CellChars {
            text: self.get(cell.grapheme_id).map(str::chars),
            inline: if cell.width == 0 {
                ['\0'; 3]
            } else {
                [cell.ch, cell.combining[0], cell.combining[1]]
            }
            .into_iter(),
        }
    }
}

/// The complete original scalar sequence of one leading cell, with no
/// normalization and no allocation. Trailing wide cells produce no characters.
#[derive(Clone)]
pub struct CellChars<'a> {
    text: Option<core::str::Chars<'a>>,
    inline: core::array::IntoIter<char, 3>,
}

impl Iterator for CellChars<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        if let Some(text) = &mut self.text {
            text.next()
        } else {
            self.inline.find(|ch| *ch != '\0')
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_interns_clusters_and_never_reuses_a_live_id() {
        let mut pool = Graphemes::default();
        let family = "👩🏽‍💻";
        let a = pool.intern(family).unwrap();
        let b = pool.intern("🇺🇳").unwrap();
        assert_eq!(pool.intern(family), Some(a));
        pool.collect(core::iter::once(a));
        assert_eq!(pool.get(a), Some(family));
        assert_eq!(pool.get(b), None);
        let c = pool.intern("1️⃣").unwrap();
        assert_eq!(b, c);
        assert_eq!(pool.get(a), Some(family));
        assert_eq!(pool.get(c), Some("1️⃣"));
    }

    #[test]
    fn pool_has_fixed_text_and_entry_limits_including_arc_headers() {
        let mut pool = Graphemes::default();
        assert!(pool.intern(&"x".repeat(MAX_GRAPHEME_BYTES + 1)).is_none());
        for i in 0..MAX_ENTRIES + 1 {
            let text = alloc::format!("{i:05}{}", "x".repeat(MAX_GRAPHEME_BYTES - 5));
            if pool.intern(&text).is_none() {
                break;
            }
        }
        assert!(pool.bytes <= MAX_TEXT_BYTES);
        assert!(pool.slots.len() <= MAX_ENTRIES);
        assert!(pool.index.len() < MAX_ENTRIES);
        pool.collect(core::iter::empty());
        assert_eq!(pool.bytes, 0);
        assert!(pool.index.is_empty());
        assert!(pool.intern("👨‍👩‍👧‍👦").is_some());
    }
}
