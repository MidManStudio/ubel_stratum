// Writes a corpus of floats with what Rust's own `{}` (Display) prints for each, one line per value:
//     d <16 hex digits of the f64 bit pattern> <text>
//     f <8 hex digits of the f32 bit pattern> <text>
// harness/float_diff_check.c reads the lines and prints the same numbers with the runtime's ubel_float.h; any
// difference is a bug in the port. Deterministic (fixed seeds), so two toolchains can be compared too.
//
//     rustc --edition 2021 -O scripts/float_diff_gen.rs -o /tmp/float_diff_gen && /tmp/float_diff_gen [scale]
//
// `scale` (default 1) multiplies the number of random values (about 700 000 per unit); the structured families
// below do not change with it.
use std::io::{BufWriter, Write};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
}

fn main() {
    let scale: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(1).max(1);
    let stdout = std::io::stdout();
    let mut w = BufWriter::with_capacity(1 << 20, stdout.lock());
    let mut d = |bits: u64| {
        writeln!(w, "d {:016x} {}", bits, f64::from_bits(bits)).unwrap();
    };
    // ---- f64
    // specials: both zeros, both infinities, NaNs of several payloads and signs, the extremes
    for bits in [
        0u64, 1 << 63, 0x7ff0_0000_0000_0000, 0xfff0_0000_0000_0000, 0x7ff8_0000_0000_0000, 0xfff8_0000_0000_0000,
        0x7ff0_0000_0000_0001, 0x7fff_ffff_ffff_ffff, 0xffff_ffff_ffff_ffff, 1, 2, 3, 0x000f_ffff_ffff_ffff,
        0x0010_0000_0000_0000, 0x7fef_ffff_ffff_ffff, 0xffef_ffff_ffff_ffff,
    ] {
        d(bits);
    }
    // every exponent field with a spread of mantissas, both signs: powers of two (frac = 0) are the asymmetric case
    let fracs: [u64; 14] = [
        0, 1, 2, 3, 0xf_ffff_ffff_ffff, 0xf_ffff_ffff_fffe, 0x8_0000_0000_0000, 0x7_ffff_ffff_ffff,
        0x4_0000_0000_0000, 0x2_aaaa_aaaa_aaaa, 0x5_5555_5555_5555, 0x1_0000_0000, 0xffff_ffff, 0x1_0000_0001,
    ];
    for e in 0u64..2047 {
        for &f in &fracs {
            d((e << 52) | f);
        }
        d((1 << 63) | (e << 52));
        d((1 << 63) | (e << 52) | 1);
    }
    // decimal powers and their neighbours (the shortest digits are 1 or 5 there), with the digits of 2^53
    for k in -330i32..=310 {
        for m in ["1", "5", "9.999999999999999", "2.2250738585072014", "1.7976931348623157", "4.9406564584124654"] {
            let s = format!("{m}e{k}");
            let v: f64 = s.parse().unwrap();
            if !v.is_finite() {
                continue;
            }
            let b = v.to_bits();
            for dlt in [0i64, 1, -1, 2, -2, 3] {
                d((b as i64 + dlt) as u64 & 0x7fff_ffff_ffff_ffff);
            }
        }
    }
    // integers, near 2^53 and 2^63 and 2^64, and their halves
    for i in 0..=200_000u64 {
        d((i as f64).to_bits());
    }
    for base in [1u64 << 53, 1 << 54, 1 << 62, 1 << 63] {
        for dlt in 0..300u64 {
            d(((base + dlt) as f64).to_bits());
            d(((base - dlt) as f64).to_bits());
        }
    }
    d((u64::MAX as f64).to_bits());
    d((i64::MIN as f64).to_bits());
    d((i64::MAX as f64).to_bits());
    // short decimals: a / 10^k and the parsed text, which have short shortest forms
    for a in 0..3000u64 {
        for k in [1u32, 2, 3, 4, 5, 8, 10, 15, 17, 20, 30, 100, 300] {
            d(((a as f64) / 10f64.powi(k as i32)).to_bits());
            let s = format!("{a}e-{k}");
            d(s.parse::<f64>().unwrap().to_bits());
        }
    }
    for a in 1..=400u64 {
        for b in 1..=60u64 {
            d(((a as f64) / (b as f64)).to_bits());
        }
    }
    // random bit patterns (mostly extreme exponents), and random mantissas at ordinary exponents
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..(400_000 * scale) {
        d(r.next());
    }
    for _ in 0..(150_000 * scale) {
        let e = 1023 - 70 + (r.next() % 140);
        d((r.next() & (1 << 63)) | (e << 52) | (r.next() & 0xf_ffff_ffff_ffff));
    }

    // ---- f32
    let mut f = |bits: u32| {
        writeln!(w, "f {:08x} {}", bits, f32::from_bits(bits)).unwrap();
    };
    for bits in [
        0u32, 1 << 31, 0x7f80_0000, 0xff80_0000, 0x7fc0_0000, 0xffc0_0000, 0x7f80_0001, 0x7fff_ffff, 0xffff_ffff, 1, 2,
        3, 0x007f_ffff, 0x0080_0000, 0x7f7f_ffff, 0xff7f_ffff,
    ] {
        f(bits);
    }
    let ffracs: [u32; 12] = [0, 1, 2, 3, 0x7f_ffff, 0x7f_fffe, 0x40_0000, 0x3f_ffff, 0x20_0000, 0x2a_aaaa, 0x55_5555, 0x1_0001];
    for e in 0u32..255 {
        for &fr in &ffracs {
            f((e << 23) | fr);
        }
        f((1 << 31) | (e << 23));
    }
    for k in -50i32..=40 {
        for m in ["1", "5", "9.999999", "1.1754944", "3.4028235", "1.4012985"] {
            let s = format!("{m}e{k}");
            let v: f32 = s.parse().unwrap();
            if !v.is_finite() {
                continue;
            }
            let b = v.to_bits();
            for dlt in [0i64, 1, -1, 2, -2, 3] {
                f((b as i64 + dlt) as u32 & 0x7fff_ffff);
            }
        }
    }
    for i in 0..=100_000u32 {
        f((i as f32).to_bits());
    }
    for base in [1u32 << 24, 1 << 25, 1 << 30, 1 << 31] {
        for dlt in 0..200u32 {
            f(((base + dlt) as f32).to_bits());
            f(((base - dlt) as f32).to_bits());
        }
    }
    for a in 0..3000u32 {
        for k in [1u32, 2, 3, 4, 5, 8, 10, 20, 30, 38] {
            f(((a as f32) / 10f32.powi(k as i32)).to_bits());
            let s = format!("{a}e-{k}");
            f(s.parse::<f32>().unwrap().to_bits());
        }
    }
    for _ in 0..(400_000 * scale) {
        f(r.next() as u32);
    }
    for _ in 0..(150_000 * scale) {
        let e = 127 - 30 + (r.next() % 60) as u32;
        f(((r.next() as u32) & 0x8000_0000) | (e << 23) | ((r.next() as u32) & 0x7f_ffff));
    }
    w.flush().unwrap();
}
