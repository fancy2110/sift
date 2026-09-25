//! Treemap layout, as pure geometry.
//!
//! Ported from the Svelte front end's squarified algorithm, with one deliberate
//! simplification. The original searched for a gamma exponent that kept the
//! smallest tile above a target area; that is unnecessary. A tile's area share
//! is exactly `value / total * area`, so the minimum value that still renders at
//! `min_tile_area` is
//!
//! ```text
//! min_value = min_tile_area * total / area
//! ```
//!
//! which is O(1) to compute and gives a predictable, explainable rule: entries
//! at or above that size get their own tile, everything smaller is folded into
//! one "other" tile whose label carries the count.
//!
//! Nothing here knows about GPUI, so the layout is unit-testable: the tests
//! assert that tiles stay inside the area, do not overlap, and preserve the
//! size ordering the user reads.

use serde::{Deserialize, Serialize};

use sift_core::NodeKey;

/// A rectangle in logical pixels, top-left origin.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn area(&self) -> f32 {
        (self.w.max(0.0)) * (self.h.max(0.0))
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// Whether two rectangles share any interior area.
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// The rectangle inset by `amount` on every side, never inverted.
    pub fn inset(&self, amount: f32) -> Rect {
        let inset = amount.min(self.w / 2.0).min(self.h / 2.0).max(0.0);
        Rect::new(
            self.x + inset,
            self.y + inset,
            (self.w - inset * 2.0).max(0.0),
            (self.h - inset * 2.0).max(0.0),
        )
    }
}

/// One entry to lay out.
#[derive(Debug, Clone, PartialEq)]
pub struct TreemapEntry {
    pub key: NodeKey,
    pub label: String,
    /// Bytes, used as the tile's weight.
    pub size: u64,
    pub is_dir: bool,
}

/// A laid-out tile.
#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    /// `None` for the aggregated "other" tile.
    pub key: Option<NodeKey>,
    pub label: String,
    pub rect: Rect,
    pub size: u64,
    pub is_dir: bool,
    /// How many entries the "other" tile stands for.
    pub folded_count: Option<usize>,
}

impl Tile {
    /// Whether this tile stands for several entries.
    pub fn is_aggregate(&self) -> bool {
        self.folded_count.is_some()
    }
}

/// The result of laying out one directory.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TreemapLayout {
    pub tiles: Vec<Tile>,
    /// Keys folded into the aggregate tile.
    pub folded: Vec<NodeKey>,
    /// Bytes the aggregate tile stands for.
    pub folded_bytes: u64,
}

impl TreemapLayout {
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// The largest tile's label, for a "biggest item" line.
    pub fn largest(&self) -> Option<&Tile> {
        self.tiles
            .iter()
            .filter(|tile| !tile.is_aggregate())
            .max_by_key(|tile| tile.size)
    }
}

/// Squarify `weights` into `area`, returning one rectangle per weight in the
/// same order.
///
/// This is the classic Bruls–Huizing–van Wijk algorithm: greedily grow a row
/// along the shorter side while doing so improves the worst aspect ratio, then
/// lay the row out and shrink the remaining area. Running time is O(n) for
/// sorted input.
pub fn squarify(weights: &[f64], area: Rect) -> Vec<Rect> {
    let mut out = vec![Rect::new(area.x, area.y, 0.0, 0.0); weights.len()];
    if weights.is_empty() || area.area() <= 0.0 {
        return out;
    }
    let total: f64 = weights.iter().map(|weight| weight.max(0.0)).sum();
    if total <= 0.0 {
        return out;
    }
    // Scale weights so their areas sum to the rectangle's area.
    let scale = area.area() as f64 / total;

    let mut rect = area;
    let mut index = 0usize;
    while index < weights.len() {
        // A row is laid along the rectangle's *longer* side; its thickness
        // becomes the extent along the shorter one. Using the wrong side here is
        // the classic way to produce tiles that spill outside the container.
        let longer = rect.w.max(rect.h) as f64;
        let landscape = rect.w >= rect.h;
        if longer <= 0.0 {
            // Degenerate remainder: everything left gets zero area.
            break;
        }

        let mut row_area = 0.0f64;
        let mut row_len = 0usize;
        let mut best_ratio = f64::INFINITY;

        while index + row_len < weights.len() {
            let candidate_area = row_area + weights[index + row_len].max(0.0) * scale;
            let thickness = candidate_area / longer;
            let mut worst = 1.0f64;
            for offset in 0..=row_len {
                let weight = weights[index + offset].max(0.0) * scale;
                let side = if thickness > 0.0 {
                    weight / thickness
                } else {
                    0.0
                };
                let ratio = if side <= 0.0 {
                    f64::INFINITY
                } else {
                    (thickness / side).max(side / thickness)
                };
                worst = worst.max(ratio);
            }
            // Keep growing while the worst aspect ratio does not get worse.
            if worst <= best_ratio {
                best_ratio = worst;
                row_area = candidate_area;
                row_len += 1;
            } else {
                break;
            }
        }
        if row_len == 0 {
            row_len = 1;
            row_area = weights[index].max(0.0) * scale;
        }

        let thickness = (row_area / longer) as f32;
        let mut cursor = if landscape { rect.x } else { rect.y };
        for offset in 0..row_len {
            let weight = weights[index + offset].max(0.0) * scale;
            let side = if thickness > 0.0 {
                (weight / thickness as f64) as f32
            } else {
                0.0
            };
            out[index + offset] = if landscape {
                Rect::new(cursor, rect.y, side, thickness)
            } else {
                Rect::new(rect.x, cursor, thickness, side)
            };
            cursor += side;
        }

        rect = if landscape {
            Rect::new(
                rect.x,
                rect.y + thickness,
                rect.w,
                (rect.h - thickness).max(0.0),
            )
        } else {
            Rect::new(
                rect.x + thickness,
                rect.y,
                (rect.w - thickness).max(0.0),
                rect.h,
            )
        };
        index += row_len;
    }
    out
}

