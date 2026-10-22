//! The offset table: for each block, the live granules in all earlier blocks.
//! That count is also where the block's first live granule lands once the heap
//! is compacted. Built in one pass over the mark bitmap; an object spanning a
//! block boundary needs no special case, since its tail is just set bits in the
//! next block.

use super::MarkBitmap;
use crate::layout::Granule;

pub(crate) struct OffsetTable(Box<[u32]>);

impl OffsetTable {
    pub(crate) fn new(blocks: usize) -> OffsetTable {
        OffsetTable(vec![0; blocks].into_boxed_slice())
    }

    /// Fill the entries for every block below `top` from `bitmap`, returning the
    /// total live granules: the top after compaction.
    pub(crate) fn build(&mut self, bitmap: &MarkBitmap, top: Granule) -> Granule {
        let mut live = 0;
        for (block, entry) in self.0[..top.blocks_below()].iter_mut().enumerate() {
            *entry = live;
            live += bitmap.live_in(block);
        }
        Granule::new(live)
    }

    pub(crate) fn get(&self, block: usize) -> u32 {
        self.0[block]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Granules;

    #[test]
    fn entries_are_live_granules_in_earlier_blocks() {
        let mut bitmap = MarkBitmap::new(4);
        bitmap.mark(Granule::new(10), Granules::new(5));
        bitmap.mark(Granule::new(62), Granules::new(70));
        bitmap.mark(Granule::new(200), Granules::new(3));
        let mut offsets = OffsetTable::new(4);
        let new_top = offsets.build(&bitmap, Granule::new(203));
        assert_eq!(new_top, Granule::new(5 + 70 + 3));
        assert_eq!(
            (0..4).map(|b| offsets.get(b)).collect::<Vec<_>>(),
            vec![0, 7, 71, 75]
        );
    }

    #[test]
    fn empty_heap_has_no_live_granules() {
        let bitmap = MarkBitmap::new(1);
        let mut offsets = OffsetTable::new(1);
        assert_eq!(offsets.build(&bitmap, Granule::ZERO), Granule::ZERO);
    }
}
