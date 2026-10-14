//! Grid geometry derived from a system's cover ratio — a port of `browse/model/GridGeometry.kt`.
//!
//! The rule the Kotlin spec states as "derive, don't hardcode": a system's column count and tile
//! size come out of the shape of its box art, not out of a table. That is what keeps the *pitch*
//! (the vertical distance between row tops) in a narrow band across systems, so scrolling a PS2
//! grid and scrolling a SNES grid feel the same even though the tiles are different shapes.
//!
//! Values are in layout units at the 1280x800 reference panel.

/// The alphabet rail pinned to the grid's right edge.
///
/// Part of the width budget, not an overlay: a rail drawn on top of the tiles would clip the
/// focused one, and the derivation below is only meaningful if the width it divides up is the
/// width tiles actually get — which is why the rail is part of [`CHROME_W`].
pub const ALPHA_RAIL_W: f32 = 34.0;

/// The page's own left/right padding around the grid, per side.
pub const GRID_PADDING: f32 = 16.0;

/// The system list down the Browse page's left edge. Part of the width budget for the same reason
/// as the rail: tiles are sized to fill the width they are given, so whatever is drawn beside the
/// grid has to come out of that width first.
///
/// Wide enough for a name like "Game Boy Advance" beside its game count without cutting it. The
/// cost is a column on the reference panel for square and portrait systems, where landscape ones
/// are already at the three-column floor and draw their covers smaller instead.
pub const SYSTEM_LIST_W: f32 = 240.0;

/// Everything beside the tiles: the system list, the page padding on both sides, and the alphabet
/// rail.
///
/// This is measured from *this* app's layout, not inherited. The Kotlin source's equivalent
/// reserves ~264 units because its figure predates the left rail being removed — that app moved
/// navigation to a top tab bar and handed the rail's 200 units back to the covers, but the grid
/// constant still spends them. Carried over unexamined it left a rail-shaped hole down the right of
/// every grid: tiles sized to fill a width ~200 units narrower than the container they sit in.
pub const CHROME_W: f32 = SYSTEM_LIST_W + GRID_PADDING * 2.0 + ALPHA_RAIL_W;

/// Gap between tiles, horizontally and vertically.
pub const GAP: f32 = 24.0;

/// The title strip under each tile.
pub const LABEL_H: f32 = 28.0;

/// The tile height the column count aims at before the ratio is applied.
pub const TARGET_H: f32 = 235.0;

/// The content width the layout is authored against (the Deck's own panel).
pub const REFERENCE_CONTENT_W: f32 = 1280.0;

/// The matching height, used as the starting guess for the grid's viewport before the window has
/// reported its real size.
pub const REFERENCE_CONTENT_H: f32 = 800.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSpec {
    pub cols: usize,
    pub tile_w: f32,
    pub tile_h: f32,
    /// Vertical distance between row tops: tile + label strip + gap.
    pub pitch: f32,
}

/// The tile-usable width for an actual content width: everything the chrome does not take.
///
/// Floored well below the reference so a window dragged narrow still lays out something sane rather
/// than producing negative or microscopic tiles; the column clamp in [`grid_for`] does the rest.
pub fn usable_width_for(content_width: f32) -> f32 {
    (content_width - CHROME_W).max(MIN_USABLE_W)
}

/// The narrowest width the grid will lay out against.
pub const MIN_USABLE_W: f32 = 480.0;

/// The grid-density ladder, as the Settings page offers it: the `cols_adjust` handed to
/// [`grid_for`]. Ported from `GridDensity` in `GridGeometry.kt`.
pub mod density {
    pub const STEPS: [i32; 4] = [-1, 0, 1, 2];
    pub const LABELS: [&str; 4] = ["Fewer", "Default", "More", "Most"];
}

/// Grid geometry for a system's cover `ratio`.
///
/// The column count comes from the width the grid *actually* has, so a wider window fits more
/// covers at a consistent size rather than inflating three of them. The Kotlin original pins the
/// count to its reference width instead — a deliberate "big posters" choice that makes sense for
/// the Deck, whose panel is a fixed 1280x800 and never resizes. On a desktop window that can be
/// dragged to any width it stops being a choice and becomes a stuck grid: at 1650 units a
/// square-boxed system still showed three covers, each half a metre of screen.
///
/// `cols_adjust` is the separate density knob: positive adds columns (more, smaller covers),
/// negative removes them. It stays orthogonal to text scale, so a large-type user can still ask for
/// a denser grid.
pub fn grid_for(ratio: f32, usable_w: f32, ui_scale: f32, cols_adjust: i32) -> GridSpec {
    let base_cols = (usable_w / (TARGET_H * ui_scale * ratio + GAP)).round() as i32;
    let base_cols = base_cols.clamp(3, 7);
    let cols = (base_cols + cols_adjust).clamp(2, 8) as usize;
    // Fill the actual width exactly, rather than leaving a ragged right edge.
    let tile_w = (usable_w - (cols as f32 - 1.0) * GAP) / cols as f32;
    let tile_h = tile_w / ratio;
    GridSpec { cols, tile_w, tile_h, pitch: tile_h + LABEL_H + GAP }
}

