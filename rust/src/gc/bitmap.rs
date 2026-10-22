//! The mark bitmap: one bit per heap granule, one `u64` word per block. Bit
//! `g.bit()` of word `g.block()` is set iff granule `g` belongs to a live object.
//! Marking sets every granule of an object, so live objects are runs of set bits
//! and the bits below a granule count the live granules before it.

use crate::layout::{Granule, Granules};

pub(crate) struct MarkBitmap(Box<[u64]>);

impl MarkBitmap {
    pub(crate) fn new(blocks: usize) -> MarkBitmap {
        MarkBitmap(vec![0; blocks].into_boxed_slice())
    }

    /// Clear every word holding a granule below `top`. Bits at and above `top`
    /// in the last word are cleared with it, so they never read as live.
    pub(crate) fn clear_below(&mut self, top: Granule) {
        self.0[..top.blocks_below()].fill(0);
    }

    pub(crate) fn is_marked(&self, g: Granule) -> bool {
        (self.0[g.block()] >> g.bit()) & 1 != 0
    }

    /// Mark the `n` granules starting at `g`: masked first and last words, whole
    /// words between.
    pub(crate) fn mark(&mut self, g: Granule, n: Granules) {
        debug_assert!(n != Granules::new(0), "every object spans a granule");
        let last = Granule::new((g + n).index() - 1);
        let head = !0u64 << g.bit();
        let tail = !0u64 >> (63 - last.bit());
        if g.block() == last.block() {
            self.0[g.block()] |= head & tail;
        } else {
            self.0[g.block()] |= head;
            self.0[g.block() + 1..last.block()].fill(!0);
            self.0[last.block()] |= tail;
        }
    }

    /// The first marked granule in `[from, limit)`.
    pub(crate) fn next_set(&self, from: Granule, limit: Granule) -> Option<Granule> {
        if from >= limit {
            return None;
        }
        let mut block = from.block();
        let mut word = self.0[block] & (!0u64 << from.bit());
        while word == 0 {
            block += 1;
            if block >= limit.blocks_below() {
                return None;
            }
            word = self.0[block];
        }
        let g = Granule::new(Granule::block_start(block).index() + word.trailing_zeros());
        (g < limit).then_some(g)
    }

    /// The marked granules before `g` in its own block.
    pub(crate) fn live_before(&self, g: Granule) -> u32 {
        (self.0[g.block()] & ((1u64 << g.bit()) - 1)).count_ones()
    }

    /// The marked granules in `block`.
    pub(crate) fn live_in(&self, block: usize) -> u32 {
        self.0[block].count_ones()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(i: u32) -> Granule {
        Granule::new(i)
    }

    fn marked(bitmap: &MarkBitmap, limit: u32) -> Vec<u32> {
        (0..limit).filter(|&i| bitmap.is_marked(g(i))).collect()
    }

    #[test]
    fn mark_within_one_word() {
        let mut bitmap = MarkBitmap::new(2);
        bitmap.mark(g(3), Granules::new(4));
        assert_eq!(marked(&bitmap, 128), vec![3, 4, 5, 6]);
    }

    #[test]
    fn mark_across_words() {
        let mut bitmap = MarkBitmap::new(4);
        bitmap.mark(g(62), Granules::new(3));
        assert_eq!(marked(&bitmap, 256), vec![62, 63, 64]);

        let mut bitmap = MarkBitmap::new(4);
        bitmap.mark(g(60), Granules::new(140));
        assert_eq!(marked(&bitmap, 256), (60..200).collect::<Vec<_>>());
    }

    #[test]
    fn mark_whole_word() {
        let mut bitmap = MarkBitmap::new(2);
        bitmap.mark(g(64), Granules::new(64));
        assert_eq!(bitmap.live_in(0), 0);
        assert_eq!(bitmap.live_in(1), 64);
    }

    #[test]
    fn clear_below_clears_whole_words() {
        let mut bitmap = MarkBitmap::new(3);
        bitmap.mark(g(0), Granules::new(192));
        bitmap.clear_below(g(65));
        assert_eq!(marked(&bitmap, 192), (128..192).collect::<Vec<_>>());
    }

    #[test]
    fn next_set_scans_forward_to_limit() {
        let mut bitmap = MarkBitmap::new(4);
        bitmap.mark(g(5), Granules::new(2));
        bitmap.mark(g(130), Granules::new(1));
        assert_eq!(bitmap.next_set(g(0), g(256)), Some(g(5)));
        assert_eq!(bitmap.next_set(g(6), g(256)), Some(g(6)));
        assert_eq!(bitmap.next_set(g(7), g(256)), Some(g(130)));
        assert_eq!(bitmap.next_set(g(7), g(130)), None);
        assert_eq!(bitmap.next_set(g(131), g(256)), None);
        assert_eq!(bitmap.next_set(g(5), g(5)), None);
    }

    #[test]
    fn live_before_counts_within_the_block() {
        let mut bitmap = MarkBitmap::new(2);
        bitmap.mark(g(0), Granules::new(3));
        bitmap.mark(g(60), Granules::new(6));
        assert_eq!(bitmap.live_before(g(0)), 0);
        assert_eq!(bitmap.live_before(g(60)), 3);
        assert_eq!(bitmap.live_before(g(63)), 6);
        assert_eq!(bitmap.live_before(g(64)), 0);
        assert_eq!(bitmap.live_before(g(66)), 2);
    }
}
