//! Safe model of deterministic bump regions.
//!
//! This module is a compiler/runtime design prototype. It deliberately returns
//! offsets rather than raw pointers, so the simulator cannot create dangling
//! references or expose uninitialized memory. It is not wired into the C ABI.

use std::any::Any;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionMark {
    cursor: usize,
    finalizers: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegionError {
    InvalidAlignment,
    CapacityOverflow,
    AllocationFailed,
    InvalidMark,
}

#[derive(Default)]
pub struct Region {
    bytes: Vec<u8>,
    cursor: usize,
    tracked: Vec<Box<dyn Any>>,
    epoch: u64,
}

impl Region {
    pub fn new() -> Self {
        Self::default()
    }

    /// Current logical allocation size; capacity may be larger and is retained
    /// after restore to make subsequent bump allocations cheap.
    pub fn used(&self) -> usize {
        self.cursor
    }

    pub fn capacity(&self) -> usize {
        self.bytes.len()
    }

    pub fn mark(&self) -> RegionMark {
        RegionMark {
            cursor: self.cursor,
            finalizers: self.tracked.len(),
        }
    }

    /// Reserve an aligned byte range. The returned range is an offset into the
    /// region's backing buffer, not a pointer.
    pub fn alloc_bytes(&mut self, size: usize, align: usize) -> Result<Range<usize>, RegionError> {
        if align == 0 || !align.is_power_of_two() {
            return Err(RegionError::InvalidAlignment);
        }
        let mask = align - 1;
        let start = self.cursor.checked_add(mask)
            .ok_or(RegionError::CapacityOverflow)? & !mask;
        let end = start.checked_add(size).ok_or(RegionError::CapacityOverflow)?;
        if end > self.bytes.len() {
            self.bytes.try_reserve(end - self.bytes.len())
                .map_err(|_| RegionError::AllocationFailed)?;
            self.bytes.resize(end, 0);
        }
        self.cursor = end;
        Ok(start..end)
    }

    /// Store an owned value whose destructor must run when this region is
    /// restored or dropped. This models deterministic finalizer ordering.
    pub fn track<T: Any>(&mut self, value: T) {
        self.tracked.push(Box::new(value));
    }

    /// Rewind to a previously created mark. Values tracked after the mark are
    /// dropped in reverse insertion order by Vec::truncate.
    pub fn restore(&mut self, mark: RegionMark) -> Result<(), RegionError> {
        if mark.cursor > self.cursor || mark.finalizers > self.tracked.len() {
            return Err(RegionError::InvalidMark);
        }
        while self.tracked.len() > mark.finalizers {
            drop(self.tracked.pop());
        }
        self.cursor = mark.cursor;
        self.epoch = self.epoch.checked_add(1).ok_or(RegionError::CapacityOverflow)?;
        Ok(())
    }

    /// Read a previously reserved range. This returns only initialized bytes.
    pub fn read(&self, range: Range<usize>) -> Option<&[u8]> {
        if range.start > range.end || range.end > self.cursor {
            return None;
        }
        self.bytes.get(range)
    }

    /// Write to a previously reserved range.
    pub fn write(&mut self, range: Range<usize>, value: &[u8]) -> bool {
        if range.start > range.end || range.end > self.cursor
            || range.len() != value.len()
        {
            return false;
        }
        self.bytes[range].copy_from_slice(value);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn bump_allocations_are_aligned_and_zero_initialized() {
        let mut region = Region::new();
        let first = region.alloc_bytes(3, 1).unwrap();
        let second = region.alloc_bytes(8, 8).unwrap();
        assert_eq!(first, 0..3);
        assert_eq!(second.start % 8, 0);
        assert_eq!(region.read(second.clone()), Some(&[0; 8][..]));
        assert_eq!(region.used(), second.end);
    }

    #[test]
    fn restore_reuses_space_and_invalidates_old_marks() {
        let mut region = Region::new();
        let mark = region.mark();
        let first = region.alloc_bytes(32, 8).unwrap();
        assert_eq!(first, 0..32);
        region.restore(mark).unwrap();
        assert_eq!(region.used(), 0);
        assert!(region.alloc_bytes(16, 8).is_ok());
        assert!(region.restore(mark).is_ok());
    }

    #[test]
    fn restore_runs_tracked_destructors_in_reverse_order() {
        struct DropRecord(usize, Arc<Mutex<Vec<usize>>>);
        impl Drop for DropRecord {
            fn drop(&mut self) {
                self.1.lock().unwrap().push(self.0);
            }
        }

        let drops = Arc::new(Mutex::new(Vec::new()));
        let mut region = Region::new();
        let mark = region.mark();
        region.track(DropRecord(1, drops.clone()));
        region.track(DropRecord(2, drops.clone()));
        region.restore(mark).unwrap();
        assert_eq!(*drops.lock().unwrap(), vec![2, 1]);
    }

    #[test]
    fn rejects_invalid_alignment_and_out_of_range_access() {
        let mut region = Region::new();
        assert_eq!(region.alloc_bytes(4, 3), Err(RegionError::InvalidAlignment));
        let range = region.alloc_bytes(4, 4).unwrap();
        assert!(!region.write(range.start..range.end + 1, &[1, 2, 3, 4, 5]));
        assert_eq!(region.read(10..11), None);
    }

    #[test]
    fn nested_marks_restore_only_inner_allocations() {
        let mut region = Region::new();
        let outer = region.alloc_bytes(8, 8).unwrap();
        let inner_mark = region.mark();
        let inner = region.alloc_bytes(16, 8).unwrap();
        region.restore(inner_mark).unwrap();
        assert_eq!(region.used(), outer.end);
        assert_eq!(region.read(outer), Some(&[0; 8][..]));
        assert_eq!(region.read(inner), None);
    }
}
