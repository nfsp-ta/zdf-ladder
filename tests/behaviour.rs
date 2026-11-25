//! Does it behave like a 4-pole ladder? (Overflow checks are on in test builds, so these also
//! show that nothing overflows at extreme settings.)

use zdf_ladder::{cutoff_hz, Ladder, CUTOFF_MAX, INPUT_LIMIT, SAMPLE_RATE};

/// Peak output amplitude for a sine of `freq` Hz and amplitude `amp`, after it settles.
fn sine_gain(cut: i32, res: i32, freq: f64, amp: f64) -> f64 {
    let mut f = Ladder::new();
    f.set(cut, res);
    let fs = SAMPLE_RATE as f64;
    let n = SAMPLE_RATE as usize; // 1 s
    let mut peak = 0i32;
    for i in 0..n {
        let x = (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / fs).sin()).round() as i32;
        let y = f.tick(x);
        if i > n / 2 {
            peak = peak.max(y.abs());
        }
    }
    peak as f64 / amp
}

#[test]
fn dc_gain_is_the_input_tanh() {
    for &x in &[1_000i32, 10_000, 60_000, -60_000] {
        let mut f = Ladder::new();
        f.set(40 << 8, 0);
        let mut y = 0;
        for _ in 0..SAMPLE_RATE {
            y = f.tick(x);
        }
        let want = 65536.0 * (x as f64 / 65536.0).tanh();
        assert!(
            (y as f64 - want).abs() < 0.002 * want.abs() + 3.0,
            "DC {x} -> {y}, want {want}"
        );
    }
}

#[test]
fn minus_12_db_at_cutoff_and_24_db_per_octave() {
    let cut = 64 << 8;
    let fc = cutoff_hz(cut) as f64;
    let at_fc = sine_gain(cut, 0, fc, 1000.0);
    assert!((at_fc - 0.25).abs() < 0.02, "gain at fc {at_fc}");
    let pass = sine_gain(cut, 0, fc / 8.0, 1000.0);
    assert!(pass > 0.96 && pass < 0.98, "passband gain {pass}");
    let oct = sine_gain(cut, 0, fc * 2.0, 1000.0);
    let two_oct = sine_gain(cut, 0, fc * 4.0, 1000.0);
    let slope_db = 20.0 * (oct / two_oct).log10();
    assert!(
        slope_db > 18.0 && slope_db < 26.0,
        "slope {slope_db} dB/oct"
    );
}

#[test]
fn resonance_peaks_at_cutoff() {
    let cut = 64 << 8;
    let fc = cutoff_hz(cut) as f64;
    let flat = sine_gain(cut, 0, fc, 4000.0);
    let peaked = sine_gain(cut, 90, fc, 4000.0);
    assert!(peaked > 4.0 * flat, "flat {flat} peaked {peaked}");
}

fn ring(res: i32) -> (i32, i32) {
    let mut f = Ladder::new();
    f.set(70 << 8, res);
    let mut peak_late = 0;
    let mut peak_all = 0;
    for i in 0..2 * SAMPLE_RATE {
        let y = f.tick(if i == 0 { 100_000 } else { 0 });
        peak_all = peak_all.max(y.abs());
        if i > SAMPLE_RATE {
            peak_late = peak_late.max(y.abs());
        }
    }
    (peak_late, peak_all)
}

#[test]
fn self_oscillates_at_full_resonance_and_stays_bounded() {
    let (late, all) = ring(127);
    assert!(late > 5_000, "no self-oscillation: {late}");
    let (late, _) = ring(124);
    assert!(late > 2_000, "res 124 should already sing: {late}");
    assert!(all < 200_000, "runaway: {all}");
}

#[test]
fn decays_below_self_oscillation() {
    let (late, _) = ring(122);
    assert!(late <= 1, "still ringing: {late}");
}

#[test]
fn survives_extremes() {
    let mut f = Ladder::new();
    for &(cut, res) in &[
        (0, 127),
        (CUTOFF_MAX, 127),
        (CUTOFF_MAX, 0),
        (0, 0),
        (-5, 500),
        (99_999, -3),
    ] {
        f.set(cut, res);
        for i in 0..20_000 {
            let x = match i % 3 {
                0 => i32::MAX,
                1 => i32::MIN,
                _ => 0,
            };
            let y = f.tick(x);
            assert!(y.abs() < 4 * INPUT_LIMIT, "cut {cut} res {res}: {y}");
        }
    }
}
