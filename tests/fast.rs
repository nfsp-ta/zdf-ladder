//! The 32-bit-product ladder against the 64-bit (bit-exact) one, and the same behaviour checks.

use zdf_ladder::{Coefs, Coefs32, Ladder32, CUTOFF_MAX, INPUT_LIMIT, SAMPLE_RATE};

struct XorShift(u32);
impl XorShift {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
}

#[test]
fn it_follows_the_64_bit_ladder() {
    let mut r = XorShift(0x2468ace1);
    let mut worst = 0;
    for cut in (0..=CUTOFF_MAX).step_by(2048) {
        // Self-oscillation (124 and up) is a limit cycle that rounding moves in phase.
        for res in [0, 40, 80, 100, 115] {
            let (a, b) = (Coefs32::new(cut, res), Coefs::new(cut, res));
            let (mut sa, mut sb) = ([0i32; 4], [0i32; 4]);
            for n in 0..20_000 {
                let x = match (n / 4000) % 3 {
                    0 => (r.next() as i32) >> 17,
                    1 => ((n as f64 * 0.07).sin() * 20_000.0) as i32,
                    _ => 0,
                };
                let (ya, yb) = (a.tick(x, &mut sa), b.tick(x, &mut sb));
                if n > 2000 {
                    worst = worst.max((ya - yb).abs());
                }
            }
        }
    }
    assert!(worst <= 24, "worst difference {worst}");
}

#[test]
fn same_dc_gain_as_the_64_bit_one() {
    // The same coefficients give the same gain, to the fraction bits' rounding (which the
    // feedback amplifies near self-oscillation).
    for cut in (0..=CUTOFF_MAX).step_by(97) {
        for res in [0, 33, 127] {
            let (c, c64) = (Coefs32::new(cut, res), Coefs::new(cut, res));
            let (mut a, mut b) = ([0i32; 4], [0i32; 4]);
            let mut last = (0, 0);
            for _ in 0..4000 {
                last = (c.tick(5000, &mut a), c64.tick(5000, &mut b));
            }
            let tol = 8.max(last.1.abs() / 64);
            assert!((last.0 - last.1).abs() <= tol, "{cut} {res}: {last:?}");
        }
    }
}

#[test]
fn it_is_a_four_pole_low_pass() {
    let gain = |hz: f64| {
        let mut f = Ladder32::new();
        f.set(80 << 8, 0);
        let fs = SAMPLE_RATE as f64;
        let mut peak = 0;
        for i in 0..SAMPLE_RATE as usize {
            let x = (4000.0 * (2.0 * std::f64::consts::PI * hz * i as f64 / fs).sin()) as i32;
            let y = f.tick(x);
            if i > SAMPLE_RATE as usize / 2 {
                peak = peak.max(y.abs());
            }
        }
        peak as f64 / 4000.0
    };
    assert!(gain(200.0) > 0.95);
    let (one, two) = (gain(3000.0), gain(6000.0));
    assert!(two < one * 0.1 && two < 0.02, "{one} {two}");
}

#[test]
fn it_self_oscillates_boundedly_and_stays_in_range() {
    let mut f = Ladder32::new();
    f.set(70 << 8, 127);
    let mut r = XorShift(7);
    let mut peak = 0;
    for n in 0..50_000 {
        let x = if n < 100 { (r.next() as i32) >> 16 } else { 0 };
        peak = peak.max(f.tick(x).abs());
    }
    assert!(peak > 1000 && peak < 12_000, "{peak}");
    // The loudest input at every setting stays in range, with no overflow (test builds check).
    for cut in (0..=CUTOFF_MAX).step_by(4096) {
        for res in [0, 127] {
            let mut f = Ladder32::new();
            f.set(cut, res);
            let mut worst = 0;
            for n in 0..4000 {
                let x = if n % 64 < 32 {
                    INPUT_LIMIT
                } else {
                    -INPUT_LIMIT
                };
                worst = worst.max(f.tick(x).abs());
            }
            assert!(worst < 1 << 17, "{cut} {res}: {worst}");
        }
    }
}

#[test]
fn out_of_range_settings_are_clamped() {
    for g in [-5, 0, 1, 100, 4096, 12288, 99_999] {
        for k in [-5, 0, 8000, 16900, 99_999] {
            let c = Coefs32::from_g_k(g, k);
            let mut s = [0; 4];
            assert!(c.tick(1000, &mut s).abs() < 1 << 17);
        }
    }
    assert_eq!(
        Coefs32::from_g(4096, 200),
        Coefs32::from_g_k(4096, Coefs32::K_MAX)
    );
}
