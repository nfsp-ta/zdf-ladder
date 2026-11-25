// SPDX-License-Identifier: GPL-3.0-only
//
// A 32-bit-product version of the MOOG filter model from the Clipper firmware for the M-VAVE FM-1
// (firmware/src/dsp.c: ladder_coef, ladder_tick), after this crate's 64-bit port in the crate root.
// Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments, and Clipper contributors.

//! The same ladder with 32-bit multiplies only, for cores without a 32×32→64 multiply
//! (Cortex-M0+, RP2040), where each 64-bit product in [`Coefs`](crate::Coefs) is a software call.
//!
//! The algorithm and every coefficient are the same. Each product is two exact 32-bit
//! multiplies (the variable split at the shift), and the signal and the four stage states keep 8
//! fractional bits instead of 12, so every sum fits in an `i32`. It is **not** bit-for-bit
//! Clipper: the output follows the 64-bit one to within a few LSB of a 16-bit signal, apart from
//! self-oscillation (resonance 124 and up), which is a limit cycle that rounding moves in phase.
//! On a Cortex-M0+ it costs about 440 instructions a sample, against about 970 for the 64-bit
//! version.
//!
//! Its coefficients take the same cutoff and resonance as [`Coefs`](crate::Coefs), or a
//! prewarped gain and a feedback of your own ([`Coefs32::from_g_k`]), and its states are
//! separate from the 64-bit filter's (the fractional bits differ), so don't swap one for the
//! other on a running filter.

use crate::tables::SVF_G;
use crate::{softclip, CUTOFF_MAX, INPUT_LIMIT, RESONANCE_MAX};

/// Feedback (Q12) at the highest resonance setting.
const K_MAX: i32 = 16_900;
/// Largest prewarped gain (Q12): G <= 0.75 keeps the states bounded.
const G_MAX: i32 = 3 * 4096;
/// Fractional bits of the signal and the stage states.
const FRAC: u32 = 8;

/// `c * v >> SH`, exactly, for `c` in 0..2^16, `v` below 2^27 in magnitude and `SH` in 9..=16, in
/// two 32-bit multiplies: `v` split at bit `SH`.
#[inline(always)]
fn mulq<const SH: u32>(c: i32, v: i32) -> i32 {
    c * (v >> SH) + ((c as u32 * (v & ((1 << SH) - 1)) as u32) >> SH) as i32
}

/// `c * v >> 16` rounded to nearest, with the same limits.
#[inline(always)]
fn mulq16_round(c: i32, v: i32) -> i32 {
    c * (v >> 16) + ((c as u32 * (v & 0xFFFF) as u32 + 0x8000) >> 16) as i32
}

/// Filter coefficients for one cutoff/resonance setting, for [`Coefs32::tick`]. Cheap to copy.
/// Recompute per control block, not per sample (two 32-bit divides).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Coefs32 {
    g: i32,   // G = g / (1 + g), Q16
    h: i32,   // 1 - G, Q16
    g4: i32,  // G^4, Q16
    inv: i32, // 1 / (1 + k G^4), Q16
    k: i32,   // feedback, Q12 (0 .. 4.13)
}

impl Default for Coefs32 {
    /// Fully open (cutoff 127 << 8), no resonance.
    fn default() -> Self {
        Self::new(CUTOFF_MAX, 0)
    }
}

impl Coefs32 {
    /// Silent and open, for a `const` start.
    pub const ZERO: Self = Self {
        g: 0,
        h: 0,
        g4: 0,
        inv: 0,
        k: 0,
    };

    /// The feedback (Q12) at the highest resonance setting, 16900: [`Coefs32::k_for`] of 127.
    pub const K_MAX: i32 = K_MAX;

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
    /// i.e. G ≤ 0.75) and the resonance (0..=127), for your own cutoff mapping.
    pub fn from_g(g_q12: i32, resonance: i32) -> Self {
        Self::from_g_k(g_q12, Self::k_for(resonance))
    }

