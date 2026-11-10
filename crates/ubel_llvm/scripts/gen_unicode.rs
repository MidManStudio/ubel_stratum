// Generates the Unicode tables the C runtime uses for `to_upper`, `to_lower` and `trim` (A9c, provisional), and
// a differential test program for them. The interpreter implements these methods with Rust's own
// `str::to_uppercase`, `str::to_lowercase` and `str::trim`, so the tables are taken from the Rust standard
// library of the toolchain that runs this program, and the test program is checked against the interpreter,
// which makes Rust the oracle. Regenerate when that toolchain's Unicode version changes (the version is
// printed in the header).
//
//   rustc --edition 2021 -O crates/ubel_llvm/scripts/gen_unicode.rs -o /tmp/gen_unicode
//   /tmp/gen_unicode crates/ubel_llvm/runtime/ubel_unicode.h crates/ubel_llvm/examples/29_string_unicode.ubl
//
// `Cased` and `Case_Ignorable`, which the final-sigma rule of `to_lowercase` needs, are not public on stable
// Rust, so they are measured through `to_lowercase` itself. The rule skips Case_Ignorable characters and then
// asks whether the next one is Cased, so a character that is both counts as ignorable, never as cased:
//   P1(c) = ("c" + "S").to_lowercase() ends in final sigma          <=> c is Cased and not Case_Ignorable
//   A(c)  = ("a" + c + "S").to_lowercase() ends in final sigma      <=> c is Cased or Case_Ignorable
// (a = Greek capital alpha, S = Greek capital sigma.) So Case_Ignorable(c) = A(c) && !P1(c), and the table
// written as "cased" holds P1: exactly the characters the rule can find after skipping the ignorable ones.
use std::fmt::Write as _;

const SIGMA: char = '\u{3A3}';
const ALPHA: char = '\u{391}';

fn lower_ends_final(s: &str) -> bool {
    s.to_lowercase().ends_with('\u{3C2}')
}

fn ranges(set: &[bool]) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    let mut i = 0usize;
    while i < set.len() {
        if set[i] {
            let lo = i;
            while i + 1 < set.len() && set[i + 1] {
                i += 1;
            }
            out.push((lo as u32, i as u32));
        }
        i += 1;
    }
    out
}

fn emit_ranges(out: &mut String, name: &str, r: &[(u32, u32)]) {
    writeln!(out, "static const uint32_t {name}[][2] = {{").unwrap();
    let mut col = 0;
    for (lo, hi) in r {
        let item = format!("{{0x{lo:X},0x{hi:X}}},");
        if col + item.len() > 110 {
            out.push('\n');
            col = 0;
        }
        out.push_str(&item);
        col += item.len();
    }
    writeln!(out, "\n}};").unwrap();
    writeln!(out, "#define {}_N (sizeof {name} / sizeof {name}[0])\n", name.to_uppercase()).unwrap();
}

