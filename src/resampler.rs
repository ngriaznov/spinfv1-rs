//! Boundary resampling: run the chip at its native 32,768 Hz inside a
//! host running at any sample rate.
//!
//! The FV-1's delay times, LFO rates and filter coefficients are all
//! per-sample constants, so the only way a program sounds exactly like
//! hardware is to run the VM at the chip rate and convert at the
//! boundary. [`HostedFv1`] does that turnkey: hand it the host rate,
//! feed it one frame per host sample, and it internally converts to
//! 32,768 Hz, runs the chip, and converts back.
//!
//! The converter itself, [`StreamResampler`], is a streaming polyphase
//! windowed-sinc design: a Kaiser-windowed sinc kernel precomputed at
//! construction into a bank of fractional-delay phases (with linear
//! blending between adjacent phases), applied over a small history
//! window per output frame. Cutoff sits below the lower of the two
//! Nyquist frequencies, so the same kernel serves both decimation
//! (host → chip) and interpolation (chip → host). Everything allocates
//! up front; the per-sample path is allocation-free and real-time safe.

use crate::vm::Fv1;

/// The FV-1's nominal crystal rate in Hz.
pub const CHIP_RATE: f64 = 32_768.0;

const TAPS: usize = 32;
const PHASES: usize = 128;
const KAISER_BETA: f64 = 9.0;

/// Zeroth-order modified Bessel function, for the Kaiser window.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    for k in 1..32 {
        term *= (x / (2.0 * f64::from(k))).powi(2);
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
    }
    sum
}

/// Streaming stereo sample-rate converter (windowed-sinc polyphase).
///
/// Push input frames at `in_rate`; pull output frames at `out_rate`.
/// Frames become available once enough history exists to center the
/// kernel, giving a fixed latency of `TAPS / 2` input samples.
pub struct StreamResampler {
    /// `(PHASES + 1) * TAPS` kernel, row `p` holding the fractional
    /// delay `p / PHASES`; the extra row makes phase blending branch-free.
    kernel: Vec<f32>,
    /// Input frame history; `history[i]` is absolute input index `base + i`.
    history: Vec<(f32, f32)>,
    /// Absolute input index of `history[0]`.
    base: u64,
    /// Total input frames pushed.
    pushed: u64,
    /// Fractional read position on the input timeline.
    pos: f64,
    /// Input samples advanced per output frame (`in_rate / out_rate`).
    step: f64,
}

impl StreamResampler {
    /// A converter from `in_rate` Hz to `out_rate` Hz. Rates only matter
    /// as a ratio. Panics if either rate is not finite and positive.
    #[must_use]
    pub fn new(in_rate: f64, out_rate: f64) -> Self {
        assert!(
            in_rate.is_finite() && in_rate > 0.0 && out_rate.is_finite() && out_rate > 0.0,
            "sample rates must be finite and positive"
        );
        let step = in_rate / out_rate;
        // Cutoff below the narrower Nyquist, with margin for the finite
        // kernel's transition band; normalized to input samples.
        let cutoff = 0.5 * 0.9 * (out_rate / in_rate).min(1.0);
        let half = (TAPS / 2) as f64;
        let mut kernel = vec![0.0f32; (PHASES + 1) * TAPS];
        for (p, row) in kernel.chunks_exact_mut(TAPS).enumerate() {
            let frac = p as f64 / PHASES as f64;
            let mut sum = 0.0;
            for (k, tap) in row.iter_mut().enumerate() {
                // Offset of input sample `k` from the read position.
                let t = k as f64 - (half - 1.0) - frac;
                // sin(2·fc·π·t)/(π·t) → 2·fc as t → 0 (the sinc's peak,
                // not 1.0 — getting this wrong leaks unfiltered signal
                // straight through integer-phase rows).
                let sinc = if t == 0.0 {
                    2.0 * cutoff
                } else {
                    let x = core::f64::consts::PI * t;
                    (2.0 * cutoff * x).sin() / x
                };
                let w = if t.abs() <= half {
                    bessel_i0(KAISER_BETA * (1.0 - (t / half).powi(2)).max(0.0).sqrt())
                        / bessel_i0(KAISER_BETA)
                } else {
                    0.0
                };
                *tap = (sinc * w) as f32;
                sum += f64::from(*tap);
            }
            // Normalize each phase row for exactly unity DC gain.
            for tap in row.iter_mut() {
                *tap = (f64::from(*tap) / sum) as f32;
            }
        }
        Self {
            kernel,
            history: Vec::with_capacity(4 * TAPS),
            base: 0,
            pushed: 0,
            pos: 0.0,
            step,
        }
    }

    /// Append one input frame.
    pub fn push(&mut self, frame: (f32, f32)) {
        self.history.push(frame);
        self.pushed += 1;
    }

