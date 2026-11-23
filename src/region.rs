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
        // Reused ranges may contain bytes from a previous allocation after restore.
        // Clear every logical allocation, not only newly grown backing storage.
        self.bytes[start..end].fill(0);
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


/// Stable identifier for a region descriptor in a RegionTable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RegionId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionState {
    Active,
    Closed,
}

/// A descriptor owns one region and records its lexical parent. Descriptors
/// contain no raw pointers and are invalidated explicitly by close().
pub struct RegionDescriptor {
    pub id: RegionId,
    pub parent: Option<RegionId>,
    pub state: RegionState,
    region: Region,
    initial_mark: RegionMark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionCheckpoint {
    pub region: RegionId,
    mark: RegionMark,
}

/// Per-activation descriptor table. The table owns all regions and enforces
/// parent/child lifetime ordering.
/// Stable handle for an object owned by a region. Handles are table indices,
/// never raw pointers into a region's backing storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RegionObjectId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionObject {
    pub owner: RegionId,
    pub payload: Vec<u8>,
    pub references: Vec<RegionObjectId>,
}

#[derive(Default)]
pub struct RegionTable {
    descriptors: Vec<RegionDescriptor>,
    objects: Vec<Option<RegionObject>>,
}

impl RegionTable {
    pub fn new() -> Self { Self::default() }

    pub fn create(&mut self, parent: Option<RegionId>) -> Result<RegionId, RegionError> {
        if let Some(parent_id) = parent {
            let parent_desc = self.descriptors.get(parent_id.0 as usize)
                .ok_or(RegionError::InvalidMark)?;
            if parent_desc.id != parent_id || parent_desc.state != RegionState::Active {
                return Err(RegionError::InvalidMark);
            }
        }
        let id = RegionId(u32::try_from(self.descriptors.len())
            .map_err(|_| RegionError::CapacityOverflow)?);
        let region = Region::new();
        let initial_mark = region.mark();
        self.descriptors.push(RegionDescriptor {
            id, parent, state: RegionState::Active, region, initial_mark,
        });
        Ok(id)
    }

    fn active_mut(&mut self, id: RegionId) -> Result<&mut RegionDescriptor, RegionError> {
        let descriptor = self.descriptors.get_mut(id.0 as usize)
            .ok_or(RegionError::InvalidMark)?;
        if descriptor.id != id || descriptor.state != RegionState::Active {
            return Err(RegionError::InvalidMark);
        }
        Ok(descriptor)
    }

    pub fn state(&self, id: RegionId) -> Option<RegionState> {
        self.descriptors.get(id.0 as usize)
            .filter(|descriptor| descriptor.id == id)
            .map(|descriptor| descriptor.state)
    }

    pub fn alloc_bytes(
        &mut self, id: RegionId, size: usize, align: usize,
    ) -> Result<Range<usize>, RegionError> {
        self.active_mut(id)?.region.alloc_bytes(size, align)
    }

    pub fn checkpoint(&mut self, id: RegionId) -> Result<RegionCheckpoint, RegionError> {
        let descriptor = self.active_mut(id)?;
        Ok(RegionCheckpoint { region: id, mark: descriptor.region.mark() })
    }

    pub fn restore(&mut self, checkpoint: RegionCheckpoint) -> Result<(), RegionError> {
        self.active_mut(checkpoint.region)?.region.restore(checkpoint.mark)
    }

    /// Close a region only after all its direct children have been closed.
    /// Descendants are checked transitively by requiring each direct child closed.
    pub fn close(&mut self, id: RegionId) -> Result<(), RegionError> {
        if self.descriptors.iter().any(|child|
            child.parent == Some(id) && child.state == RegionState::Active
        ) {
            return Err(RegionError::InvalidMark);
        }
        let descriptor = self.active_mut(id)?;
        descriptor.region.restore(descriptor.initial_mark)?;
        descriptor.state = RegionState::Closed;
        self.remove_owned_objects(id);
        Ok(())
    }