/// Lay out a directory's entries.
///
/// `min_tile_area` is the smallest tile worth drawing; entries whose area share
/// would fall below it are folded into one aggregate tile. Passing `0.0` shows
/// every entry.
pub fn layout(entries: &[TreemapEntry], area: Rect, min_tile_area: f32) -> TreemapLayout {
    if entries.is_empty() || area.area() <= 0.0 {
        return TreemapLayout::default();
    }

    // Biggest first: the reading order of a treemap is by importance.
    let mut sorted: Vec<&TreemapEntry> = entries.iter().collect();
    sorted.sort_by(|left, right| {
        right
            .size
            .cmp(&left.size)
            .then_with(|| left.key.cmp(&right.key))
    });

    let total: u64 = sorted.iter().map(|entry| entry.size).sum();
    if total == 0 {
        return TreemapLayout::default();
    }

    // The direct rule: a value of `min_tile_area * total / area` occupies
    // exactly `min_tile_area`.
    let min_value = if min_tile_area > 0.0 {
        ((min_tile_area as f64) * (total as f64) / (area.area() as f64)).ceil() as u64
    } else {
        0
    };

    let (shown, folded): (Vec<&TreemapEntry>, Vec<&TreemapEntry>) = sorted
        .into_iter()
        .partition(|entry| entry.size >= min_value);

    // A single entry always renders, however small the area: an empty treemap
    // for a directory with content reads as a bug.
    let (shown, folded) = if shown.is_empty() {
        (vec![folded[0]], folded[1..].to_vec())
    } else {
        (shown, folded)
    };

    let mut weights: Vec<f64> = shown.iter().map(|entry| entry.size as f64).collect();
    let folded_bytes: u64 = folded.iter().map(|entry| entry.size).sum();
    if !folded.is_empty() {
        // The aggregate competes for area on the same scale as real entries, so
        // a long tail of small things still occupies its honest share.
        weights.push(folded_bytes as f64);
    }

    let rects = squarify(&weights, area);
    let mut tiles: Vec<Tile> = Vec::with_capacity(weights.len());
    for (index, entry) in shown.iter().enumerate() {
        tiles.push(Tile {
            key: Some(entry.key),
            label: entry.label.clone(),
            rect: rects[index],
            size: entry.size,
            is_dir: entry.is_dir,
            folded_count: None,
        });
    }
    if !folded.is_empty() {
        if let Some(rect) = rects.get(shown.len()) {
            tiles.push(Tile {
                key: None,
                label: String::new(),
                rect: *rect,
                size: folded_bytes,
                is_dir: false,
                folded_count: Some(folded.len()),
            });
        }
    }

    TreemapLayout {
        tiles,
        folded: folded.iter().map(|entry| entry.key).collect(),
        folded_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, size: u64) -> TreemapEntry {
        TreemapEntry {
            key: NodeKey::from_bytes(name.as_bytes()),
            label: name.to_string(),
            size,
            is_dir: true,
        }
    }

    fn full() -> Rect {
        Rect::new(0.0, 0.0, 800.0, 600.0)
    }

    #[test]
    fn rect_helpers_behave() {
        let rect = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert_eq!(rect.area(), 5000.0);
        assert_eq!(rect.right(), 110.0);
        assert_eq!(rect.bottom(), 70.0);
        let inner = rect.inset(5.0);
        assert_eq!(inner, Rect::new(15.0, 25.0, 90.0, 40.0));
        // An inset larger than half the rectangle must not invert it.
        let squashed = rect.inset(1000.0);
        assert!(squashed.w >= 0.0 && squashed.h >= 0.0);
        assert!(rect.overlaps(&Rect::new(50.0, 30.0, 100.0, 100.0)));
        assert!(!rect.overlaps(&Rect::new(200.0, 200.0, 10.0, 10.0)));
    }

    #[test]
    fn squarify_fills_the_area_without_overlap() {
        let weights = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let rects = squarify(&weights, full());
        assert_eq!(rects.len(), weights.len());

        // Every tile is inside the area.
        for rect in &rects {
            assert!(rect.x >= -0.01 && rect.y >= -0.01, "{rect:?}");
            assert!(rect.right() <= full().right() + 0.01, "{rect:?}");
            assert!(rect.bottom() <= full().bottom() + 0.01, "{rect:?}");
            assert!(rect.w > 0.0 && rect.h > 0.0, "{rect:?}");
        }
        // No two tiles overlap.
        for (index, left) in rects.iter().enumerate() {
            for right in rects.iter().skip(index + 1) {
                assert!(!left.overlaps(right), "{left:?} overlaps {right:?}");
            }
        }
        // The covered area matches the container's, within rounding.
        let covered: f32 = rects.iter().map(Rect::area).sum();
        let expected = full().area();
        assert!(
            (covered - expected).abs() / expected < 0.05,
            "covered {covered} vs {expected}"
        );
    }

    #[test]
    fn squarify_keeps_values_monotonic_in_area() {
        let weights = [10.0, 5.0, 1.0];
        let rects = squarify(&weights, full());
        assert!(
            rects[0].area() > rects[1].area(),
            "a bigger value must get a bigger tile"
        );
        assert!(rects[1].area() > rects[2].area());
    }

    #[test]
    fn squarify_handles_degenerate_input() {
        assert!(squarify(&[], full()).is_empty());
        assert!(squarify(&[1.0], Rect::new(0.0, 0.0, 0.0, 0.0)).len() == 1);
        // Zero weights must not produce NaN geometry.
        let rects = squarify(&[0.0, 0.0], full());
        assert_eq!(rects.len(), 2);
        for rect in &rects {
            assert!(rect.x.is_finite() && rect.w.is_finite());
        }
    }

    #[test]
    fn layout_orders_by_size_and_keeps_everything_visible() {
        let entries = vec![
            entry("small", 1),
            entry("big", 100),
            entry("medium", 50),
        ];
        // min_tile_area 0 shows everything.
        let laid = layout(&entries, full(), 0.0);
        assert_eq!(laid.tiles.len(), 3);
        assert!(laid.folded.is_empty());
        assert_eq!(laid.tiles[0].label, "big");
        assert_eq!(laid.tiles[1].label, "medium");
        assert_eq!(laid.tiles[2].label, "small");
        assert_eq!(laid.largest().unwrap().label, "big");
    }

    #[test]
    fn a_long_tail_is_folded_into_one_aggregate_tile() {
        // 400 bytes of content plus 100 one-byte entries.
        let mut entries = vec![entry("big", 400)];
        for index in 0..100u64 {
            entries.push(entry(&format!("tiny-{index}"), 1));
        }
        let laid = layout(&entries, full(), 4000.0);
        // The big entry survives; the tail becomes one tile.
        assert_eq!(
            laid.tiles.iter().filter(|tile| !tile.is_aggregate()).count(),
            1
        );
        let aggregate = laid
            .tiles
            .iter()
            .find(|tile| tile.is_aggregate())
            .expect("an aggregate tile");
        assert_eq!(aggregate.folded_count, Some(100));
        assert_eq!(aggregate.size, 100);
        assert_eq!(laid.folded.len(), 100);
        assert_eq!(laid.folded_bytes, 100);
    }

    #[test]
    fn a_single_entry_always_renders_even_below_the_threshold() {
        let entries = vec![entry("only", 1)];
        let laid = layout(&entries, full(), 1_000_000.0);
        assert_eq!(laid.tiles.len(), 1);
        assert_eq!(laid.tiles[0].label, "only");
        assert!(laid.tiles[0].rect.area() > 0.0);
    }

    #[test]
    fn empty_input_produces_an_empty_layout() {
        assert!(layout(&[], full(), 0.0).is_empty());
        assert!(layout(&[entry("x", 10)], Rect::new(0.0, 0.0, 0.0, 0.0), 0.0).is_empty());
        // Zero total bytes must not divide by zero.
        assert!(layout(&[entry("x", 0)], full(), 0.0).is_empty());
    }

    #[test]
    fn tiles_do_not_overlap_for_a_realistic_directory() {
        let mut entries: Vec<TreemapEntry> = (0..24)
            .map(|index| entry(&format!("dir-{index}"), (index as u64 + 1) * 1024 * 1024))
            .collect();
        entries.push(entry("dominant", 900 * 1024 * 1024));
        let laid = layout(&entries, Rect::new(0.0, 0.0, 1180.0, 700.0), 2000.0);
        assert!(laid.tiles.len() > 3);
        for (index, left) in laid.tiles.iter().enumerate() {
            for right in laid.tiles.iter().skip(index + 1) {
                assert!(
                    !left.rect.overlaps(&right.rect),
                    "{} overlaps {}",
                    left.label,
                    right.label
                );
            }
        }
    }

    #[test]
    fn a_tile_records_where_it_came_from() {
        let entries = vec![entry("a", 100), entry("b", 50)];
        let laid = layout(&entries, full(), 0.0);
        assert_eq!(laid.tiles[0].key, Some(NodeKey::from_bytes(b"a")));
        assert!(laid.tiles[0].is_dir);
        assert!(!laid.tiles[0].is_aggregate());
    }
}
