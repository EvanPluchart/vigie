//! Time-keyed history in tiers (decision 0006): each tier keeps one bucket per period, so the
//! time axis stays true whatever the tick interval (1 s focused, 4 s in the background, 10 s
//! minimised). Buckets hold the mean of the values recorded in them and, on coarse tiers, their
//! peak. Gaps longer than a minute (the machine slept) stay empty instead of being made up.

use std::fmt::Write as _;

pub const SECOND: u64 = 1_000_000_000;

/// One tier: a bucket every `period_s` seconds, `len` buckets at most.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TierSpec {
    pub period_s: u32,
    pub len: usize,
    /// Keep the peak of each bucket next to its mean.
    pub peaks: bool,
}

/// Machine-wide figures: 1 s for 10 min, 10 s for 6 h, 1 min for 7 days.
pub const SYSTEM_TIERS: [TierSpec; 3] = [
    TierSpec {
        period_s: 1,
        len: 600,
        peaks: false,
    },
    TierSpec {
        period_s: 10,
        len: 2160,
        peaks: true,
    },
    TierSpec {
        period_s: 60,
        len: 10080,
        peaks: true,
    },
];

/// Gauges (memory in use) need no peaks: their mean is what the graph shows.
pub const GAUGE_TIERS: [TierSpec; 3] = [
    SYSTEM_TIERS[0],
    TierSpec {
        peaks: false,
        ..SYSTEM_TIERS[1]
    },
    TierSpec {
        peaks: false,
        ..SYSTEM_TIERS[2]
    },
];

/// One tracked process: 1 s for 10 min, 1 min for 6 h.
pub const PROCESS_TIERS: [TierSpec; 2] = [
    TierSpec {
        period_s: 1,
        len: 600,
        peaks: false,
    },
    TierSpec {
        period_s: 60,
        len: 360,
        peaks: true,
    },
];

/// A gap up to this long is filled with the value that ends it: rates are measured over the
/// whole gap, so the value is right for every second of it. A longer gap stays empty.
const MAX_FILL_NS: u64 = 60 * SECOND;

#[derive(Clone, Debug)]
struct Tier {
    period_ns: u64,
    capacity: usize,
    /// Closed buckets in a ring once full; grown in steps until then, so memory follows uptime.
    means: Vec<f32>,
    /// Empty when the tier keeps no peaks.
    peaks: Vec<f32>,
    keep_peaks: bool,
    /// Next write position once the ring is full.
    next: usize,
    /// The bucket being filled, by index (time / period).
    open: Option<u64>,
    sum: f64,
    count: u32,
    peak: f32,
    last_at: u64,
}

impl Tier {
    fn new(spec: TierSpec) -> Self {
        Self {
            period_ns: u64::from(spec.period_s.max(1)) * SECOND,
            capacity: spec.len.max(1),
            means: Vec::new(),
            peaks: Vec::new(),
            keep_peaks: spec.peaks,
            next: 0,
            open: None,
            sum: 0.0,
            count: 0,
            peak: f32::NAN,
            last_at: 0,
        }
    }

    fn record(&mut self, at_ns: u64, value: f32) {
        let bucket = at_ns / self.period_ns;
        match self.open {
            // Same bucket, or a clock that went backwards: add to the open bucket.
            Some(open) if bucket <= open => {}
            Some(open) => {
                self.close();
                let missing = (bucket - open - 1).min(self.capacity as u64);
                let fill = if at_ns.saturating_sub(self.last_at) <= MAX_FILL_NS {
                    value
                } else {
                    f32::NAN
                };
                for _ in 0..missing {
                    self.push(fill, fill);
                }
                self.open = Some(bucket);
            }
            None => self.open = Some(bucket),
        }
        if value.is_finite() {
            self.sum += f64::from(value);
            self.count += 1;
            self.peak = if self.peak.is_nan() {
                value
            } else {
                self.peak.max(value)
            };
        }
        self.last_at = at_ns;
    }

    fn open_mean(&self) -> f32 {
        if self.count == 0 {
            f32::NAN
        } else {
            (self.sum / f64::from(self.count)) as f32
        }
    }

    fn close(&mut self) {
        let (mean, peak) = (self.open_mean(), self.peak);
        self.push(mean, peak);
        self.sum = 0.0;
        self.count = 0;
        self.peak = f32::NAN;
    }

