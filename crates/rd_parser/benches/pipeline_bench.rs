// Benchmark suite: tokenizer, rd_parser, and sema, on both synthetic
// scaling inputs and every real fixture file.
//
// Run with: cargo bench -p ubel_stratum_rd
// HTML report (if the html_reports feature is active): target/criterion/report/index.html

use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput,
};
use std::fs;
use std::path::Path;
use std::time::Duration;
use ubel_stratum::ast::arena::AstArena;

/// Generate a synthetic .ubl source with `n` small functions, so we can see
/// how each stage scales with input size rather than only measuring one
/// fixed-size sample.
fn synthetic_source(n_fns: usize) -> String {
    let mut s = String::with_capacity(n_fns * 64);
    for i in 0..n_fns {
        s.push_str(&format!(
            "fn f{i}(a int, b int) int {{\n    let x = a + b\n    return x\n}}\n\n"
        ));
    }
    s
}

fn bench_lex_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("lex_scaling");
    group.measurement_time(Duration::from_secs(5));
    for n in [10usize, 100, 500] {
        let source = synthetic_source(n);
        group.throughput(Throughput::Bytes(source.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &source, |b, source| {
            b.iter(|| ubel_stratum::lexer::tokenize(black_box(source)));
        });
    }
    group.finish();
}

fn bench_parse_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse_scaling");
    group.measurement_time(Duration::from_secs(5));
    for n in [10usize, 100, 500] {
        let source = synthetic_source(n);
        let tokens = ubel_stratum::lexer::tokenize(&source).expect("synthetic source must lex");
        group.throughput(Throughput::Bytes(source.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(n),
            &(tokens, source),
            |b, (tokens, source)| {
                b.iter(|| {
                    let arena = AstArena::new();
                    let _ = ubel_stratum_rd::parse(&arena, black_box(tokens), source.clone());
                });
            },
        );
    }
    group.finish();
}

fn bench_sema_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("sema_scaling");
    for n in [10usize, 100, 500] {
        let source = synthetic_source(n);
        let tokens = ubel_stratum::lexer::tokenize(&source).expect("synthetic source must lex");
        group.bench_with_input(
            BenchmarkId::from_parameter(n),
            &(tokens, source),
            |b, (tokens, source)| {
                b.iter(|| {
                    let arena = AstArena::new();
                    let program = ubel_stratum_rd::parse(&arena, tokens, source.clone())
                        .expect("synthetic source must parse");
                    let _ = ubel_stratum::sema::analyse(black_box(&program), &arena, source.clone());
                });
            },
        );
    }
    group.finish();
}

/// Full lex+parse pipeline against every real .ubl fixture, so regressions
/// on real (small, hand-written) programs are visible alongside the
/// synthetic scaling curves above.
fn bench_fixtures(c: &mut Criterion) {
    let mut group = c.benchmark_group("fixtures_lex_parse");
    // One benchmark per fixture, so the cost of this group is the fixture
    // count times the cost of one benchmark. Criterion's defaults (3 s
    // warm-up, 5 s measurement, 100 samples, 100,000 resamples) cost 8 s or
    // more per fixture; at 245 fixtures that is over half an hour, and the
    // Deploy Site job (30 minute limit) was cancelled on every push. One
    // fixture is a fraction of a millisecond of work (4 to 241 us measured),
    // so a short window gives a usable estimate and the group takes about
    // four minutes.
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(100));
    group.measurement_time(Duration::from_millis(300));
    group.nresamples(10_000);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let Ok(entries) = fs::read_dir(&dir) else {
        eprintln!("skipping fixture benches: {} not found", dir.display());
        return;
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "ubl").unwrap_or(false))
        .collect();
    paths.sort();

    for path in paths {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let source = fs::read_to_string(&path).unwrap();
        group.bench_function(&name, |b| {
            b.iter(|| {
                if let Ok(tokens) = ubel_stratum::lexer::tokenize(black_box(&source)) {
                    let arena = AstArena::new();
                    let _ = ubel_stratum_rd::parse(&arena, &tokens, source.clone());
                }
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_lex_scaling,
    bench_parse_scaling,
    bench_sema_scaling,
    bench_fixtures
);
criterion_main!(benches);
