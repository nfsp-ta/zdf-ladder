//! Bit-exactness against Clipper's original C (reference/golden.c prints these hashes).

use zdf_ladder::{softclip, Coefs, SAMPLE_RATE};

struct Golden {
    coef: u64,
    softclip: u64,
    stream: u64,
}

fn golden() -> Option<Golden> {
    match SAMPLE_RATE {
        44_100 => Some(Golden {
            coef: 0x8f6f231057a97aaf,
            softclip: 0xa1c6375cea0fc592,
            stream: 0xd265fe0f3632e29b,
        }),
        48_000 => Some(Golden {
            coef: 0xa9c5457514e71c36,
            softclip: 0xa1c6375cea0fc592,
            stream: 0x103dea60bf05a5b5,
        }),
        _ => None,
    }
}

struct Fnv(u64);
impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf29ce484222325)
    }
    fn add(&mut self, v: i32) {
        for b in (v as u32).to_le_bytes() {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}

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
fn coefficients_match_c_for_every_setting() {
    let Some(g) = golden() else { return };
    let mut h = Fnv::new();
    for cut in 0..=(127 << 8) {
        for res in 0..=127 {
            for v in Coefs::new(cut, res).raw() {
                h.add(v);
            }
        }
    }
    assert_eq!(h.0, g.coef);
}

#[test]
fn softclip_matches_c() {
    let Some(g) = golden() else { return };
    let mut h = Fnv::new();
    for x in -300_000..=300_000 {
        h.add(softclip(x));
    }
    assert_eq!(h.0, g.softclip);
}

#[test]
fn two_million_samples_match_c() {
    let Some(g) = golden() else { return };
    let mut h = Fnv::new();
    let mut r = XorShift(0x12345678);
    let mut s = [0i32; 4];
    let mut c = Coefs::default();
    let (mut kind, mut sh, mut phase, mut inc) = (0u32, 16u32, 0u32, 0u32);
    for n in 0..2_000_000u32 {
        if n % 2048 == 0 {
            kind = r.next() % 4;
            sh = 12 + r.next() % 8;
            inc = r.next() >> 6;
        }
        if n % 32 == 0 {
            let cut = (r.next() % 32513) as i32;
            let res = (r.next() % 128) as i32;
            c = Coefs::new(cut, res);
        }
        let x = match kind {
            0 => (r.next() as i32) >> sh,
            1 => {
                phase = phase.wrapping_add(inc);
                (phase as i32) >> sh
            }
            2 if n % 512 == 0 => 131_072,
            _ => 0,
        };
        h.add(c.tick(x, &mut s));
    }
    for v in s {
        h.add(v);
    }
    assert_eq!(h.0, g.stream);
}
