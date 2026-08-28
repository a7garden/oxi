//! Pressure-driven tool row allocation ladder.
//!
//! The ladder is the *automatic* layer that the live region uses to fit
//! ever-taller blocks (tool calls with diffs, file reads with full
//! previews, command outputs) into a finite viewport. It is pure:
//! given a list of per-block natural heights and a row budget, it
//! returns the rendered row count for each block. The renderer
//! decides how to draw the quantized shapes (glyph / folded / full);
//! this module decides how many rows each block gets.
//!
//! # Allocation levels
//!
//! - `0` — block is hidden entirely (emergency truncation; the
//!   caller reserves a single banner row instead).
//! - `1` — glyph row (`▸ tool · activity`), animated wall-clock
//!   pulse on a shared period so the live region breathes.
//! - `2` — folded card (`╭─ tool · activity` / `╰─ …`), the static
//!   "something is here" affordance.
//! - `natural` — full block at its natural height.
//!
//! Allocations are quantized to those four shapes: a block never
//! receives `3..natural-1` rows, because the live-region renderer
//! maps any allocation of 3 or more rows to the FULL natural
//! render — a "3 of 5" allocation would paint 5 rows and overflow
//! the budget.
//!
//! User-set `BlockDisplayMode` overrides happen at the call site
//! (see `render_transcript`); the ladder applies only to blocks
//! without a manual override.

/// Row budget assigned to a single block by [`allocate_rows`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockAlloc {
    pub rows: usize,
}

