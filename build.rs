//! Generates the two lookup tables the filter needs, for the sample rate chosen with the
//! `ZDF_LADDER_FS` environment variable (default 44100, as in Clipper).
//!
//! Formulas are the same as Clipper's `tools/gen_tables.py`, so at 44100 Hz the tables are
//! identical to the firmware's.

use std::{env, fmt::Write as _, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=ZDF_LADDER_FS");

    let fs_hz: u32 = match env::var("ZDF_LADDER_FS") {
        Ok(v) => v.trim().parse().unwrap_or_else(|_| {
            panic!("ZDF_LADDER_FS must be an integer sample rate in Hz, got {v:?}")
        }),
        Err(_) => 44_100,
    };
    assert!(
        (8_000..=192_000).contains(&fs_hz),
        "ZDF_LADDER_FS = {fs_hz} is outside 8000..=192000"
    );
    let fs = fs_hz as f64;

    // Cutoff curve: 30 Hz .. 16 kHz, exponential over 0..=127.
    let fc: Vec<f64> = (0..128)
        .map(|v| 30.0 * (16000.0f64 / 30.0).powf(v as f64 / 127.0))
        .collect();

    // Prewarped one-pole gain g = tan(pi fc / fs), Q12 (truncated, like Python's int()).
    let svf_g: Vec<u16> = fc
        .iter()
        .map(|&f| (4096.0 * (std::f64::consts::PI * f.min(0.45 * fs) / fs).tan()) as u16)
        .collect();

    // tanh over |x| in 0..2, 257 points, Q15.
    let tanh_q15: Vec<i16> = (0..257)
        .map(|i| (32767.0 * (i as f64 / 256.0 * 2.0).tanh()) as i16)
        .collect();

    let cutoff_hz: Vec<u16> = fc.iter().map(|f| f.round() as u16).collect();

    let mut out = String::new();
    writeln!(
        out,
        "/// Sample rate the tables were generated for (set with `ZDF_LADDER_FS` at build time)."
    )
    .unwrap();
    writeln!(out, "pub const SAMPLE_RATE: u32 = {fs_hz};").unwrap();
    emit(&mut out, "SVF_G", "u16", &svf_g);
    emit(&mut out, "TANH_Q15", "i16", &tanh_q15);
    emit(&mut out, "CUTOFF_HZ", "u16", &cutoff_hz);

    let path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("tables.rs");
    fs::write(path, out).unwrap();
}

fn emit<T: std::fmt::Display>(out: &mut String, name: &str, ty: &str, vals: &[T]) {
    writeln!(out, "pub(crate) static {name}: [{ty}; {}] = [", vals.len()).unwrap();
    for chunk in vals.chunks(12) {
        out.push_str("   ");
        for v in chunk {
            write!(out, " {v},").unwrap();
        }
        out.push('\n');
    }
    out.push_str("];\n");
}