    /// Produce the next output frame, or `None` until enough input has
    /// been pushed to center the kernel window.
    pub fn pull(&mut self) -> Option<(f32, f32)> {
        let i0 = self.pos.floor() as i64 - (TAPS as i64 / 2 - 1);
        let last_needed = i0 + TAPS as i64 - 1;
        if last_needed >= self.pushed as i64 {
            return None;
        }
        let frac = self.pos - self.pos.floor();
        let x = frac * PHASES as f64;
        let p = x.floor() as usize;
        let blend = (x - p as f64) as f32;
        let row_a = &self.kernel[p * TAPS..(p + 1) * TAPS];
        let row_b = &self.kernel[(p + 1) * TAPS..(p + 2) * TAPS];

        let (mut l, mut r) = (0.0f32, 0.0f32);
        for k in 0..TAPS {
            let idx = i0 + k as i64;
            // Indices before the first pushed frame read as silence.
            let (sl, sr) = if idx < self.base as i64 {
                (0.0, 0.0)
            } else {
                self.history[(idx as u64 - self.base) as usize]
            };
            let h = row_a[k] + (row_b[k] - row_a[k]) * blend;
            l += sl * h;
            r += sr * h;
        }
        self.pos += self.step;

        // Drop history the kernel can no longer reach.
        let min_needed = (self.pos.floor() as i64 - (TAPS as i64 / 2 - 1)).max(0) as u64;
        if min_needed > self.base {
            let drop = (min_needed - self.base) as usize;
            self.history.drain(..drop.min(self.history.len()));
            self.base = min_needed;
        }
        Some((l, r))
    }
}

/// An [`Fv1`] running at the authentic chip rate behind a host-rate
/// stream: feed one frame per host sample, get one frame back, and the
/// program's delays, LFOs and filters behave exactly as on hardware.
///
/// Latency is [`Self::latency`] host samples (the two converters'
/// kernel centering), reported so hosts can compensate.
pub struct HostedFv1 {
    chip: Fv1,
    down: StreamResampler,
    up: StreamResampler,
    latency: usize,
    host_rate: f64,
    chip_rate: f64,
}

impl HostedFv1 {
    /// Wrap a chip for a host running at `host_rate` Hz.
    #[must_use]
    pub fn new(chip: Fv1, host_rate: f64) -> Self {
        Self::with_chip_rate(chip, host_rate, CHIP_RATE)
    }

    /// Wrap a chip clocked at `chip_rate` Hz instead of the stock
    /// crystal — the hardware "clock mod": underclocking stretches
    /// delays and darkens the bandwidth, overclocking does the
    /// opposite. Panics if either rate is not finite and positive.
    #[must_use]
    pub fn with_chip_rate(chip: Fv1, host_rate: f64, chip_rate: f64) -> Self {
        let mut hosted = Self {
            chip,
            down: StreamResampler::new(host_rate, chip_rate),
            up: StreamResampler::new(chip_rate, host_rate),
            latency: 0,
            host_rate,
            chip_rate,
        };
        // Prime with silence until output frames flow, so `process`
        // always has a frame ready; the count is the pipeline latency.
        while hosted.step((0.0, 0.0)).is_none() {
            hosted.latency += 1;
        }
        hosted
    }

    /// Swap the crystal on a running chip: rebuilds the two converters
    /// for the new `chip_rate` and re-primes them. Chip state — delay
    /// RAM, registers, the loaded program — is untouched, so tails
    /// carry across the switch (re-priming runs a few milliseconds of
    /// silence through the chip, as on a real clock change). Panics if
    /// `chip_rate` is not finite and positive.
    pub fn set_chip_rate(&mut self, chip_rate: f64) {
        self.down = StreamResampler::new(self.host_rate, chip_rate);
        self.up = StreamResampler::new(chip_rate, self.host_rate);
        self.chip_rate = chip_rate;
        self.latency = 0;
        while self.step((0.0, 0.0)).is_none() {
            self.latency += 1;
        }
    }

    /// The chip clock in Hz ([`CHIP_RATE`] unless modded).
    #[must_use]
    pub fn chip_rate(&self) -> f64 {
        self.chip_rate
    }

    /// The wrapped chip (load programs, set pots, inspect state).
    pub fn chip_mut(&mut self) -> &mut Fv1 {
        &mut self.chip
    }

    /// The wrapped chip, read-only.
    #[must_use]
    pub fn chip(&self) -> &Fv1 {
        &self.chip
    }

    /// Pipeline latency in host samples.
    #[must_use]
    pub fn latency(&self) -> usize {
        self.latency
    }

    fn step(&mut self, frame: (f32, f32)) -> Option<(f32, f32)> {
        self.down.push(frame);
        while let Some((cl, cr)) = self.down.pull() {
            let out = self.chip.process(cl, cr);
            self.up.push(out);
        }
        self.up.pull()
    }

    /// Process one host-rate frame through the chip.
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        // After priming, the up-converter always has a frame in steady
        // state; silence covers the unreachable startup edge.
        self.step((l, r)).unwrap_or((0.0, 0.0))
    }
}
