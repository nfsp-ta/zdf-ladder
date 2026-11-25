// SPDX-License-Identifier: GPL-3.0-only
//
// Rust port of the MOOG filter model from the Clipper firmware for the M-VAVE FM-1
// (firmware/src/dsp.c: softclip, ladder_coef, ladder_tick).
// Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments, and Clipper contributors.
// Rust port: 2026.

//! A four-pole (24 dB/oct) Moog-style transistor-ladder low-pass filter in pure integer math.
//!
//! The design is Vadim Zavalishin's TPT ladder (*The Art of VA Filter Design*, ch. 5): four
//! trapezoidal one-pole low-passes, with the feedback solved within the same sample (zero-delay
//! feedback), so cutoff and resonance stay accurate up to the top of the range. A tanh at the ladder input
//! (the differential pair) gives soft saturation, and the filter self-oscillates from resonance
//! 124 of 127 (k = 4). Some passband is restored at high resonance (input × (1 + k/2)).
//!
//! It needs no FPU or hardware divide in the audio path. Per sample it uses a handful of
//! 64-bit multiplies and one table lookup. The coefficient update uses two 32-bit divides,
//! so call it once per block, not per sample.
//!
//! The output is bit-for-bit identical to Clipper's `ladder_tick` at the default 44.1 kHz
//! sample rate.
//!
//! # Levels
//! * Input is an `i32` sample clamped to ±[`INPUT_LIMIT`] (±131072).
//! * The tanh knee sits around ±65536. A 16-bit full-scale signal (±32767) is mostly
//!   clean, and anything hotter drives the filter into saturation.
//! * Output is on the same scale as the input: unity passband gain at zero resonance, apart
//!   from the input tanh (65536·tanh(x/65536), so 10000 in gives about 9925 out). Raising the resonance
//!   lowers the passband (to about 0.6 at full resonance). Clipper halves the output before mixing.
//! * Self-oscillation is moderate, about ±5600 at full resonance, because the tanh limits it.
//!
//! # Example
//! ```
//! use zdf_ladder::Ladder;
//! let mut f = Ladder::new();
//! f.set(64 << 8, 90);             // cutoff (0..=127<<8), resonance (0..=127)
//! let y = f.tick(12_000);
//! # let _ = y;
//! ```
#![no_std]
#![forbid(unsafe_code)]

mod tables {
    include!(concat!(env!("OUT_DIR"), "/tables.rs"));
}

pub use tables::SAMPLE_RATE;
use tables::{CUTOFF_HZ, SVF_G, TANH_Q15};

/// Highest cutoff value: 127 << 8 (16 kHz). 0 is 30 Hz; the curve is exponential.
pub const CUTOFF_MAX: i32 = 127 << 8;
/// Cutoff units per octave (127·256 / log2(16000/30) ≈ 3589). Add this per octave of key tracking.
pub const CUTOFF_PER_OCTAVE: i32 = 3589;
/// Cutoff units per semitone (≈ 299).
pub const CUTOFF_PER_SEMITONE: i32 = 299;
/// Highest resonance value. Self-oscillation starts at 124 (k = 4).
pub const RESONANCE_MAX: i32 = 127;
/// Input samples are clamped to ± this.
pub const INPUT_LIMIT: i32 = 131_072;

/// Soft clip: `tanh(x / 32768) * 32767`, linearly interpolated from a 257-point table.
/// Saturates at `tanh(2) * 32767` (31590) for |x| ≥ 65536.
#[inline]
pub fn softclip(x: i32) -> i32 {
    let a = x.unsigned_abs();
    let i = (a >> 8) as usize;
    let y = if i >= 256 {
        TANH_Q15[256] as i32
    } else {
        let t0 = TANH_Q15[i] as i32;
        let t1 = TANH_Q15[i + 1] as i32;
        t0 + (((t1 - t0) * (a & 255) as i32) >> 8)
    };
    if x < 0 {
        -y
    } else {
        y
    }
}

/// Approximate cutoff frequency in Hz for a cutoff value (0..=[`CUTOFF_MAX`]), for display.
pub fn cutoff_hz(cutoff: i32) -> u32 {
    let c = cutoff.clamp(0, CUTOFF_MAX);
    let i = (c >> 8) as usize;
    let a = CUTOFF_HZ[i] as i32;
    if i < 127 {
        let b = CUTOFF_HZ[i + 1] as i32;
        (a + (((b - a) * (c & 255)) >> 8)) as u32
    } else {
        a as u32
    }
}

/// Filter coefficients for one cutoff/resonance setting. Cheap to copy. Recompute per control
/// block (Clipper uses 32 samples), not per sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Coefs {
    g: i32,   // G = g / (1 + g), Q16
    h: i32,   // 1 - G, Q16
    g4: i32,  // G^4, Q16
    inv: i32, // 1 / (1 + k G^4), Q16
    k: i32,   // feedback, Q12 (0 .. 4.13)
}

impl Default for Coefs {
    /// Fully open (cutoff 127 << 8), no resonance.
    fn default() -> Self {
        Self::new(CUTOFF_MAX, 0)
    }
}