    pub fn used(&self, id: RegionId) -> Option<usize> {
        self.descriptors.get(id.0 as usize)
            .filter(|descriptor| descriptor.id == id && descriptor.state == RegionState::Active)
            .map(|descriptor| descriptor.region.used())
    }

    fn is_ancestor(&self, ancestor: RegionId, mut child: RegionId) -> bool {
        loop {
            if ancestor == child { return true; }
            let Some(descriptor) = self.descriptors.get(child.0 as usize) else { return false; };
            let Some(parent) = descriptor.parent else { return false; };
            child = parent;
        }
    }

    /// Create a logical object owned by an active region. References are handles
    /// and are validated before insertion, so no dangling edge can be created.
    pub fn create_object(
        &mut self, owner: RegionId, payload: Vec<u8>, references: Vec<RegionObjectId>,
    ) -> Result<RegionObjectId, RegionError> {
        self.active_mut(owner)?;
        for reference in &references {
            if self.objects.get(reference.0 as usize).and_then(Option::as_ref).is_none() {
                return Err(RegionError::InvalidMark);
            }
        }
        let id = RegionObjectId(u32::try_from(self.objects.len())
            .map_err(|_| RegionError::CapacityOverflow)?);
        self.objects.push(Some(RegionObject { owner, payload, references }));
        Ok(id)
    }

    pub fn object(&self, id: RegionObjectId) -> Option<&RegionObject> {
        self.objects.get(id.0 as usize).and_then(Option::as_ref)
    }

    /// Promote an object graph into an ancestor region. The graph is deep-copied
    /// transitively, cycles and shared subgraphs are preserved through a memo
    /// table, and the original graph remains owned by its original regions.
    pub fn promote(
        &mut self, root: RegionObjectId, destination: RegionId,
    ) -> Result<RegionObjectId, RegionError> {
        self.active_mut(destination)?;
        let root_object = self.object(root).ok_or(RegionError::InvalidMark)?;
        if !self.is_ancestor(destination, root_object.owner) {
            return Err(RegionError::InvalidMark);
        }

        let mut pending = vec![root];
        let mut copies: std::collections::HashMap<RegionObjectId, RegionObjectId> =
            std::collections::HashMap::new();
        while let Some(old_id) = pending.pop() {
            if copies.contains_key(&old_id) { continue; }
            let old = self.object(old_id).cloned().ok_or(RegionError::InvalidMark)?;
            let new_id = RegionObjectId(u32::try_from(self.objects.len())
                .map_err(|_| RegionError::CapacityOverflow)?);
            copies.insert(old_id, new_id);
            self.objects.push(Some(RegionObject {
                owner: destination,
                payload: old.payload,
                references: Vec::new(),
            }));
            pending.extend(old.references);
        }

        for (old_id, new_id) in &copies {
            let old = self.object(*old_id).cloned().ok_or(RegionError::InvalidMark)?;
            let new_refs = old.references.iter()
                .map(|id| copies.get(id).copied().ok_or(RegionError::InvalidMark))
                .collect::<Result<Vec<_>, _>>()?;
            let target = self.objects.get_mut(new_id.0 as usize)
                .and_then(Option::as_mut).ok_or(RegionError::InvalidMark)?;
            target.references = new_refs;
        }
        copies.get(&root).copied().ok_or(RegionError::InvalidMark)
    }