    fn push(&mut self, mean: f32, peak: f32) {
        if self.means.len() < self.capacity {
            if self.means.len() == self.means.capacity() {
                // Grow by half (64 at least), never past the capacity.
                let step = (self.means.len() / 2).max(64);
                let step = step.min(self.capacity - self.means.len());
                self.means.reserve_exact(step);
                if self.keep_peaks {
                    self.peaks.reserve_exact(step);
                }
            }
            self.means.push(mean);
            if self.keep_peaks {
                self.peaks.push(peak);
            }
        } else {
            self.means[self.next] = mean;
            if self.keep_peaks {
                self.peaks[self.next] = peak;
            }
            self.next = (self.next + 1) % self.capacity;
        }
    }

    /// Buckets kept, the open one included.
    fn len(&self) -> usize {
        self.means.len() + usize::from(self.open.is_some())
    }

    /// Bucket `i` counted from the newest (0 is the open bucket): (mean, peak).
    fn back(&self, i: usize) -> Option<(f32, f32)> {
        self.open?;
        if i == 0 {
            let mean = self.open_mean();
            return Some((mean, if self.keep_peaks { self.peak } else { mean }));
        }
        let closed = self.means.len();
        if i > closed {
            return None;
        }
        let newest = if closed < self.capacity {
            closed - 1
        } else {
            (self.next + self.capacity - 1) % self.capacity
        };
        let at = (newest + self.capacity - (i - 1)) % self.capacity;
        let mean = self.means[at];
        let peak = if self.keep_peaks { self.peaks[at] } else { mean };
        Some((mean, peak))
    }

    fn bytes(&self) -> usize {
        (self.means.capacity() + self.peaks.capacity()) * std::mem::size_of::<f32>()
    }
}

/// The history of one metric across tiers.
#[derive(Clone, Debug)]
pub struct History {
    tiers: Vec<Tier>,
}

impl History {
    pub fn new(specs: &[TierSpec]) -> Self {
        Self {
            tiers: specs.iter().map(|&s| Tier::new(s)).collect(),
        }
    }

    /// Records a value measured at `at_ns` (monotonic). NaN records time without a value.
    pub fn record(&mut self, at_ns: u64, value: f32) {
        for tier in &mut self.tiers {
            tier.record(at_ns, value);
        }
    }

    /// The newest value of the finest tier.
    pub fn last(&self) -> Option<f32> {
        self.tiers
            .first()
            .and_then(|t| t.back(0))
            .map(|(m, _)| m)
            .filter(|m| m.is_finite())
    }

    /// Mean of the finest tier over the last `seconds` before `now_ns`, with the share of that
    /// time that has a value (0 to 1). `None` when nothing was recorded in it.
    pub fn mean_over(&self, now_ns: u64, seconds: u64) -> Option<(f32, f32)> {
        let (mut sum, mut count) = (0.0f64, 0u32);
        let buckets = self.recent(0, now_ns, seconds, |_, mean, _| {
            if mean.is_finite() {
                sum += f64::from(mean);
                count += 1;
            }
        });
        (count > 0).then(|| {
            (
                (sum / f64::from(count)) as f32,
                count as f32 / buckets.max(1) as f32,
            )
        })
    }

    /// Calls `f(age_s, mean, peak)` for each bucket of `tier` in the last `seconds` before
    /// `now_ns`, newest first (gaps as NaN). Returns how many buckets that window holds.
    pub fn recent(
        &self,
        tier: usize,
        now_ns: u64,
        seconds: u64,
        mut f: impl FnMut(u64, f32, f32),
    ) -> usize {
        let Some(t) = self.tiers.get(tier) else {
            return 0;
        };
        let period_s = t.period_ns / SECOND;
        let window = (seconds / period_s.max(1)).max(1) as usize;
        let Some(open) = t.open else {
            return window;
        };
        let now_bucket = now_ns / t.period_ns;
        // Buckets between the newest one and now have no value.
        let skip = now_bucket.saturating_sub(open) as usize;
        for i in 0..window.saturating_sub(skip) {
            let Some((mean, peak)) = t.back(i) else { break };
            f((skip + i) as u64 * period_s, mean, peak);
        }
        window
    }

    /// The finest tier that covers `span_s` seconds.
    pub fn tier_for(&self, span_s: u64) -> usize {
        self.tiers
            .iter()
            .position(|t| t.period_ns / SECOND * t.capacity as u64 >= span_s)
            .unwrap_or(self.tiers.len().saturating_sub(1))
    }

    /// Period of a tier in seconds.
    pub fn period_s(&self, tier: usize) -> u64 {
        self.tiers.get(tier).map_or(1, |t| t.period_ns / SECOND)
    }

    /// Index of the newest bucket of a tier: changes when the tier has something new to show.
    pub fn newest_bucket(&self, tier: usize) -> Option<u64> {
        self.tiers.get(tier).and_then(|t| t.open)
    }

