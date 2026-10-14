//! Frame-interval measurement, for deciding between renderers on the machine that matters.
//!
//! iced compiles in two backends and picks at runtime, so `ICED_BACKEND=tiny-skia` swaps GPU
//! rendering for CPU rendering without a rebuild. On a desktop that choice is nearly free; on a
//! Deck it is a real trade between a ~220 MB GPU floor and a CPU that has to rasterise every
//! cover. This is what tells you which way it falls, there rather than here.
//!
//! Off unless `DECKHAND_FRAMETIMES` is set, and it costs a `None` check per redraw when off.
//!
//! # Reading the output
//!
//! What is measured is the interval *between* redraws, not the time spent inside one — the app
//! cannot see the latter. The two backends pace themselves differently, and that changes what the
//! same number means:
//!
//! - **wgpu presents on vsync**, so its interval pins to the refresh period (16.67 ms at 60 Hz)
//!   however little work it did. That says it never misses a frame; it does not say what a frame
//!   cost. Only the tail — p95, p99, max — carries information.
//! - **tiny-skia is not vsync-paced**, so its interval is roughly the work itself, and the median
//!   is a real render cost.
//!
//! So compare the two on their tails, and read tiny-skia's median as a cost while treating wgpu's
//! as a floor.

use std::time::Instant;

/// How many intervals go into one report. At 60 Hz this is a line every four seconds; uncapped on
/// tiny-skia it is far more often, which is the point — the report is per sample count, not per
/// wall-clock second, so both backends are summarised over the same amount of evidence.
const BATCH: usize = 240;

/// A rolling frame-interval recorder.
#[derive(Debug)]
pub struct FrameLog {
    last: Option<Instant>,
    intervals: Vec<f32>,
}

impl FrameLog {
    /// One recorder if `DECKHAND_FRAMETIMES` is set in the environment, otherwise nothing.
    pub fn from_env() -> Option<Self> {
        std::env::var_os("DECKHAND_FRAMETIMES").map(|_| Self {
            last: None,
            intervals: Vec::with_capacity(BATCH),
        })
    }

    /// Record a redraw, reporting once a batch has built up.
    ///
    /// The first tick only sets the clock: an interval needs two frames, and counting from process
    /// start would put the whole of window creation into the first sample.
    pub fn tick(&mut self, now: Instant) {
        if let Some(last) = self.last {
            self.intervals.push((now - last).as_secs_f32() * 1000.0);
        }
        self.last = Some(now);
        if self.intervals.len() >= BATCH {
            eprintln!("{}", self.report());
            self.intervals.clear();
        }
    }

    /// The batch as one line. Percentiles rather than a mean, because the mean of a frame series
    /// says nothing useful — a run of good frames hides exactly the stalls you are looking for.
    fn report(&self) -> String {
        let mut sorted = self.intervals.clone();
        sorted.sort_by(f32::total_cmp);
        format!(
            "FRAMES n={} p50={:.2} p95={:.2} p99={:.2} max={:.2} (ms)",
            sorted.len(),
            percentile(&sorted, 0.50),
            percentile(&sorted, 0.95),
            percentile(&sorted, 0.99),
            percentile(&sorted, 1.0),
        )
    }
}

/// The value at `q` through an already-sorted slice, by nearest rank: the smallest sample with at
/// least `q` of the batch at or below it, so p50 of a hundred frames is the fiftieth.
///
/// Empty reads as 0 rather than panicking. A diagnostic must never be the thing that takes the app
/// down, and a batch is only ever empty here if it was reported before a frame landed.
fn percentile(sorted: &[f32], q: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (q.clamp(0.0, 1.0) * sorted.len() as f32).ceil().max(1.0) as usize;
    sorted[rank - 1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn percentiles_pick_the_expected_places_in_the_order() {
        let sorted: Vec<f32> = (1..=100).map(|n| n as f32).collect();
        assert_eq!(percentile(&sorted, 0.0), 1.0);
        assert_eq!(percentile(&sorted, 0.50), 50.0);
        assert_eq!(percentile(&sorted, 1.0), 100.0);
    }

    #[test]
    fn an_empty_batch_reads_as_zero_rather_than_panicking() {
        assert_eq!(percentile(&[], 0.5), 0.0);
    }

    #[test]
    fn the_first_tick_only_starts_the_clock() {
        // Counting from process start would fold window creation into the first interval and put a
        // hundreds-of-milliseconds outlier at the head of every run.
        let mut log = FrameLog { last: None, intervals: Vec::new() };
        let t0 = Instant::now();
        log.tick(t0);
        assert!(log.intervals.is_empty(), "one frame is not an interval");
        log.tick(t0 + Duration::from_millis(16));
        assert_eq!(log.intervals.len(), 1);
    }

    #[test]
    fn intervals_are_recorded_in_milliseconds() {
        let mut log = FrameLog { last: None, intervals: Vec::new() };
        let t0 = Instant::now();
        log.tick(t0);
        log.tick(t0 + Duration::from_millis(20));
        assert!((log.intervals[0] - 20.0).abs() < 0.01, "{:?}", log.intervals);
    }

    #[test]
    fn a_report_names_every_percentile_it_measured() {
        let mut log = FrameLog { last: None, intervals: vec![10.0, 20.0, 30.0] };
        let line = log.report();
        assert!(line.contains("n=3"), "{line}");
        assert!(line.contains("p50=20.00"), "{line}");
        assert!(line.contains("max=30.00"), "{line}");
        log.intervals.clear();
    }
}
