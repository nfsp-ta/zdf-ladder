# zdf-ladder

A four-pole (24 dB/oct) Moog-style ladder low-pass filter in pure integer math, as a
`no_std` Rust crate with no dependencies. It is a port of the **MOOG** filter model from the
[Clipper](https://github.com/isod89/clipper-fm1) firmware for the M-VAVE FM-1 (`firmware/src/dsp.c`,
commit `7bc15a9`).

* Zavalishin TPT ladder: four trapezoidal one-poles with the feedback solved in the same sample (zero-delay feedback).
* A tanh at the ladder input for warm drive and soft-limited self-oscillation.
* Passband compensation at high resonance: input × (1 + k/2).
* No floats and no divides in the audio path. It runs on a Cortex-M0+ (RP2040) without an FPU.
* **Bit-for-bit identical to Clipper's C** at 44.1 kHz (checked by `tests/bit_exact.rs`).

## Use

```toml
[dependencies]
zdf-ladder = { path = "../zdf-ladder" }
```

```rust
use zdf_ladder::{Ladder, Coefs, CUTOFF_PER_SEMITONE};

let mut f = Ladder::new();

// Once per control block (e.g. every 32 samples):
let cutoff = (64 << 8) + key_tracking_semitones * CUTOFF_PER_SEMITONE;
f.set(cutoff, resonance);          // cutoff 0..=32512 (30 Hz–16 kHz, exponential), resonance 0..=127

// Per sample, or f.process(&mut block):
let y = f.tick(x);
```

For polyphony you can share one `Coefs` between voices and keep a `[i32; 4]` state per voice,
using `coefs.tick(x, &mut state)`.

### Levels

| | |
|---|---|
| Input | `i32`, clamped to ±131072 |
| Saturation knee | ±65536 (a 16-bit full-scale signal is mostly clean, hotter drives it) |
| Output | same scale as input, through 65536·tanh(x/65536); passband drops to about 0.6 at full resonance |
| Self-oscillation | from resonance 124, about ±5600 at 127 |

### Sample rate

The two lookup tables are generated at build time for one sample rate. The default is
44100 Hz, as in Clipper. To change it, set `ZDF_LADDER_FS`, for example in your firmware
project's `.cargo/config.toml`:

```toml
[env]
ZDF_LADDER_FS = "48000"
```

`zdf_ladder::SAMPLE_RATE` reports the rate the crate was built for.

## Cost on the RP2040

Each sample takes about 270 Thumb instructions plus 12 64-bit multiplies (`__aeabi_lmul`,
done in software on the M0+). That estimate comes from compiling the same C with clang for
Cortex-M0+, which uses the LLVM backend Rust uses, so it should be in the same range. Expect
roughly **500–700 cycles per sample**. At 133 MHz and 44.1 kHz a core has about 3000 cycles
per sample, so budget about one core-fifth per filtered voice. Ways to make it cheaper:

* Run it from RAM, not XIP flash. `tick` is `#[inline]`, so write your voice loop as a
  function in your firmware and put it in your linker script's RAM-code section
  (for example `#[link_section = ".data.ram_func"]`). The filter code is then inlined into RAM.
* Update coefficients per block. `Coefs::new` has two 32-bit divides, which run in software
  unless the HAL's hardware-divider intrinsics are enabled.
* Or use an RP2350 (Cortex-M33), which multiplies 32×32→64 in one instruction.

A cheaper, non-bit-exact 32-bit variant is possible if this turns out to be too slow.

## Tests

```sh
cargo test                          # bit-exactness + behaviour at 44.1 kHz
ZDF_LADDER_FS=48000 cargo test      # and at 48 kHz
```

The expected hashes come from `reference/golden.c`, which compiles Clipper's original C
(`reference/ladder_ref.h`, extracted verbatim) and prints them:

```sh
cc -O2 -o golden reference/golden.c -lm && ./golden 44100
```

## License

GPL-3.0-only, the same as Clipper. This crate is a derivative of its code. If you distribute
firmware that includes it, that firmware must also be distributed under GPL-3.0 with its
source. Copyright (C) 2026 Leo Kuroshita / Hügelton Instruments and Clipper contributors.
