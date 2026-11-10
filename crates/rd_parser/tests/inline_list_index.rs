//! `InlineList<T>` indexing at run time.
//!
//! Sema has accepted `l[i]` and `l[i] = v` on an `InlineList` since S5d (it
//! satisfies `Index` and `IndexSet` natively), but the interpreter had no arm
//! for it and panicked with "cannot index InlineList with int". The index is
//! checked against the items present, not the capacity, and a negative index
//! counts from the end, as for a `List`.
//!
//! See docs/DATASTRUCTURES.md, section 5.

use ubel_stratum::ast::arena::AstArena;
use ubel_stratum::interpreter::Interpreter;
use ubel_stratum_rd::{check_source, Stage};

fn messages(source: &str) -> Vec<String> {
    check_source(source).diagnostics.iter().map(|d| d.message.clone()).collect()
}

fn run(source: &str) -> Result<String, String> {
    assert!(matches!(check_source(source).stage, Stage::Clean), "program should check clean: {:?}\n{source}", messages(source));
    let tokens = ubel_stratum::lexer::tokenize(source).expect("source should lex");
    let arena = AstArena::new();
    let program = ubel_stratum_rd::parse(&arena, &tokens, source.to_string())
        .unwrap_or_else(|_| panic!("source should parse"));
    let ctx = ubel_stratum::sema::analyse(&program, &arena, source.to_string())
        .unwrap_or_else(|_| panic!("source should pass sema"));

    ubel_stratum::builtins::global::io::start_output_capture();
    let mut interp = Interpreter::new(&arena);
    interp.set_int_literal_types(ctx.int_literal_types);
    interp.set_trait_call_sites(ctx.trait_call_sites);
    let result = interp.run_program(&program);
    let output = ubel_stratum::builtins::global::io::take_captured_output();
    result.map(|_| output)
}

fn out(source: &str) -> String {
    run(source).unwrap_or_else(|e| panic!("run failed: {e}\n{source}"))
}

fn main_with(body: &str) -> String {
    format!(
        "fn main() void {{\n    let l: InlineList<int> = InlineList.new(4)\n    l.push(10)\n    l.push(20)\n{body}\n}}\n"
    )
}

#[test]
fn reads_by_index() {
    assert_eq!(out(&main_with("    println(l[0])\n    println(l[1])")), "10\n20\n");
}

#[test]
fn a_negative_index_counts_from_the_end() {
    assert_eq!(out(&main_with("    println(l[-1])\n    println(l[-2])")), "20\n10\n");
}

#[test]
fn assigns_by_index() {
    assert_eq!(out(&main_with("    l[0] = 11\n    println(l[0] + l[1])")), "31\n");
}

#[test]
fn assigns_by_a_negative_index() {
    assert_eq!(out(&main_with("    l[-1] = 99\n    println(l[1])")), "99\n");
}

#[test]
fn a_compound_assignment_on_an_element() {
    assert_eq!(out(&main_with("    l[1] += 5\n    println(l[1])")), "25\n");
}

#[test]
fn an_element_read_inside_a_loop() {
    let body = "    let mut t = 0\n    let mut i = 0\n    while i < l.len() {\n        t += l[i]\n        i += 1\n    }\n    println(t)";
    assert_eq!(out(&main_with(body)), "30\n");
}

#[test]
fn reading_past_the_items_panics_with_the_length() {
    // Capacity is 4 but only two items are present.
    let e = run(&main_with("    println(l[3])")).unwrap_err();
    assert!(e.contains("out of bounds") && e.contains("len 2"), "{e}");
}

#[test]
fn a_negative_index_before_the_first_item_panics() {
    let e = run(&main_with("    println(l[-3])")).unwrap_err();
    assert!(e.contains("out of bounds"), "{e}");
}

#[test]
fn assigning_past_the_items_panics() {
    let e = run(&main_with("    l[2] = 1")).unwrap_err();
    assert!(e.contains("out of bounds") && e.contains("len 2"), "{e}");
}

#[test]
fn an_element_of_a_string_inline_list() {
    let src = "fn main() void {\n    let l: InlineList<string> = InlineList.new(2)\n    l.push(\"a\")\n    l.push(\"b\")\n    l[0] = \"c\"\n    println(l[0] + l[1])\n}\n";
    assert_eq!(out(src), "cb\n");
}

#[test]
fn index_through_a_bound_reads_the_element() {
    // A generic function bounded by `Index` calls `index`, which reaches the
    // run-time index read through the trait method rather than `c[i]`.
    let src = "fn second<C: Index<int, int>>(c: C) int { return c.index(1) }\nfn main() void {\n    let l: InlineList<int> = InlineList.new(4)\n    l.push(10)\n    l.push(20)\n    println(second(l))\n}\n";
    assert_eq!(out(src), "20\n");
}

#[test]
fn index_set_through_a_bound_writes_the_element() {
    let src = "fn put<C: IndexSet<int, int>>(c: C) void { c.index_set(0, 5) }\nfn main() void {\n    let l: InlineList<int> = InlineList.new(4)\n    l.push(10)\n    put(l)\n    println(l[0])\n}\n";
    assert_eq!(out(src), "5\n");
}

#[test]
fn index_set_through_a_bound_past_the_items_panics() {
    let src = "fn put<C: IndexSet<int, int>>(c: C) void { c.index_set(3, 5) }\nfn main() void {\n    let l: InlineList<int> = InlineList.new(4)\n    l.push(10)\n    put(l)\n}\n";
    let e = run(src).unwrap_err();
    assert!(e.contains("out of bounds") && e.contains("len 1"), "{e}");
}
