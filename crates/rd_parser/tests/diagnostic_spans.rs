//! Where a diagnostic points.
//!
//! A type mismatch found while comparing the ARGUMENTS of two generic
//! types (`List<int>` against `List<string>`, a function's parameter
//! types, a dictionary's key and value) used to carry `Span::at(0)`, the
//! top of the file, because the structural comparison had no span of its
//! own to report with. Every nested comparison now reports at the span of
//! the outer one. A test that only counts diagnostics cannot see this, so
//! these assert the line.

use ubel_stratum_rd::check_source;

/// The 1-based line and column of each diagnostic's primary span.
fn lines(source: &str) -> Vec<(usize, usize)> {
    check_source(source).diagnostics.iter()
        .map(|d| (d.primary_span.line as usize, d.primary_span.column as usize))
        .collect()
}

#[test]
fn a_generic_argument_mismatch_in_a_call_points_at_the_offending_argument() {
    // Line 5, column 14 is the `y` in `first(x, y)`.
    let src = "fn first(a: List<int>, b: List<string>) void { }\n\
               fn main() void {\n    let x: List<int> = List.new()\n    let y: List<int> = List.new()\n    first(x, y)\n}\n";
    assert_eq!(lines(src), vec![(5, 14)]);
}

#[test]
fn a_dictionary_argument_mismatch_points_at_the_statement_not_the_file_start() {
    let src = "fn main() void {\n    let m: Dictionary<string, int> = Dictionary.new()\n    let k: Dictionary<int, int> = m\n}\n";
    let found = lines(src);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, 3, "should point at line 3, got {found:?}");
}

#[test]
fn no_diagnostic_points_at_offset_zero() {
    // The three shapes that used to: a generic instance, a function
    // type's parameter, and a dictionary's key.
    let src = "fn first(a: List<int>, b: List<string>) void { }\n\
               fn takes(h: fn(int) int) void { }\n\
               fn main() void {\n    let x: List<int> = List.new()\n    first(x, x)\n\
                   takes(fn(s: string) s)\n    let m: Dictionary<string, int> = Dictionary.new()\n\
                   let k: Dictionary<int, int> = m\n}\n";
    let found = lines(src);
    assert!(found.len() >= 3, "expected at least three diagnostics, got {found:?}");
    assert!(found.iter().all(|&(line, _)| line >= 4), "a diagnostic points at the file start: {found:?}");
}