/// Allocate a row budget across the given per-block natural heights.
/// Pure function: same `(block_heights, budget)` always yields the
/// same `Vec<BlockAlloc>`. See module docs for the algorithm.
pub fn allocate_rows(block_heights: &[usize], budget: usize) -> Vec<BlockAlloc> {
    let n = block_heights.len();
    let mut out = vec![BlockAlloc { rows: 0 }; n];

    // Empty input: nothing to allocate, nothing to overflow.
    if n == 0 {
        return out;
    }

    let total: usize = block_heights.iter().sum();

    // Roomy: every block fits in full. Cap each block at its
    // natural height so excess budget never inflates a row.
    if total <= budget {
        for (i, &h) in block_heights.iter().enumerate() {
            out[i].rows = h;
        }
        return out;
    }

    // Emergency: more blocks than rows. Hide the oldest
    // `n - budget` blocks entirely. The caller reserves one row
    // for the `… N earlier blocks hidden` banner in the budget
    // when `budget >= 1`, so we keep exactly `budget` glyph rows.
    // A zero budget collapses every block to hidden.
    if n > budget {
        let keep = budget;
        // Indices in `[n - keep, n)` get a glyph row; the rest
        // are hidden. When `keep == 0` the range is empty and
        // every slot stays at 0.
        let first_kept = n.saturating_sub(keep);
        for (i, slot) in out.iter_mut().enumerate() {
            slot.rows = if i >= first_kept { 1 } else { 0 };
        }
        return out;
    }
    // Pressure: every block gets at least 1 row. Surplus
    // (`budget - n`) is distributed newest-first, quantized to the
    // shapes the live region can actually render: the block's full
    // natural height, the 2-row folded card, or the 1-row glyph
    // floor. Mid-range allocations (`3..natural-1` rows) are never
    // emitted — the renderer maps any `alloc.rows >= 3` to the full
    // natural render, so a 3-of-5 allocation would paint 5 rows and
    // blow the budget.
    let surplus = budget - n;
    let mut remaining = surplus;
    for i in (0..n).rev() {
        let natural = block_heights[i];
        // Rows beyond the glyph floor needed to render in full.
        let full_deficit = natural.saturating_sub(1);
        if full_deficit > 0 && remaining >= full_deficit {
            // Full natural height.
            out[i].rows = natural;
            remaining -= full_deficit;
        } else if natural >= 2 && remaining >= 1 {
            // Folded card (header + ellipsis row).
            out[i].rows = 2;
            remaining -= 1;
        } else {
            // Glyph floor.
            out[i].rows = 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roomy_all_full() {
        // 3 blocks, heights 4/2/3 = 9 total; budget 12 → all full,
        // and excess budget is NOT inflated onto any block.
        let heights = [4, 2, 3];
        let alloc = allocate_rows(&heights, 12);
        assert_eq!(alloc.len(), 3);
        assert_eq!(alloc[0].rows, 4);
        assert_eq!(alloc[1].rows, 2);
        assert_eq!(alloc[2].rows, 3);
        // Exactly-fits budget: no extra rows.
        let alloc2 = allocate_rows(&[2, 3, 4], 9);
        assert_eq!(alloc2[0].rows, 2);
        assert_eq!(alloc2[1].rows, 3);
        assert_eq!(alloc2[2].rows, 4);
    }

    #[test]
    fn pressure_folds_oldest_first() {
        // Surplus is distributed newest-first. Newest gets filled
        // up first; once it caps, the next-newest absorbs the rest.
        let heights = [5, 4, 3];
        // budget 6 → surplus 3. Newest(h=3) takes 2 → rows=3.
        // remaining=1 → mid(h=4) takes 1 → rows=2 (folded).
        // Oldest stays at 1 (glyph).
        let alloc = allocate_rows(&heights, 6);
        assert_eq!(alloc[0].rows, 1, "oldest pinned to glyph");
        assert_eq!(alloc[1].rows, 2, "mid folded card (1 surplus)");
        assert_eq!(alloc[2].rows, 3, "newest full");

        // 3 blocks, heights 5/4/3 = 12 total; budget 8 → 5 surplus.
        // Newest gets min(2, 3) = 2 → rows = 3. remaining = 3.
        // Next gets min(3, 4) = 3 → rows = 4. remaining = 0.
        // Oldest stays at 1.
        let alloc2 = allocate_rows(&heights, 8);
        assert_eq!(alloc2[0].rows, 1, "oldest pinned (no surplus left)");
        assert_eq!(alloc2[1].rows, 4, "middle full");
        assert_eq!(alloc2[2].rows, 3, "newest full");

        // 3 blocks, heights 5/4/3; budget 7 → 4 surplus.
        // Newest gets deficit 2 → full 3. remaining = 2.
        // Next (h=4): deficit 3 > 2 → folded card 2. remaining = 1.
        // Oldest (h=5): deficit 4 > 1 → folded card 2. remaining = 0.
        // No block ever lands in the forbidden 3..natural-1 band.
        let alloc3 = allocate_rows(&heights, 7);
        assert_eq!(alloc3[0].rows, 2, "oldest folded card");
        assert_eq!(alloc3[1].rows, 2, "mid folded card (2-row fold)");
        assert_eq!(alloc3[2].rows, 3);
    }

    #[test]
    fn pressure_never_allocates_mid_range_rows() {
        // A block's share is always 0, 1, 2, or its natural height —
        // never 3..natural-1. The renderer treats alloc.rows >= 3 as
        // "render natural", so a mid-range share would overflow the
        // budget (final-review finding 5).

        // Single 10-row block, budget 4: the old allocator emitted 4
        // (mid-range); the renderer would have painted all 10 rows.
        let alloc = allocate_rows(&[10], 4);
        assert_eq!(alloc[0].rows, 2, "budget 4 of natural 10 folds");

        // Budget 9 is still mid-range for natural 10 → folded card,
        // even though 7 budget rows go unused.
        let alloc = allocate_rows(&[10], 9);
        assert_eq!(alloc[0].rows, 2);

        // Budget 10 = natural → roomy, full render.
        let alloc = allocate_rows(&[10], 10);
        assert_eq!(alloc[0].rows, 10);

        // Exhaustive sweep: for every (heights, budget) combination
        // every allocation is quantized and the sum stays in budget.
        for budget in 0..=30usize {
            for h in 3..=8usize {
                let heights = [h, h, h];
                let allocs = allocate_rows(&heights, budget);
                let mut sum = 0usize;
                for a in &allocs {
                    assert!(
                        a.rows <= 2 || a.rows == h,
                        "mid-range allocation rows={} for natural={h} (budget {budget})",
                        a.rows
                    );
                    sum += a.rows;
                }
                assert!(
                    sum <= budget.max(heights.iter().sum()),
                    "sum {sum} exceeds budget {budget} (heights {heights:?})"
                );
            }
        }
    }

    #[test]
    fn emergency_hides_oldest_and_banners() {
        // 5 blocks, budget 3 → emergency. Newest 3 get 1 glyph
        // row each; oldest 2 hidden. The caller reserves 1 row
        // for the banner; total painted = 3 glyphs + 1 banner.
        let heights = [2, 3, 4, 5, 6];
        let alloc = allocate_rows(&heights, 3);
        assert_eq!(alloc.len(), 5);
        assert_eq!(alloc[0].rows, 0, "oldest hidden");
        assert_eq!(alloc[4].rows, 1, "newest glyph");
        // Sum of allocated glyph rows equals budget; caller adds
        // +1 for the banner.
        assert_eq!(alloc.iter().map(|a| a.rows).sum::<usize>(), 3);

        // n > budget exactly: 4 blocks of height 2, budget 3 →
        // emergency. Newest 3 glyphs, oldest 1 hidden.
        let alloc3 = allocate_rows(&[2, 2, 2, 2], 3);
        assert_eq!(alloc3[0].rows, 0);
        assert_eq!(alloc3[1].rows, 1);
        assert_eq!(alloc3[2].rows, 1);
        assert_eq!(alloc3[3].rows, 1);
    }

    #[test]
    fn empty_inputs_no_panic() {
        // Empty heights: returns empty vec.
        let alloc = allocate_rows(&[], 10);
        assert!(alloc.is_empty());

        // Zero budget with non-empty heights: every block hidden
        // (emergency branch, keep = 0 → all slots 0).
        let alloc = allocate_rows(&[5, 4, 3], 0);
        assert_eq!(alloc.len(), 3);
        assert!(alloc.iter().all(|a| a.rows == 0));

        // Zero budget, empty heights: still empty, no panic.
        let alloc = allocate_rows(&[], 0);
        assert!(alloc.is_empty());

        // Heights of 0: roomy wins (sum 0 <= budget); every
        // block gets 0 rows (which is "nothing to render").
        let alloc = allocate_rows(&[0, 0, 0], 10);
        assert_eq!(alloc.len(), 3);
        assert!(alloc.iter().all(|a| a.rows == 0));
    }

    #[test]
    fn single_block_taller_than_budget_folds() {
        // Single block of height 10, budget 4 → pressure branch
        // (n=1, budget=4). Surplus 3 < deficit 9, so the share is
        // quantized down to the 2-row folded card (never 3..9).
        let alloc = allocate_rows(&[10], 4);
        assert_eq!(alloc[0].rows, 2);

        // Budget 2: surplus = 1. Newest gets min(9, 1) = 1 →
        // rows = 2 = folded card.
        let alloc = allocate_rows(&[10], 2);
        assert_eq!(alloc[0].rows, 2);

        // Budget 1: surplus = 0. Newest gets 0 extra → rows = 1
        // = glyph row.
        let alloc = allocate_rows(&[10], 1);
        assert_eq!(alloc[0].rows, 1);

        // Budget 0: emergency branch (n > b). All hidden.
        let alloc = allocate_rows(&[10], 0);
        assert_eq!(alloc[0].rows, 0);

        // Two blocks both taller than budget: oldest pinned at
        // glyph if no surplus reaches it.
        let alloc = allocate_rows(&[10, 10], 3);
        // n=2, budget=3 → pressure. Surplus = 1. Newest (idx 1)
        // gets min(9, 1) = 1 → rows = 2. Oldest (idx 0) stays
        // at 1.
        assert_eq!(alloc[0].rows, 1);
        assert_eq!(alloc[1].rows, 2);
    }
}
