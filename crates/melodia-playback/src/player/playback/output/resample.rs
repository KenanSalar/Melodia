//! The band-limited kernel [`super::convert::Converter`] interpolates with.
//!
//! A windowed sinc, tabulated once for the process and read by every converter: evaluating the
//! sine and the window per tap per output frame would cost far more than the lookup. Between two
//! rows of the table the kernel is linear, which is what [`PHASES`] is sized against.
//!
//! **Each output frame is scaled by its weights' own sum**, so the kernel's gain at DC is exactly
//! one whatever the phase. Left to the table, the sum drifts a little from phase to phase, and a
//! phase that cycles at a fixed ratio turns that drift into a faint tone.
//!
//! **Downsampling stretches the kernel** rather than keeping its length and lowering the cutoff, so
//! the transition band stays a fixed fraction of the *output's* Nyquist. The cost grows with the
//! stretch, which is why it stops at [`MAX_STRETCH`]: past it the kernel is as wide as it gets and
//! only the cutoff keeps falling.

use std::f64::consts::PI;
use std::ops::Range;
use std::sync::LazyLock;

/// Taps either side of the point being interpolated, at a stretch of one.
const HALF_TAPS: usize = 64;

/// How far the kernel widens for a step past one.
const MAX_STRETCH: usize = 4;

/// Frames the kernel reaches past the centre at its widest, and so the most lookahead a converter
/// ever pulls.
pub const REACH: usize = HALF_TAPS * MAX_STRETCH;

/// The frames [`Kernel::weigh`] lays its weights across.
pub const SPAN: usize = 2 * REACH;

/// Where in [`SPAN`] the centre frame sits, with [`REACH`] frames after it.
pub const CENTRE: usize = REACH - 1;

/// Table rows per frame of distance from the kernel's centre.
const PHASES: usize = 1024;

/// Entries in one table row: a tap per frame of distance, and a zero past the last.
const ROW: usize = HALF_TAPS + 1;

/// The fixed point a stretched side's places are walked in: a row count above these bits, a
/// fraction of a row in them.
const FIXED_BITS: u32 = 16;
const FIXED_ONE: u32 = 1 << FIXED_BITS;

/// The fraction's unit, as a fraction of a row.
const FIXED_UNIT: f32 = 1.0 / 65_536.0;
const _: () = assert!(FIXED_ONE == 65_536, "the fraction is the sixteen bits a u16 keeps");

/// The four-term Blackman-Harris window's coefficients.
const WINDOW_TERMS: [f64; 4] = [0.358_75, 0.488_29, 0.141_28, 0.011_68];

static KERNEL: LazyLock<Kernel> = LazyLock::new(Kernel::build);

/// The shared kernel. Its first call builds the table, so a converter asks for it on the control
/// thread and the audio thread only ever reads it.
pub fn kernel() -> &'static Kernel {
    &KERNEL
}

/// Frames the kernel reaches either side of the point it interpolates, stepping `step` source
/// frames per output frame.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a stretch clamped to [1, MAX_STRETCH] times HALF_TAPS is a small positive count"
)]
pub fn reach(step: f64) -> usize {
    (count(HALF_TAPS) * stretch(step)).ceil() as usize
}

fn stretch(step: f64) -> f64 {
    step.clamp(1.0, count(MAX_STRETCH))
}

/// `n` as a float. Every caller passes a tap or table index, which `f64` holds exactly.
#[expect(clippy::cast_precision_loss, reason = "tap and table indices are exact in f64")]
fn count(n: usize) -> f64 {
    n as f64
}

/// What [`Kernel::weigh`] wrote: the frames the weights cover, and the gain that brings their sum
/// to one.
pub struct Weighed {
    pub taps: Range<usize>,
    pub gain: f32,
}

pub struct Kernel {
    /// Row `r`, entry `t` is the kernel `t + r / PHASES` frames from its centre: one row more than
    /// [`PHASES`], so the last closes the interval the first opens. A row holds every tap one side
    /// of the point sits at, so an unstretched side reads two rows straight through.
    table: Box<[f32]>,
    /// Each row's taps summed, so an unstretched output frame's weights never have to be.
    row_sums: Box<[f32]>,
}