impl Coefs {
    /// Coefficients from the cutoff (0..=127 << 8, i.e. 0..=32512, exponential 30 Hz .. 16 kHz)
    /// and the resonance (0..=127). Out-of-range values are clamped.
    pub fn new(cutoff: i32, resonance: i32) -> Self {
        let cut = cutoff.clamp(0, CUTOFF_MAX);
        let i = (cut >> 8) as usize;
        let mut g = SVF_G[i] as i32;
        if i < 127 {
            g += ((SVF_G[i + 1] as i32 - g) * (cut & 255)) >> 8;
        }
        Self::from_g(g, resonance)
    }

    /// Coefficients from a prewarped gain `g = tan(π fc / fs)` in Q12 (clamped to 1..=3·4096,
    /// i.e. G ≤ 0.75, which keeps the state bounded), for your own cutoff mapping.
    pub fn from_g(g_q12: i32, resonance: i32) -> Self {
        let g = g_q12.clamp(1, 3 * 4096);
        let big_g = (((g as u32) << 16) / (4096 + g) as u32) as i32;
        let g2 = mul_sh(big_g, big_g, 16);
        let g4 = mul_sh(g2, g2, 16);
        let k = resonance.clamp(0, RESONANCE_MAX) * 16900 / 127;
        let inv = (0xFFFF_FFFFu32 / (65536 + mul_sh(k, g4, 12)) as u32) as i32;
        Coefs {
            g: big_g,
            h: 65536 - big_g,
            g4,
            inv,
            k,
        }
    }

    /// Raw fixed-point values `[G, 1-G, G^4, inv, k]`, for tests and debugging.
    #[doc(hidden)]
    pub fn raw(&self) -> [i32; 5] {
        [self.g, self.h, self.g4, self.inv, self.k]
    }

    /// One sample through the ladder whose four stage states are `s` (Q12).
    /// Use this form to share one `Coefs` between several voices.
    #[inline]
    pub fn tick(&self, x: i32, s: &mut [i32; 4]) -> i32 {
        let g = self.g as i64;
        let mut inp = (x.clamp(-INPUT_LIMIT, INPUT_LIMIT) as i64) << 12;
        inp += (inp * self.k as i64) >> 13; // x (1 + k/2)

        // The ladder's output, solved for the zero-delay feedback.
        let mut acc = ((g * s[0] as i64) >> 16) + s[1] as i64;
        acc = ((g * acc) >> 16) + s[2] as i64;
        acc = ((g * acc) >> 16) + s[3] as i64;
        acc = (self.h as i64 * acc) >> 16;
        let y = ((((self.g4 as i64 * inp) >> 16) + acc) * self.inv as i64) >> 16;
        let u = inp - ((self.k as i64 * y) >> 12); // the ladder's input, Q12

        // tanh at 65536, applied to u / 2 (softclip's range), with the bits below
        // one unit added back (the slope is about 1 there).
        let hh = u >> 13;
        let mut uq: i32 = if hh <= -65536 || hh >= 65536 {
            softclip(if hh < 0 { -65536 } else { 65536 }) << 13
        } else {
            (softclip(hh as i32) << 13).wrapping_add((u - (hh << 13)) as i32)
        };

        // Four trapezoidal one-poles.
        for sj in s.iter_mut() {
            let v = ((g * uq.wrapping_sub(*sj) as i64 + 32768) >> 16) as i32;
            uq = v.wrapping_add(*sj);
            *sj = uq.wrapping_add(v);
        }
        uq.wrapping_add(2048) >> 12
    }
}

#[inline]
fn mul_sh(a: i32, b: i32, sh: u32) -> i32 {
    ((a as i64 * b as i64) >> sh) as i32
}

/// A single filter: coefficients plus its four stage states.
#[derive(Clone, Copy, Debug)]
pub struct Ladder {
    coefs: Coefs,
    s: [i32; 4],
}

impl Default for Ladder {
    fn default() -> Self {
        Self::new()
    }
}

impl Ladder {
    /// A filter fully open (cutoff 127 << 8), no resonance, zero state.
    pub fn new() -> Self {
        Ladder {
            coefs: Coefs::new(CUTOFF_MAX, 0),
            s: [0; 4],
        }
    }

    /// Set cutoff (0..=127 << 8) and resonance (0..=127). Do this per block, not per sample.
    pub fn set(&mut self, cutoff: i32, resonance: i32) {
        self.coefs = Coefs::new(cutoff, resonance);
    }

    /// Use precomputed coefficients.
    pub fn set_coefs(&mut self, coefs: Coefs) {
        self.coefs = coefs;
    }

    pub fn coefs(&self) -> &Coefs {
        &self.coefs
    }

    /// Clear the state (silence, stop any self-oscillation).
    pub fn reset(&mut self) {
        self.s = [0; 4];
    }

    /// One sample.
    #[inline]
    pub fn tick(&mut self, x: i32) -> i32 {
        self.coefs.tick(x, &mut self.s)
    }

    /// Filter a block in place.
    pub fn process(&mut self, buf: &mut [i32]) {
        let c = self.coefs;
        for x in buf.iter_mut() {
            *x = c.tick(*x, &mut self.s);
        }
    }
}