/// Tiles of one height and each its own width, packed into rows left to right.
///
/// A row ends where the next tile would not fit, so rows hold different numbers of tiles and the
/// right edge is ragged — what a grid mixing cover shapes takes to show every cover whole at one
/// height. A grid of one shape packs into the same columns [`grid_for`] derives, so a system's grid
/// is the regular grid it always was.
///
/// Every row is the same height, which is what keeps the vertical geometry — [`GridSpec::pitch`],
/// the row window, the spacers — exactly what it was. Only *which* tiles share a row changes, and
/// with it how Up and Down find the next one: by horizontal position, through [`Packing::nearest`],
/// since a column number means nothing across rows of different counts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Packing {
    widths: Vec<f32>,
    rows: Vec<std::ops::Range<usize>>,
    row_of: Vec<usize>,
    centers: Vec<f32>,
}

impl Packing {
    /// Pack tiles of `widths` into rows no wider than `available`, [`GAP`] apart.
    pub fn pack(widths: Vec<f32>, available: f32) -> Self {
        // A hair of slack: a row of tiles sized to fill the width exactly adds up to it only to
        // within float rounding, and must not spill its last tile onto a row of its own for that.
        const SLACK: f32 = 0.5;
        let mut rows = Vec::new();
        let mut row_of = Vec::with_capacity(widths.len());
        let mut centers = Vec::with_capacity(widths.len());
        let (mut start, mut right) = (0, 0.0f32);
        for (i, w) in widths.iter().enumerate() {
            let mut left = if i == start { 0.0 } else { right + GAP };
            // A tile wider than the whole row still gets a row, of its own.
            if i > start && left + w > available + SLACK {
                rows.push(start..i);
                start = i;
                left = 0.0;
            }
            centers.push(left + w / 2.0);
            right = left + w;
            row_of.push(rows.len());
        }
        if start < widths.len() {
            rows.push(start..widths.len());
        }
        Packing { widths, rows, row_of, centers }
    }

    /// `count` tiles in rows of `cols` — the regular grid, for when every tile is one shape.
    #[cfg(test)]
    pub fn columns(count: usize, cols: usize) -> Self {
        let cols = cols.max(1);
        Self::pack(vec![1.0; count], cols as f32 + (cols as f32 - 1.0) * GAP)
    }

    pub fn len(&self) -> usize {
        self.widths.len()
    }

    pub fn rows(&self) -> &[std::ops::Range<usize>] {
        &self.rows
    }

    /// The row tile `index` is in — the last row's for an index past the end.
    pub fn row_of(&self, index: usize) -> usize {
        self.row_of.get(index).or(self.row_of.last()).copied().unwrap_or(0)
    }

    pub fn width(&self, index: usize) -> f32 {
        self.widths.get(index).copied().unwrap_or(0.0)
    }

    /// Where tile `index`'s centre sits, from its row's left edge.
    pub fn center(&self, index: usize) -> f32 {
        self.centers.get(index).copied().unwrap_or(0.0)
    }

    /// The tile in `row` whose centre is nearest `x` — where Up or Down lands.
    pub fn nearest(&self, row: usize, x: f32) -> usize {
        let Some(tiles) = self.rows.get(row) else { return self.len().saturating_sub(1) };
        tiles.clone().min_by(|a, b| (self.centers[*a] - x).abs().total_cmp(&(self.centers[*b] - x).abs())).unwrap_or(tiles.start)
    }
}