    /// Remove objects owned by a region when it closes. Object IDs are never
    /// recycled, so stale handles cannot accidentally refer to a new object.
    fn remove_owned_objects(&mut self, owner: RegionId) {
        for object in &mut self.objects {
            if object.as_ref().is_some_and(|value| value.owner == owner) {
                *object = None;
            }
        }
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
    #[test]
    fn reused_allocation_is_zero_initialized_after_restore() {
        let mut region = Region::new();
        let mark = region.mark();
        let first = region.alloc_bytes(4, 1).unwrap();
        assert!(region.write(first.clone(), &[9, 8, 7, 6]));
        region.restore(mark).unwrap();
        let second = region.alloc_bytes(4, 1).unwrap();
        assert_eq!(first, second);
        assert_eq!(region.read(second), Some(&[0, 0, 0, 0][..]));
    }


    #[test]
    fn descriptors_enforce_parent_child_close_order() {
        let mut table = RegionTable::new();
        let parent = table.create(None).unwrap();
        let child = table.create(Some(parent)).unwrap();
        assert_eq!(table.close(parent), Err(RegionError::InvalidMark));
        table.close(child).unwrap();
        table.close(parent).unwrap();
        assert_eq!(table.state(parent), Some(RegionState::Closed));
        assert_eq!(table.alloc_bytes(parent, 1, 1), Err(RegionError::InvalidMark));
    }

    #[test]
    fn descriptor_checkpoint_restores_only_its_region() {
        let mut table = RegionTable::new();
        let region = table.create(None).unwrap();
        table.alloc_bytes(region, 8, 8).unwrap();
        let checkpoint = table.checkpoint(region).unwrap();
        table.alloc_bytes(region, 16, 8).unwrap();
        assert_eq!(table.used(region), Some(24));
        table.restore(checkpoint).unwrap();
        assert_eq!(table.used(region), Some(8));
    }

    #[test]
    fn descriptor_rejects_unknown_parent_and_checkpoint_after_close() {
        let mut table = RegionTable::new();
        assert_eq!(table.create(Some(RegionId(99))), Err(RegionError::InvalidMark));
        let region = table.create(None).unwrap();
        let checkpoint = table.checkpoint(region).unwrap();
        table.close(region).unwrap();
        assert_eq!(table.restore(checkpoint), Err(RegionError::InvalidMark));
    }


    #[test]
    fn promotion_copies_transitive_graph_to_ancestor_region() {
        let mut table = RegionTable::new();
        let parent = table.create(None).unwrap();
        let child = table.create(Some(parent)).unwrap();
        let leaf = table.create_object(child, vec![7, 8], vec![]).unwrap();
        let root = table.create_object(child, vec![1, 2], vec![leaf]).unwrap();

        let promoted = table.promote(root, parent).unwrap();
        assert_ne!(promoted, root);
        let promoted_root = table.object(promoted).unwrap();
        assert_eq!(promoted_root.owner, parent);
        assert_eq!(promoted_root.payload, vec![1, 2]);
        let promoted_leaf = table.object(promoted_root.references[0]).unwrap();
        assert_eq!(promoted_leaf.owner, parent);
        assert_eq!(promoted_leaf.payload, vec![7, 8]);

        table.close(child).unwrap();
        assert_eq!(table.object(root), None);
        assert_eq!(table.object(promoted).unwrap().payload, vec![1, 2]);
    }

    #[test]
    fn promotion_preserves_shared_references_and_cycles() {
        let mut table = RegionTable::new();
        let parent = table.create(None).unwrap();
        let child = table.create(Some(parent)).unwrap();
        let a = table.create_object(child, vec![1], vec![]).unwrap();
        let b = table.create_object(child, vec![2], vec![a]).unwrap();
        table.objects[a.0 as usize].as_mut().unwrap().references.push(b);
        let promoted_a = table.promote(a, parent).unwrap();
        let promoted_b = table.object(promoted_a).unwrap().references[0];
        assert_eq!(table.object(promoted_b).unwrap().references, vec![promoted_a]);
    }

    #[test]
    fn promotion_rejects_destination_that_does_not_outlive_source() {
        let mut table = RegionTable::new();
        let parent = table.create(None).unwrap();
        let child = table.create(Some(parent)).unwrap();
        let sibling = table.create(Some(parent)).unwrap();
        let object = table.create_object(parent, vec![3], vec![]).unwrap();
        assert_eq!(table.promote(object, child), Err(RegionError::InvalidMark));
        assert_eq!(table.promote(object, sibling), Err(RegionError::InvalidMark));
    }

    #[test]
    fn closing_region_invalidates_its_object_handles() {
        let mut table = RegionTable::new();
        let region = table.create(None).unwrap();
        let object = table.create_object(region, vec![42], vec![]).unwrap();
        table.close(region).unwrap();
        assert_eq!(table.object(object), None);
    }


}