impl Kernel {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the table is stored at the width of the samples it weighs"
    )]
    fn build() -> Self {
        // Half the window's main lobe below Nyquist, so the stopband opens at Nyquist rather than
        // straddling it. The four-term window's main lobe is four bins either side.
        let cutoff = 1.0 - 4.0 / count(HALF_TAPS);
        let table: Box<[f32]> = (0..=PHASES)
            .flat_map(|row| (0..ROW).map(move |tap| count(tap) + count(row) / count(PHASES)))
            .map(|distance| prototype(distance, cutoff) as f32)
            .collect();
        let row_sums = table.chunks_exact(ROW).map(|row| row.iter().sum()).collect();
        Self { table, row_sums }
    }

    /// Write into `weights` what each frame of a [`SPAN`]-long window contributes to an output
    /// frame `phase` past the one at [`CENTRE`], stepping `step` source frames per output frame.
    ///
    /// Only the frames in [`Weighed::taps`] are written or ever read, so a converter only has to
    /// have pulled [`reach`] frames ahead of its centre.
    pub fn weigh(&self, phase: f64, step: f64, weights: &mut [f32]) -> Weighed {
        let reach = reach(step);
        let taps = CENTRE + 1 - reach..CENTRE + 1 + reach;
        let (before, after) = weights[taps.clone()].split_at_mut(reach);
        // The centre frame and those before it sit `phase` frames and on from the point, the rest
        // `1 - phase` and on.
        let sum = if step <= 1.0 {
            self.rows_between(phase, before.iter_mut().rev())
                + self.rows_between(1.0 - phase, after.iter_mut())
        } else {
            let per_frame = count(PHASES) / stretch(step);
            self.stepped(phase * per_frame, per_frame, before.iter_mut().rev())
                + self.stepped((1.0 - phase) * per_frame, per_frame, after.iter_mut())
        };
        Weighed { taps, gain: sum.recip() }
    }

    /// Fill one unstretched side, whose taps are all `distance` past a whole frame from the point,
    /// and return their sum.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a distance in [0, 1] times PHASES is a row index"
    )]
    fn rows_between<'a>(&self, distance: f64, side: impl Iterator<Item = &'a mut f32>) -> f32 {
        let place = distance * count(PHASES);
        // A distance of a whole frame is the last row, read as the far end of the one before it.
        let row = (place as usize).min(PHASES - 1);
        let between = (place - count(row)) as f32;
        let near = &self.table[row * ROW..][..HALF_TAPS];
        let far = &self.table[(row + 1) * ROW..][..HALF_TAPS];
        for (weight, (near, far)) in side.zip(near.iter().zip(far)) {
            *weight = near + (far - near) * between;
        }
        let (near, far) = (self.row_sums[row], self.row_sums[row + 1]);
        near + (far - near) * between
    }

    /// Fill one stretched side, whose taps sit `per_frame` rows apart from `place`, and return
    /// their sum.
    ///
    /// The places are walked in fixed point, so a tap costs an add, a shift and a mask rather than
    /// two conversions between float and integer.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "places are non-negative and inside the table, well within the fixed point's range"
    )]
    fn stepped<'a>(
        &self,
        place: f64,
        per_frame: f64,
        side: impl Iterator<Item = &'a mut f32>,
    ) -> f32 {
        let mut place = (place * f64::from(FIXED_ONE)) as u64;
        let per_frame = (per_frame * f64::from(FIXED_ONE)) as u64;
        let mut sum = 0.0;
        for weight in side {
            let whole = (place >> FIXED_BITS) as usize;
            let between = f32::from(place as u16) * FIXED_UNIT;
            let (row, tap) = (whole % PHASES, whole / PHASES);
            let near = self.table[row * ROW + tap];
            let far = self.table[(row + 1) * ROW + tap];
            *weight = near + (far - near) * between;
            sum += *weight;
            place += per_frame;
        }
        sum
    }
}

/// The windowed sinc `distance` frames from its centre, zero from [`HALF_TAPS`] out.
fn prototype(distance: f64, cutoff: f64) -> f64 {
    let half = count(HALF_TAPS);
    if distance >= half {
        return 0.0;
    }
    let x = cutoff * distance;
    let sinc = if x == 0.0 { 1.0 } else { (PI * x).sin() / (PI * x) };
    sinc * window(distance / half)
}

/// The Blackman-Harris window at `edge` of the way from its centre to its end.
fn window(edge: f64) -> f64 {
    let [a0, a1, a2, a3] = WINDOW_TERMS;
    a0 + a1 * (PI * edge).cos() + a2 * (2.0 * PI * edge).cos() + a3 * (3.0 * PI * edge).cos()
}
