use std::ops::Bound;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use crossbeam_skiplist::map::Entry;
use crossbeam_skiplist::SkipMap;
use ouroboros::self_referencing;

use crate::iterators::impls::StorageIterator;
use crate::table::SsTableBuilder;

/// A basic mem-table based on crossbeam-skiplist
pub struct MemTable {
    map: Arc<SkipMap<Bytes, Bytes>>,
    sealed: AtomicBool,
}

pub(crate) fn map_bound(bound: Bound<&[u8]>) -> Bound<Bytes> {
    match bound {
        Bound::Included(x) => Bound::Included(Bytes::copy_from_slice(x)),
        Bound::Excluded(x) => Bound::Excluded(Bytes::copy_from_slice(x)),
        Bound::Unbounded => Bound::Unbounded,
    }
}

impl MemTable {
    /// Create a new mem-table.
    pub fn create() -> Self {
        Self {
            map: Arc::new(SkipMap::new()),
            sealed: AtomicBool::new(false),
        }
    }

    /// Get a value by key.
    pub fn get(&self, key: &[u8]) -> Option<Bytes> {
        self.map.get(key).map(|e| e.value().clone())
    }

    /// Put a key-value pair into the mem-table. If the current mem-table is sealed, return false.
    pub fn put(&self, key: &[u8], value: &[u8]) -> bool {
        use std::sync::atomic::Ordering;
        if self.sealed.load(Ordering::Acquire) {
            return false;
        }
        self.map
            .insert(Bytes::copy_from_slice(key), Bytes::copy_from_slice(value));
        true
    }

    /// Get an iterator over a range of keys.
    pub fn scan(&self, lower: Bound<&[u8]>, upper: Bound<&[u8]>) -> Result<MemTableIterator> {
        let (lower, upper) = (map_bound(lower), map_bound(upper));
        let mut iter = MemTableIteratorBuilder {
            map: self.map.clone(),
            iter_builder: |map| map.range((lower, upper)),
            item: (Bytes::from_static(&[]), Bytes::from_static(&[])),
        }
        .build();
        let entry = iter.with_iter_mut(|iter| MemTableIterator::entry_to_item(iter.next()));
        iter.with_mut(|x| *x.item = entry);
        Ok(iter)
    }

    /// Flush the mem-table to SSTable.
    pub fn flush(&self, builder: &mut SsTableBuilder) -> Result<()> {
        for entry in self.map.iter() {
            builder.add(&entry.key()[..], &entry.value()[..]);
        }
        Ok(())
    }

    /// Disable writes to this memtable.
    pub(crate) fn seal(&self) {
        use std::sync::atomic::Ordering;
        self.sealed.store(true, Ordering::Release);
    }
}

type SkipMapRangeIter<'a> =
    crossbeam_skiplist::map::Range<'a, Bytes, (Bound<Bytes>, Bound<Bytes>), Bytes, Bytes>;

/// An iterator over a range of `SkipMap`.
#[self_referencing]
pub struct MemTableIterator {
    map: Arc<SkipMap<Bytes, Bytes>>,
    #[borrows(map)]
    #[not_covariant]
    iter: SkipMapRangeIter<'this>,
    item: (Bytes, Bytes),
}

impl MemTableIterator {
    fn entry_to_item(entry: Option<Entry<'_, Bytes, Bytes>>) -> (Bytes, Bytes) {
        entry
            .map(|x| (x.key().clone(), x.value().clone()))
            .unwrap_or_else(|| (Bytes::from_static(&[]), Bytes::from_static(&[])))
    }
}

impl StorageIterator for MemTableIterator {
    fn value(&self) -> &[u8] {
        &self.borrow_item().1[..]
    }

    fn key(&self) -> &[u8] {
        &self.borrow_item().0[..]
    }

    fn is_valid(&self) -> bool {
        !self.borrow_item().0.is_empty()
    }

    fn next(&mut self) -> Result<()> {
        let entry = self.with_iter_mut(|iter| MemTableIterator::entry_to_item(iter.next()));
        self.with_mut(|x| *x.item = entry);
        Ok(())
    }
}

#[cfg(test)]
mod tests;