/// (code point, mapped value): a single code point as is, or 0x80000000 | len << 24 | pool offset.
fn emit_case(out: &mut String, name: &str, entries: &[(u32, Vec<u32>)], pool: &mut Vec<u32>) {
    let mut vals: Vec<u32> = Vec::new();
    for (_, m) in entries {
        if m.len() == 1 {
            vals.push(m[0]);
        } else {
            assert!(m.len() == 2 || m.len() == 3);
            let off = pool.len() as u32;
            assert!(off < (1 << 24));
            pool.extend(m.iter());
            vals.push(0x8000_0000 | ((m.len() as u32) << 24) | off);
        }
    }
    for (suffix, items) in [("cp", entries.iter().map(|e| e.0).collect::<Vec<_>>()), ("val", vals)] {
        writeln!(out, "static const uint32_t {name}_{suffix}[] = {{").unwrap();
        let mut col = 0;
        for v in items {
            let item = format!("0x{v:X},");
            if col + item.len() > 110 {
                out.push('\n');
                col = 0;
            }
            out.push_str(&item);
            col += item.len();
        }
        writeln!(out, "\n}};").unwrap();
    }
    writeln!(out, "#define {}_N (sizeof {name}_cp / sizeof {name}_cp[0])\n", name.to_uppercase()).unwrap();
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn lit(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            '\r' => o.push_str("\\r"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 2 && args[1] == "--version" {
        // The Unicode version of this toolchain's standard library; scripts/run_equiv.sh compares it with the
        // version recorded in runtime/ubel_unicode.h and regenerates when they differ.
        let (maj, min, pat) = std::char::UNICODE_VERSION;
        println!("{maj}.{min}.{pat}");
        return;
    }
    if args.len() != 3 {
        eprintln!("usage: gen_unicode <header.h> <example.ubl>   |   gen_unicode --version");
        std::process::exit(2);
    }
    let n = 0x110000usize;
    let mut ws = vec![false; n];
    let mut cased = vec![false; n];
    let mut ign = vec![false; n];
    let mut upper: Vec<(u32, Vec<u32>)> = Vec::new();
    let mut lower: Vec<(u32, Vec<u32>)> = Vec::new();
    for cp in 0..n as u32 {
        let c = match char::from_u32(cp) {
            Some(c) => c,
            None => continue,
        };
        ws[cp as usize] = c.is_whitespace();
        let up: Vec<u32> = c.to_uppercase().map(|x| x as u32).collect();
        if up != [cp] {
            upper.push((cp, up));
        }
        let lo: Vec<u32> = c.to_lowercase().map(|x| x as u32).collect();
        if lo != [cp] {
            lower.push((cp, lo));
        }
        let mut s = String::new();
        s.push(c);
        s.push(SIGMA);
        let is_cased = lower_ends_final(&s);
        let mut a = String::new();
        a.push(ALPHA);
        a.push(c);
        a.push(SIGMA);
        let a_final = lower_ends_final(&a);
        cased[cp as usize] = is_cased;
        ign[cp as usize] = a_final && !is_cased;
        // Every Lowercase and Uppercase letter is Cased, so it is either found by P1 or skipped as ignorable:
        // a cheap check on the probe.
        if c.is_lowercase() || c.is_uppercase() {
            assert!(is_cased || ign[cp as usize], "probe finds U+{cp:X} neither cased nor ignorable although it is upper or lower case");
        }
    }
    let (wsr, casedr, ignr) = (ranges(&ws), ranges(&cased), ranges(&ign));

    // ---- the header
    let mut h = String::new();
    let (maj, min, pat) = std::char::UNICODE_VERSION;
    writeln!(h, "/* GENERATED by crates/ubel_llvm/scripts/gen_unicode.rs. Do not edit; regenerate (see that file). */").unwrap();
    writeln!(h, "/* Unicode {maj}.{min}.{pat}, from the Rust standard library the interpreter is built with: `to_upper`,").unwrap();
    writeln!(h, " * `to_lower` and `trim` are Rust's str::to_uppercase, to_lowercase and trim. */").unwrap();
    writeln!(h, "#ifndef UBEL_UNICODE_H\n#define UBEL_UNICODE_H\n#include <stdint.h>\n#include <stddef.h>\n").unwrap();
    writeln!(h, "#define UBEL_UNICODE_VERSION \"{maj}.{min}.{pat}\"\n").unwrap();
    writeln!(h, "/* White_Space: what str::trim removes. */").unwrap();
    emit_ranges(&mut h, "ubel_ws", &wsr);
    writeln!(h, "/* The final-sigma rule of to_lower skips Case_Ignorable characters (ubel_ign) and then asks whether the next\n * one is cased (ubel_cased: the Cased characters that are not Case_Ignorable). */").unwrap();
    emit_ranges(&mut h, "ubel_cased", &casedr);
    emit_ranges(&mut h, "ubel_ign", &ignr);
    let mut pool: Vec<u32> = Vec::new();
    writeln!(h, "/* Full case mappings that differ from the identity, sorted by code point. A value with bit 31 set is\n * 0x80000000 | length << 24 | offset into ubel_case_pool (2 or 3 code points); otherwise it is the code point. */").unwrap();
    emit_case(&mut h, "ubel_upper", &upper, &mut pool);
    emit_case(&mut h, "ubel_lower", &lower, &mut pool);
    writeln!(h, "static const uint32_t ubel_case_pool[] = {{").unwrap();
    for (i, v) in pool.iter().enumerate() {
        write!(h, "0x{v:X},{}", if i % 12 == 11 { "\n" } else { "" }).unwrap();
    }
    writeln!(h, "\n}};\n\n#endif").unwrap();
    std::fs::write(&args[1], h).unwrap();

    // ---- the differential test program
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let mut t = String::new();
    writeln!(t, "// GENERATED by crates/ubel_llvm/scripts/gen_unicode.rs (Unicode {maj}.{min}.{pat}). Do not edit; regenerate.").unwrap();
    writeln!(t, "// String methods that build a new string (A9c, provisional): `to_upper`, `to_lower`, `trim`, `trim_start`,").unwrap();
    writeln!(t, "// `trim_end` and `replace`, checked against the interpreter, which uses Rust's own Unicode tables. Sections:").unwrap();
    writeln!(t, "// every code point whose upper or lower case mapping is not itself (so every table entry), the whitespace").unwrap();
    writeln!(t, "// set and its look-alikes, the final-sigma rule around the edges of every Cased and Case_Ignorable range,").unwrap();
    writeln!(t, "// random strings from a pool of awkward characters, and random `replace` calls. 30_string_build holds the").unwrap();
    writeln!(t, "// hand-written cases (tiers, arenas, long strings, the collector).").unwrap();
    writeln!(t, "fn show(s: string) void {{").unwrap();
    writeln!(t, "    println(\"[\" + s.to_upper() + \"]\", \"[\" + s.to_lower() + \"]\", \"[\" + s.trim() + \"]\", \"[\" + s.trim_start() + \"]\", \"[\" + s.trim_end() + \"]\")").unwrap();
    writeln!(t, "}}\n").unwrap();
    writeln!(t, "fn rep(h: string, f: string, r: string) void {{").unwrap();
    writeln!(t, "    println(\"[\" + h.replace(f, r) + \"]\")").unwrap();
    writeln!(t, "}}\n").unwrap();
    writeln!(t, "fn main() void {{").unwrap();

    let mut lines = 0usize;
    let mut put = |t: &mut String, s: &str| {
        writeln!(t, "    show({})", lit(s)).unwrap();
        lines += 1;
    };
    // 1. every code point with a non-identity upper or lower mapping, 40 to a line
    let mut mapped: Vec<u32> = upper.iter().map(|e| e.0).chain(lower.iter().map(|e| e.0)).collect();
    mapped.sort();
    mapped.dedup();
    for chunk in mapped.chunks(40) {
        let s: String = chunk.iter().map(|&c| char::from_u32(c).unwrap()).collect();
        put(&mut t, &s);
    }
    // 2. whitespace, and look-alikes that are not whitespace
    let wsc: Vec<char> = (0..n as u32).filter(|&c| ws[c as usize]).map(|c| char::from_u32(c).unwrap()).collect();
    for &w in &wsc {
        put(&mut t, &format!("{w}x{w}"));
        put(&mut t, &format!("{w}{w} x y {w}"));
    }
    let all_ws: String = wsc.iter().collect();
    put(&mut t, &format!("{all_ws}x{all_ws}"));
    put(&mut t, &format!("{all_ws}{all_ws}"));
    for &l in &['\u{200B}', '\u{FEFF}', '\u{180E}', '\u{2060}', '\u{AD}', '\u{200C}', '\u{200D}', '\u{2800}', '\u{3164}', '\u{FFA0}', '\u{115F}'] {
        put(&mut t, &format!("{l}x{l}"));
        put(&mut t, &format!(" {l} x {l} "));
    }
    // 3. final sigma around every range edge of Cased and Case_Ignorable, 32 per line, both contexts
    let mut edges: Vec<u32> = Vec::new();
    for set in [&casedr, &ignr] {
        for &(lo, hi) in set.iter() {
            for e in [lo.wrapping_sub(1), lo, hi, hi + 1] {
                if e < n as u32 && char::from_u32(e).is_some() {
                    edges.push(e);
                }
            }
        }
    }
    edges.sort();
    edges.dedup();
    for chunk in edges.chunks(32) {
        let (mut before, mut after) = (String::new(), String::new());
        for &e in chunk {
            let c = char::from_u32(e).unwrap();
            before.push(ALPHA);
            before.push(c);
            before.push(SIGMA);
            before.push(' ');
            after.push(ALPHA);
            after.push(SIGMA);
            after.push(c);
            after.push(ALPHA);
            after.push(' ');
        }
        put(&mut t, &before);
        put(&mut t, &after);
    }
    // 4. random strings from a pool of awkward characters
    let pool_chars: Vec<char> = "aAzZ09 .,:'-_\t\n\u{B}\u{C}\r\u{A0}\u{2003}\u{3000}\u{85}\u{2028}\u{1680}\u{202F}\u{205F}\u{180E}\u{200B}\u{FEFF}\
        \u{DF}\u{E9}\u{C9}\u{FF}\u{B5}\u{AA}\u{130}\u{131}\u{1C5}\u{1C8}\u{1CB}\u{1F2}\u{149}\u{17F}\u{212A}\u{212B}\u{1E9E}\
        \u{3A3}\u{3C3}\u{3C2}\u{391}\u{3B1}\u{390}\u{3B0}\u{1F80}\u{1FBC}\u{1FFC}\u{345}\u{414}\u{434}\u{587}\u{10D0}\u{1C90}\
        \u{13A0}\u{AB70}\u{10400}\u{10428}\u{FB00}\u{FB01}\u{FB03}\u{FB05}\u{300}\u{307}\u{301}\u{2B0}\u{2B7}\u{2019}\u{B7}\
        \u{65E5}\u{672C}\u{1F600}"
        .chars()
        .collect();
    for _ in 0..260 {
        let len = 1 + rng.below(10);
        let s: String = (0..len).map(|_| pool_chars[rng.below(pool_chars.len())]).collect();
        put(&mut t, &s);
    }
    // 5. replace
    let alpha: [&str; 12] = ["a", "b", "ab", "aa", "\u{E9}", "e\u{301}", "\u{65E5}", "\u{DF}", " ", "x", "aaa", "ba"];
    for _ in 0..200 {
        let hl = rng.below(9);
        let hay: String = (0..hl).map(|_| alpha[rng.below(alpha.len())]).collect();
        let from = if rng.below(8) == 0 { String::new() } else { alpha[rng.below(alpha.len())].to_string() };
        let to = if rng.below(6) == 0 { String::new() } else { alpha[rng.below(alpha.len())].to_string() };
        writeln!(t, "    rep({}, {}, {})", lit(&hay), lit(&from), lit(&to)).unwrap();
    }
    writeln!(t, "}}").unwrap();
    std::fs::write(&args[2], t).unwrap();

    eprintln!(
        "Unicode {maj}.{min}.{pat}: white_space {} ranges, cased {} ranges, case_ignorable {} ranges, upper {} entries, lower {} entries, pool {} code points, {} show lines",
        wsr.len(), casedr.len(), ignr.len(), upper.len(), lower.len(), pool.len(), lines
    );
}