    /// How many buckets the finest tier holds (open one included).
    pub fn len(&self) -> usize {
        self.tiers.first().map_or(0, Tier::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bytes held by the buckets.
    pub fn bytes(&self) -> usize {
        self.tiers.iter().map(Tier::bytes).sum()
    }

    /// Fills `out` with the window ending `offset_s` seconds before `now_ns` and lasting
    /// `span_s`, in at most `columns` columns (oldest first). Each column holds the mean of
    /// its buckets and their peak; NaN where nothing was recorded.
    pub fn window(&self, now_ns: u64, span_s: u64, offset_s: u64, columns: usize, out: &mut Series) {
        out.means.clear();
        out.peaks.clear();
        let tier = self.tier_for(span_s);
        let period_s = self.period_s(tier);
        let buckets = (span_s / period_s).max(1) as usize;
        let n = buckets.min(columns.max(1));
        out.means.resize(n, f32::NAN);
        out.peaks.resize(n, f32::NAN);
        out.span_s = span_s;
        out.offset_s = offset_s;
        out.counts.clear();
        out.counts.resize(n, 0);
        let counts = &mut out.counts;
        // Age 0 is now; the window covers ages offset .. offset + span.
        self.recent(tier, now_ns, offset_s + span_s, |age, mean, peak| {
            if age < offset_s || !mean.is_finite() {
                return;
            }
            let from_end = ((age - offset_s) as f64 / span_s as f64 * n as f64) as usize;
            if from_end >= n {
                return;
            }
            let c = n - 1 - from_end;
            let m = &mut out.means[c];
            *m = if counts[c] == 0 { mean } else { *m + mean };
            counts[c] += 1;
            let p = &mut out.peaks[c];
            *p = if p.is_nan() { peak } else { p.max(peak) };
        });
        for (m, &k) in out.means.iter_mut().zip(&out.counts) {
            if k > 1 {
                *m /= f32::from(k);
            }
        }
    }
}

/// A window of a history, ready to draw: one value per column, oldest first, NaN for gaps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Series {
    pub means: Vec<f32>,
    pub peaks: Vec<f32>,
    pub span_s: u64,
    pub offset_s: u64,
    /// Buckets per column, reused between windows.
    counts: Vec<u16>,
}

impl Series {
    /// Largest peak, 0 when empty.
    pub fn max(&self) -> f32 {
        self.peaks
            .iter()
            .chain(&self.means)
            .copied()
            .filter(|v| v.is_finite())
            .fold(0.0, f32::max)
    }
}

/// SVG path commands for `series` in a `width` × `height` box, `max` at the top: the line of the
/// means, and the area under the peaks (equal to the means on fine tiers). Gaps break both.
/// Writes into the given strings, which are cleared first.
pub fn graph_paths(
    series: &Series,
    max: f32,
    width: f32,
    height: f32,
    line: &mut String,
    area: &mut String,
) {
    line.clear();
    area.clear();
    let n = series.means.len();
    if n == 0 || max <= 0.0 {
        return;
    }
    let step = width / (n.max(2) - 1) as f32;
    let x = |i: usize| if n == 1 { width } else { step * i as f32 };
    let y = |v: f32| height - (v / max).clamp(0.0, 1.0) * height;
    let mut drawing = false;
    for (i, &v) in series.means.iter().enumerate() {
        if !v.is_finite() {
            drawing = false;
            continue;
        }
        let _ = write!(
            line,
            "{}{:.1} {:.1}",
            if drawing { " L " } else if line.is_empty() { "M " } else { " M " },
            x(i),
            y(v)
        );
        drawing = true;
    }
    // One closed shape per run of values.
    let mut start = None;
    for i in 0..=n {
        let v = series.peaks.get(i).copied().unwrap_or(f32::NAN);
        match (v.is_finite(), start) {
            (true, None) => {
                start = Some(i);
                let _ = write!(
                    area,
                    "{}{:.1} {height:.1} L {:.1} {:.1}",
                    if area.is_empty() { "M " } else { " M " },
                    x(i),
                    x(i),
                    y(v)
                );
            }
            (true, Some(_)) => {
                let _ = write!(area, " L {:.1} {:.1}", x(i), y(v));
            }
            (false, Some(_)) => {
                let _ = write!(area, " L {:.1} {height:.1} Z", x(i - 1));
                start = None;
            }
            (false, None) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> History {
        History::new(&[
            TierSpec {
                period_s: 1,
                len: 4,
                peaks: false,
            },
            TierSpec {
                period_s: 2,
                len: 4,
                peaks: true,
            },
        ])
    }

    fn values(h: &History, tier: usize, now: u64, secs: u64) -> Vec<(u64, f32, f32)> {
        let mut v = Vec::new();
        h.recent(tier, now * SECOND, secs, |age, m, p| v.push((age, m, p)));
        v
    }

    #[test]
    fn buckets_follow_time_not_ticks() {
        let mut h = tiny();
        // Ticks every 2 s: each value is the rate over its 2 s, so it fills both seconds.
        for (t, v) in [(1, 10.0), (3, 30.0), (5, 50.0)] {
            h.record(t * SECOND, v);
        }
        let fine: Vec<f32> = values(&h, 0, 5, 4).iter().map(|x| x.1).collect();
        assert_eq!(fine, [50.0, 50.0, 30.0, 30.0]);
        // The 2 s tier holds one value per bucket, with its peak.
        let coarse: Vec<(f32, f32)> = values(&h, 1, 5, 6).iter().map(|x| (x.1, x.2)).collect();
        assert_eq!(coarse, [(50.0, 50.0), (30.0, 30.0), (10.0, 10.0)]);
    }

    #[test]
    fn coarse_buckets_keep_mean_and_peak() {
        let mut h = tiny();
        h.record(2 * SECOND, 10.0);
        h.record(2 * SECOND + SECOND / 2, 30.0);
        h.record(3 * SECOND, 20.0);
        assert_eq!(values(&h, 1, 3, 2), [(0, 20.0, 30.0)]);
    }

    #[test]
    fn long_gaps_stay_empty() {
        let mut h = tiny();
        h.record(SECOND, 1.0);
        // Asleep for two minutes.
        h.record(121 * SECOND, 2.0);
        let fine = values(&h, 0, 121, 4);
        assert_eq!(fine[0].1, 2.0);
        assert!(fine[1..].iter().all(|x| x.1.is_nan()));
    }

    #[test]
    fn rings_wrap_and_memory_follows_uptime() {
        let mut h = History::new(&SYSTEM_TIERS);
        assert_eq!(h.bytes(), 0);
        for t in 1..=700 {
            h.record(t * SECOND, t as f32);
        }
        assert_eq!(h.len(), 601, "600 closed buckets and the open one");
        assert_eq!(h.last(), Some(700.0));
        let (mean, coverage) = h.mean_over(700 * SECOND, 10).unwrap();
        assert_eq!((mean, coverage), (695.5, 1.0));
        // Ten minutes in, the 7-day tier holds a dozen buckets, not 10 080.
        assert!(h.bytes() < 8 * 1024, "{} bytes", h.bytes());
    }

    #[test]
    fn windows_pick_a_tier_and_fold_columns() {
        let mut h = History::new(&SYSTEM_TIERS);
        for t in 1..=600 {
            h.record(t * SECOND, if t > 540 { 100.0 } else { 0.0 });
        }
        let mut s = Series::default();
        // The last minute at full resolution.
        h.window(600 * SECOND, 60, 0, 300, &mut s);
        assert_eq!(s.means.len(), 60);
        assert!(s.means.iter().all(|&v| v == 100.0));
        // Ten minutes in 100 columns: 6 s each, the last 10 columns at 100.
        h.window(600 * SECOND, 600, 0, 100, &mut s);
        assert_eq!(s.means.len(), 100);
        assert_eq!(s.means[99], 100.0);
        assert_eq!(s.means[89], 0.0);
        // Panned back by 5 minutes: nothing at 100.
        h.window(600 * SECOND, 60, 300, 300, &mut s);
        assert!(s.means.iter().all(|&v| v == 0.0));
        // Six hours come from the 10 s tier; mostly empty after ten minutes.
        assert_eq!(h.tier_for(6 * 3600), 1);
        h.window(600 * SECOND, 6 * 3600, 0, 300, &mut s);
        assert!(s.means[0].is_nan() && s.means[299].is_finite());
    }

    #[test]
    fn paths_break_at_gaps() {
        let series = Series {
            means: vec![0.0, 50.0, f32::NAN, 100.0],
            peaks: vec![0.0, 50.0, f32::NAN, 100.0],
            ..Series::default()
        };
        let (mut line, mut area) = (String::new(), String::new());
        graph_paths(&series, 100.0, 300.0, 100.0, &mut line, &mut area);
        assert_eq!(line, "M 0.0 100.0 L 100.0 50.0 M 300.0 0.0");
        assert_eq!(area.matches('Z').count(), 2);
        graph_paths(&Series::default(), 100.0, 1.0, 1.0, &mut line, &mut area);
        assert!(line.is_empty() && area.is_empty());
    }
}