/// The rows a `viewport_h`-tall window shows at scroll offset `scroll`, widened by `margin` rows
/// on each side.
///
/// This is the whole of the grid's laziness, in one place because two callers have to agree on it
/// exactly: the art fetch decides what to decode from it, and the view decides what to build. If
/// they disagreed, the page would either show tiles with no art or reserve art for tiles that were
/// never drawn.
///
/// `viewport_h` may be an over-estimate — the caller passes the window height rather than measuring
/// the grid's own box — which widens the window and is always safe. Under-estimating is not, so
/// there is no floor on it to be clever with.
pub fn rows_in_view(
    spec: &GridSpec,
    row_count: usize,
    scroll: f32,
    viewport_h: f32,
    margin: usize,
) -> std::ops::Range<usize> {
    if row_count == 0 {
        return 0..0;
    }
    let pitch = spec.pitch.max(1.0);
    // Clamped the way the scrollable resolves its own offset. `scroll_to` stores whatever it is
    // handed, but reading it back pins it to the end of the content — so a jump to the last letter
    // records an offset further down than the grid can actually go, and without this the window
    // would sit past every row there is and the page would come out empty.
    let max_scroll = (row_count as f32 * pitch - viewport_h).max(0.0);
    let scroll = scroll.clamp(0.0, max_scroll);

    let first = (scroll / pitch).floor().max(0.0) as usize;
    // Plus one: a viewport that is not an exact multiple of the pitch always shows part of one
    // more row than it has room for.
    let visible = (viewport_h / pitch).ceil() as usize + 1;
    let end = first.saturating_add(visible).saturating_add(margin).min(row_count);
    first.saturating_sub(margin).min(end)..end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_of_one_shape_pack_into_the_columns_the_grid_derives() {
        for (ratio, width) in [(0.71, 1060.0), (1.37, 1060.0), (1.0, 1650.0)] {
            let spec = grid_for(ratio, width, 1.0, 0);
            let packing = Packing::pack(vec![spec.tile_h * ratio; 23], width);
            assert!(packing.rows()[..packing.rows().len() - 1].iter().all(|r| r.len() == spec.cols), "{ratio}: {:?}", packing.rows());
            assert_eq!(packing.rows().len(), 23usize.div_ceil(spec.cols));
        }
    }

    #[test]
    fn tiles_of_mixed_widths_wrap_where_the_next_would_not_fit() {
        // 100-wide boxes and 160-wide jewel cases, 24 apart, in a 500-wide row.
        let packing = Packing::pack(vec![100.0, 160.0, 100.0, 160.0, 160.0, 100.0, 600.0], 500.0);
        assert_eq!(packing.rows(), [0..3, 3..6, 6..7], "three, then three, then one too wide for any row");
        assert_eq!(packing.row_of(4), 1);
        assert_eq!(packing.center(1), 100.0 + GAP + 80.0);
        // Down from the far right of the first row lands on the far right of the next: centre 358
        // is nearer 418 than 264.
        assert_eq!(packing.nearest(1, packing.center(2)), 5);
        assert_eq!(packing.nearest(1, 0.0), 3);
        assert!(Packing::pack(Vec::new(), 500.0).rows().is_empty());
    }

    /// A spec whose pitch is a round 100, so the window arithmetic below reads as row numbers.
    fn spec_with_pitch_100() -> GridSpec {
        GridSpec { cols: 4, tile_w: 100.0, tile_h: 48.0, pitch: 100.0 }
    }

    #[test]
    fn the_window_covers_the_viewport_plus_its_margin() {
        let spec = spec_with_pitch_100();
        // Scrolled to row 10 of 200, showing 5 rows, with a 3-row margin either side.
        let rows = rows_in_view(&spec, 200, 1000.0, 500.0, 3);
        assert_eq!(rows, 7..19, "rows 10-15 are visible; 3 either side are the margin");
    }

    #[test]
    fn the_window_does_not_run_off_either_end() {
        let spec = spec_with_pitch_100();
        assert_eq!(rows_in_view(&spec, 200, 0.0, 500.0, 3), 0..9, "no negative rows at the top");

        // At the bottom the last row is the last one built, with nothing past it.
        let bottom = rows_in_view(&spec, 200, 200.0 * 100.0, 500.0, 3);
        assert_eq!(bottom.end, 200);
        assert!(bottom.contains(&199), "the last row is in the window: {bottom:?}");
    }

    #[test]
    fn an_offset_past_the_end_still_lands_on_the_last_rows() {
        // The regression this guards: `scroll_to` accepts an offset past the content and resolves
        // it to the end, so the recorded offset can be further down than the grid goes. Taken at
        // face value the window would start past the last row and the grid would draw nothing.
        let spec = spec_with_pitch_100();
        let rows = rows_in_view(&spec, 20, 99_999.0, 500.0, 3);
        assert!(rows.contains(&19), "the bottom of the grid is still built: {rows:?}");
        assert!(!rows.is_empty());
    }

    #[test]
    fn an_empty_grid_has_no_rows_to_build() {
        assert_eq!(rows_in_view(&spec_with_pitch_100(), 0, 0.0, 500.0, 3), 0..0);
    }

    #[test]
    fn a_viewport_taller_than_the_grid_takes_all_of_it() {
        let spec = spec_with_pitch_100();
        assert_eq!(rows_in_view(&spec, 6, 0.0, 5000.0, 3), 0..6);
    }

    #[test]
    fn a_wide_box_gets_fewer_columns_than_a_tall_one() {
        // A SNES box is landscape (1.37) and a PSP box is tall (0.58), so the same page fits far
        // fewer of the former across.
        let snes = grid_for(1.37, usable_width_for(REFERENCE_CONTENT_W), 1.0, 0);
        let psp = grid_for(0.58, usable_width_for(REFERENCE_CONTENT_W), 1.0, 0);
        assert!(snes.cols < psp.cols, "snes {} psp {}", snes.cols, psp.cols);
    }

    #[test]
    fn pitch_stays_in_a_narrow_band_across_systems() {
        // The point of deriving rather than tabulating: scroll rhythm and focus-travel distance
        // should not lurch when you change system, even though a PSP tile and a SNES tile are
        // completely different shapes.
        //
        // Asserted as a *spread* rather than against fixed bounds. The Kotlin source quotes a
        // "~288-314" band, but that figure is stale against its own constants — the same comment
        // claims GBA (ratio 1.00) lands on 5 columns where the formula it documents yields 4. The
        // formula is ported exactly; the narrowness is the property worth pinning.
        let pitches: Vec<f32> = [0.67, 0.71, 1.00, 1.11, 1.16, 1.37]
            .into_iter()
            .map(|r| grid_for(r, usable_width_for(REFERENCE_CONTENT_W), 1.0, 0).pitch)
            .collect();
        let min = pitches.iter().cloned().fold(f32::MAX, f32::min);
        let max = pitches.iter().cloned().fold(f32::MIN, f32::max);
        assert!(
            max / min < 1.25,
            "pitch spread should stay tight, got {min:.0}..{max:.0} ({:.2}x)",
            max / min
        );
    }

    #[test]
    fn tiles_always_fill_the_available_width_exactly() {
        for ratio in [0.67, 1.0, 1.37] {
            let usable = usable_width_for(REFERENCE_CONTENT_W);
            let spec = grid_for(ratio, usable, 1.0, 0);
            let used = spec.tile_w * spec.cols as f32 + GAP * (spec.cols as f32 - 1.0);
            assert!((used - usable).abs() < 0.01, "ratio {ratio} left {used} of {usable}");
        }
    }

    #[test]
    fn a_wider_surface_fits_more_covers_at_a_steady_size() {
        let reference = grid_for(0.71, usable_width_for(REFERENCE_CONTENT_W), 1.0, 0);
        let wide = grid_for(0.71, usable_width_for(1920.0), 1.0, 0);
        assert!(wide.cols > reference.cols, "a wider window earns more columns");
        // And the covers stay roughly the size they were, rather than ballooning to fill.
        let growth = wide.tile_w / reference.tile_w;
        assert!(growth < 1.35, "tiles should stay near their target size, grew {growth:.2}x");
    }

    #[test]
    fn the_grid_fills_the_width_it_is_given_with_no_rail_shaped_hole() {
        // The regression this guards: the chrome band once reserved ~200 units for a rail that was
        // not drawn, so tiles were sized for a width narrower than their container and left a gap
        // down the right of every grid. Whatever `CHROME_W` counts has to be what is on screen.
        for content_width in [1280.0, 1440.0, 1650.0, 1920.0] {
            let usable = usable_width_for(content_width);
            assert!(
                (content_width - usable - CHROME_W).abs() < 0.01,
                "at {content_width} the chrome took {} units, expected {CHROME_W}",
                content_width - usable
            );
            let spec = grid_for(1.0, usable, 1.15, 0);
            let used = spec.tile_w * spec.cols as f32 + GAP * (spec.cols as f32 - 1.0);
            assert!((used - usable).abs() < 0.01, "at {content_width} the tiles left {} spare", usable - used);
        }
    }

    #[test]
    fn a_very_narrow_window_still_lays_out_something_sane() {
        assert_eq!(usable_width_for(200.0), MIN_USABLE_W);
        let spec = grid_for(1.0, usable_width_for(200.0), 1.0, 0);
        assert!(spec.cols >= 2 && spec.tile_w > 0.0);
    }

    #[test]
    fn larger_type_means_fewer_bigger_columns_and_density_is_independent() {
        let normal = grid_for(0.71, usable_width_for(REFERENCE_CONTENT_W), 1.0, 0);
        let huge = grid_for(0.71, usable_width_for(REFERENCE_CONTENT_W), 1.5, 0);
        assert!(huge.cols <= normal.cols, "bigger type means fewer columns");

        // The density knob moves columns on its own, whatever the text scale is.
        let denser = grid_for(0.71, usable_width_for(REFERENCE_CONTENT_W), 1.5, 1);
        assert_eq!(denser.cols, huge.cols + 1);
    }
}