    /// As [`Coefs32::from_g`] with the feedback `k` itself in Q12 (clamped to 0..=16900), for
    /// resonance in steps finer than 1/127.
    pub fn from_g_k(g_q12: i32, k_q12: i32) -> Self {
        let g = g_q12.clamp(1, G_MAX) as u32;
        let big_g = (g << 16) / (4096 + g);
        let g2 = (big_g * big_g) >> 16;
        let g4 = (g2 * g2) >> 16;
        let k = k_q12.clamp(0, K_MAX) as u32;
        let inv = 0xFFFF_FFFF / (65536 + ((k * g4) >> 12));
        Coefs32 {
            g: big_g as i32,
            h: 65536 - big_g as i32,
            g4: g4 as i32,
            inv: inv as i32,
            k: k as i32,
        }
    }

    /// The feedback (Q12) of a resonance setting 0..=127 (clamped).
    pub fn k_for(resonance: i32) -> i32 {
        resonance.clamp(0, RESONANCE_MAX) * K_MAX / RESONANCE_MAX
    }

    /// One sample through the ladder whose four stage states are `s` (8 fractional bits). Use
    /// this form to share one `Coefs32` between several voices. Same levels as
    /// [`Coefs::tick`](crate::Coefs::tick): the input is clamped to ±[`INPUT_LIMIT`].
    #[inline]
    pub fn tick(&self, x: i32, s: &mut [i32; 4]) -> i32 {
        let g = self.g;
        let mut inp = x.clamp(-INPUT_LIMIT, INPUT_LIMIT) << FRAC;
        inp += mulq::<13>(self.k, inp); // x (1 + k/2)

        // The ladder's output, solved for the zero-delay feedback.
        let mut acc = mulq::<16>(g, s[0]) + s[1];
        acc = mulq::<16>(g, acc) + s[2];
        acc = mulq::<16>(g, acc) + s[3];
        acc = mulq::<16>(self.h, acc);
        let y = mulq::<16>(self.inv, mulq::<16>(self.g4, inp) + acc);
        let u = inp - mulq::<12>(self.k, y); // the ladder's input

        // tanh at 65536, applied to u / 2 (softclip's range), with the bits below one unit added
        // back (the slope is about 1 there).
        let hh = u >> (FRAC + 1);
        let mut uq = if hh <= -65536 || hh >= 65536 {
            softclip(if hh < 0 { -65536 } else { 65536 }) << (FRAC + 1)
        } else {
            (softclip(hh) << (FRAC + 1)) + (u - (hh << (FRAC + 1)))
        };

        // Four trapezoidal one-poles.
        for sj in s.iter_mut() {
            let v = mulq16_round(g, uq - *sj);
            uq = v + *sj;
            *sj = uq + v;
        }
        (uq + (1 << (FRAC - 1))) >> FRAC
    }
}

/// A single 32-bit-product filter: coefficients plus its four stage states.
#[derive(Clone, Copy, Debug)]
pub struct Ladder32 {
    coefs: Coefs32,
    s: [i32; 4],
}

impl Default for Ladder32 {
    fn default() -> Self {
        Self::new()
    }
}

impl Ladder32 {
    /// A filter fully open (cutoff 127 << 8), no resonance, zero state.
    pub fn new() -> Self {
        Ladder32 {
            coefs: Coefs32::new(CUTOFF_MAX, 0),
            s: [0; 4],
        }
    }

    /// Set cutoff (0..=127 << 8) and resonance (0..=127). Do this per block, not per sample.
    pub fn set(&mut self, cutoff: i32, resonance: i32) {
        self.coefs = Coefs32::new(cutoff, resonance);
    }

    /// Use precomputed coefficients.
    pub fn set_coefs(&mut self, coefs: Coefs32) {
        self.coefs = coefs;
    }

    pub fn coefs(&self) -> &Coefs32 {
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
