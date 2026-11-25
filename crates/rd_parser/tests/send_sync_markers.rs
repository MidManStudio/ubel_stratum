//! `Send` and `Sync` (docs/ASYNC_SYNC_DESIGN.md, AS4a): two marker traits that a struct or an enum
//! takes with an `impl`, checked against its fields, and an `unsafe impl` that is trusted.
//!
//! The `.ubl` fixtures show whole programs accepted or rejected. These tests pin what they
//! cannot see: the exact code, count and wording of each diagnostic, which built-in types have
//! the markers and which do not, how a bound sees through generics, and that the marker rules
//! leave the ordinary trait machinery alone.

use ubel_stratum::ast::arena::AstArena;
use ubel_stratum::interpreter::Interpreter;
use ubel_stratum_rd::{check_source, Stage};

fn codes(source: &str) -> Vec<&'static str> {
    check_source(source).diagnostics.iter().map(|d| d.code).collect()
}

fn messages(source: &str) -> Vec<String> {
    check_source(source).diagnostics.iter().map(|d| d.message.clone()).collect()
}

fn is_clean(source: &str) -> bool {
    matches!(check_source(source).stage, Stage::Clean)
}

/// Helper functions that ask for a marker through a bound, so a program can test what a type has
/// without a task. Every program below ends with `fn main`.
const BOUNDS: &str = "fn needs_send<T: Send>(x: T) void {\n}\n\nfn needs_sync<T: Sync>(x: T) void {\n}\n\n";
const MAIN: &str = "\nfn main() void {\n}\n";

fn program(body: &str) -> String {
    format!("{BOUNDS}{body}{MAIN}")
}

// ── What an impl takes ───────────────────────────────────────────

#[test]
fn a_struct_with_sendable_fields_may_say_it_is_send_and_sync() {
    let src = program(
        "struct Job { id: int, name: string, tags: List<string>, pos: (int, int), note: string?, table: Dictionary<string, int> }\n\n\
         impl Send for Job {\n}\n\nimpl Sync for Job {\n}\n");
    assert_eq!(codes(&src), Vec::<&str>::new());
}

#[test]
fn a_struct_that_holds_another_sendable_struct_is_checked_through_its_impl() {
    let src = program(
        "struct Inner { n: int }\n\nimpl Send for Inner {\n}\n\n\
         struct Outer { inner: Inner, more: List<Inner> }\n\nimpl Send for Outer {\n}\n");
    assert_eq!(codes(&src), Vec::<&str>::new());
}

#[test]
fn an_enum_is_checked_variant_by_variant() {
    let src = program(
        "enum Msg {\n    Quit,\n    Move(int, int),\n    Say { text: string },\n}\n\nimpl Send for Msg {\n}\n");
    assert_eq!(codes(&src), Vec::<&str>::new());
}

#[test]
fn a_sync_shared_field_is_send_and_sync_when_its_contents_are_send() {
    let src = program(
        "struct Counter { total: SyncShared<int> }\n\nimpl Send for Counter {\n}\n\nimpl Sync for Counter {\n}\n");
    assert_eq!(codes(&src), Vec::<&str>::new());
}

#[test]
fn a_unique_field_has_what_its_contents_have() {
    let src = program(
        "struct Inner { n: int }\n\nimpl Send for Inner {\n}\n\n\
         struct Owner { inner: Unique<Inner> }\n\nimpl Send for Owner {\n}\n");
    assert_eq!(codes(&src), Vec::<&str>::new());
}

// ── What an impl is refused ──────────────────────────────────────

#[test]
fn a_shared_field_makes_an_impl_of_send_an_error_that_names_the_field() {
    let src = program("struct Holder { cell: Shared<int> }\n\nimpl Send for Holder {\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-145"]);
    assert_eq!(
        messages(&src),
        vec!["`impl Send for Holder`: field `cell` has type `Shared<int>`, which is not `Send`"]
    );
}

#[test]
fn the_same_field_is_refused_for_sync_with_the_sync_wording() {
    let src = program("struct Holder { cell: Shared<int> }\n\nimpl Sync for Holder {\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-145"]);
    assert!(messages(&src)[0].starts_with("`impl Sync for Holder`: field `cell`"), "{:?}", messages(&src));
    assert!(messages(&src)[0].ends_with("which is not `Sync`"), "{:?}", messages(&src));
}

#[test]
fn a_field_whose_own_type_has_no_impl_is_refused() {
    let src = program(
        "struct Inner { n: int }\n\nstruct Outer { inner: Inner }\n\nimpl Send for Outer {\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-145"]);
    assert!(messages(&src)[0].contains("field `inner` has type `Inner`"), "{:?}", messages(&src));
}

#[test]
fn a_part_inside_a_collection_is_found() {
    let src = program(
        "struct Pool2 { cells: List<Shared<int>> }\n\nimpl Send for Pool2 {\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-145"]);
    assert!(messages(&src)[0].contains("field `cells` has type `List<Shared<int>>`"), "{:?}", messages(&src));
}

#[test]
fn a_function_value_and_a_task_are_not_send() {
    let src = program(
        "struct Callback { run: fn(int) int }\n\nimpl Send for Callback {\n}\n\n\
         struct Pending { t: Task<int> }\n\nimpl Send for Pending {\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-145", "TYPE-145"]);
}

#[test]
fn a_variant_payload_is_named_by_its_variant() {
    let src = program(
        "enum Slot {\n    Empty,\n    Full(Shared<int>),\n}\n\nimpl Send for Slot {\n}\n\n\
         enum Named {\n    A { cell: Shared<int> },\n}\n\nimpl Send for Named {\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-145", "TYPE-145"]);
    let m = messages(&src);
    assert!(m[0].contains("field 0 of variant `Full`"), "{m:?}");
    assert!(m[1].contains("field `cell` of variant `A`"), "{m:?}");
}

#[test]
fn a_sync_shared_of_something_not_send_is_not_send() {
    let src = program(
        "struct Bad { total: SyncShared<Shared<int>> }\n\nimpl Send for Bad {\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-145"]);
}

#[test]
fn only_the_first_part_that_fails_is_named() {
    let src = program(
        "struct Two { a: Shared<int>, b: Shared<string> }\n\nimpl Send for Two {\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-145"]);
    assert!(messages(&src)[0].contains("field `a`"), "{:?}", messages(&src));
}

// ── unsafe impl ──────────────────────────────────────────────────

#[test]
fn an_unsafe_impl_is_trusted_and_not_checked_against_the_fields() {
    let src = program(
        "struct Holder { cell: Shared<int> }\n\nunsafe impl Send for Holder {\n}\n\nunsafe impl Sync for Holder {\n}\n");
    assert_eq!(codes(&src), Vec::<&str>::new());
}

#[test]
fn an_unsafe_impl_of_one_marker_does_not_excuse_the_other() {
    let src = program(
        "struct Holder { cell: Shared<int> }\n\nunsafe impl Send for Holder {\n}\n\nimpl Sync for Holder {\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-145"]);
    assert!(messages(&src)[0].starts_with("`impl Sync for Holder`"), "{:?}", messages(&src));
}

#[test]
fn a_trusted_impl_makes_the_type_send_for_a_bound() {
    let src = program(
        "struct Holder { cell: Shared<int> }\n\nunsafe impl Send for Holder {\n}\n\n\
         fn go(h: Holder) void {\n    needs_send(h)\n}\n");
    assert_eq!(codes(&src), Vec::<&str>::new());
}

#[test]
fn unsafe_is_for_send_and_sync_only() {
    let src = program(
        "trait Show {\n    fn show(self) string\n}\n\nstruct A { n: int }\n\n\
         unsafe impl Show for A {\n    fn show(self) string { return \"a\" }\n}\n\n\
         unsafe impl A {\n    fn get(self) int { return self.n }\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-146", "TYPE-146"]);
    assert_eq!(
        messages(&src),
        vec![
            "`unsafe impl` is only for `Send` and `Sync`, found an impl of `Show`",
            "`unsafe impl` is only for `Send` and `Sync`, found an inherent impl",
        ]
    );
}

#[test]
fn an_unsafe_impl_may_carry_attributes() {
    let src = program(
        "struct Holder { cell: Shared<int> }\n\n@doc(\"trusted\")\nunsafe impl Send for Holder {\n}\n");
    assert_eq!(codes(&src), Vec::<&str>::new());
}

#[test]
fn unsafe_before_anything_but_an_impl_is_a_parse_error() {
    let report = check_source("unsafe fn f() void {\n}\n\nfn main() void {\n}\n");
    assert!(matches!(report.stage, Stage::Parse), "stage: {}", report.stage.as_str());
    assert!(report.diagnostics.iter().all(|d| d.code.starts_with("PARSE-")));
    // The error is at the token after `unsafe`, and says what was wanted there.
    let d = &report.diagnostics[0];
    assert!(d.message.contains("'impl' after 'unsafe'"), "{}", d.message);
    assert_eq!((d.primary_span.line, d.primary_span.column), (1, 8));
}

// ── Parts, one by one ────────────────────────────────────────────

#[test]
fn a_tuple_with_one_part_that_is_not_send_is_not_send() {
    let src = program(
        "struct Pair { both: (int, Shared<int>) }\n\nimpl Send for Pair {\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-145"]);
}

#[test]
fn a_dictionary_is_checked_through_its_values_as_well_as_its_keys() {
    let src = program(
        "struct Table { rows: Dictionary<string, Shared<int>> }\n\nimpl Send for Table {\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-145"]);
}

#[test]
fn an_optional_and_a_fallible_have_what_their_contents_have() {
    let src = program(
        "struct A { maybe: Shared<int>? }\n\nimpl Send for A {\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-145"]);
}

#[test]
fn a_sync_shared_is_sync_when_its_contents_are_only_send() {
    // `Only` says `Send` and nothing else. A lock is what makes sharing the `SyncShared` safe,
    // so the `SyncShared<Only>` is `Sync` anyway.
    let src = program(
        "struct Only { n: int }\n\nimpl Send for Only {\n}\n\n\
         fn go(s: SyncShared<Only>) void {\n    needs_send(s)\n    needs_sync(s)\n}\n");
    assert_eq!(codes(&src), Vec::<&str>::new());
}

#[test]
fn a_unique_has_sync_only_when_its_contents_do() {
    let src = program(
        "struct Only { n: int }\n\nimpl Send for Only {\n}\n\n\
         fn go(u: Unique<Only>) void {\n    needs_send(u)\n    needs_sync(u)\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-126"]);
    assert!(messages(&src)[0].contains("`Sync`"), "{:?}", messages(&src));
}

#[test]
fn an_overlapping_impl_that_registration_refused_is_not_checked_again() {
    let src = program(
        "struct Holder { cell: Shared<int> }\n\nimpl Send for Holder {\n}\n\nimpl Send for Holder {\n}\n");
    // One overlap, and one error for the first impl. The second is not read a second time.
    let mut got = codes(&src);
    got.sort();
    assert_eq!(got, vec!["TYPE-128", "TYPE-145"]);
}

#[test]
fn an_unsafe_impl_is_reported_where_the_unsafe_keyword_is() {
    let src = program("struct A { n: int }\n\nunsafe impl A {\n}\n");
    let report = check_source(&src);
    let d = &report.diagnostics[0];
    assert_eq!(d.code, "TYPE-146");
    // The line is found in the source, so the test does not depend on the helpers above the impl.
    let line_of_impl = src.lines().position(|l| l.starts_with("unsafe impl")).unwrap() + 1;
    assert_eq!(d.primary_span.line, line_of_impl);
    assert_eq!(d.primary_span.column, 1);
}

// ── Built-in types ───────────────────────────────────────────────

#[test]
fn no_impl_is_accepted_for_a_built_in_type() {
    let src = program(
        "impl Send for int {\n}\n\nimpl Sync for List<int> {\n}\n\nunsafe impl Send for string {\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-129", "TYPE-129", "TYPE-129"]);
}

#[test]
fn a_second_impl_of_a_marker_for_one_type_is_an_overlap() {
    let src = program("struct A { n: int }\n\nimpl Send for A {\n}\n\nimpl Send for A {\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-128"]);
}

// ── Bounds ───────────────────────────────────────────────────────

#[test]
fn scalars_strings_and_the_collections_of_them_satisfy_both_bounds() {
    let src = program(
        "fn go(l: List<int>, d: Dictionary<string, double>, t: (int, string), o: int?, q: Queue<char>, s: Set<bool>) void {\n\
         \x20   needs_send(1)\n    needs_send(\"x\")\n    needs_send(2.5)\n    needs_send(l)\n    needs_send(d)\n    needs_send(t)\n    needs_send(o)\n    needs_send(q)\n    needs_send(s)\n\
         \x20   needs_sync(1)\n    needs_sync(\"x\")\n    needs_sync(l)\n    needs_sync(d)\n    needs_sync(t)\n}\n");
    assert_eq!(codes(&src), Vec::<&str>::new());
}

#[test]
fn the_prelude_enum_needs_no_impl() {
    let src = program("fn go() void {\n    needs_send(Ordering.Less)\n    needs_sync(Ordering.Greater)\n}\n");
    assert_eq!(codes(&src), Vec::<&str>::new());
}

#[test]
fn shared_a_borrowed_view_a_task_and_a_function_fail_a_bound() {
    let src = program(
        "fn go(s: Shared<int>, t: Task<int>, f: fn(int) int) void {\n    needs_send(s)\n    needs_send(t)\n    needs_send(f)\n    needs_sync(s)\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-126", "TYPE-126", "TYPE-126", "TYPE-126"]);
}

#[test]
fn a_struct_without_an_impl_fails_a_bound_and_one_with_an_impl_passes() {
    let src = program(
        "struct Plain { n: int }\n\nstruct Marked { n: int }\n\nimpl Send for Marked {\n}\n\n\
         fn go(p: Plain, m: Marked) void {\n    needs_send(m)\n    needs_send(p)\n    needs_sync(m)\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-126", "TYPE-126"]);
}

#[test]
fn a_bound_looks_at_where_a_value_lives_and_does_not_peel_the_wrapper() {
    // `Shared<int>` holds an `int`, and an `int` is `Send`: the wrapper is what is not.
    let src = program("fn go(s: Shared<int>) void {\n    needs_send(s)\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-126"]);
    // The error names the wrapper, not the `int` inside it.
    assert_eq!(messages(&src), vec!["the trait `Send` is not implemented for `Shared<int>`"]);
}

#[test]
fn a_sync_shared_satisfies_both_bounds_when_its_contents_are_send() {
    let src = program(
        "fn go(a: SyncShared<int>, b: SyncShared<Shared<int>>) void {\n    needs_send(a)\n    needs_sync(a)\n    needs_send(b)\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-126"]);
}

#[test]
fn a_type_parameter_is_send_only_through_its_own_bound() {
    let ok = program("fn pass<T: Send>(x: T) void {\n    needs_send(x)\n}\n");
    assert_eq!(codes(&ok), Vec::<&str>::new());
    let missing = program("fn pass<T>(x: T) void {\n    needs_send(x)\n}\n");
    assert_eq!(codes(&missing), vec!["TYPE-126"]);
    let other = program("fn pass<T: Sync>(x: T) void {\n    needs_send(x)\n}\n");
    assert_eq!(codes(&other), vec!["TYPE-126"]);
}

// ── Generic impls ────────────────────────────────────────────────

#[test]
fn a_generic_impl_with_a_bound_is_conditional_on_it() {
    let src = program(
        "struct Box<T> { v: T }\n\nimpl<T: Send> Send for Box<T> {\n}\n\n\
         fn go(a: Box<int>, b: Box<Shared<int>>) void {\n    needs_send(a)\n    needs_send(b)\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-126"]);
}

#[test]
fn a_generic_impl_without_the_bound_is_refused_for_the_field_it_cannot_vouch_for() {
    let src = program("struct Box<T> { v: T }\n\nimpl<T> Send for Box<T> {\n}\n");
    assert_eq!(codes(&src), vec!["TYPE-145"]);
    assert!(messages(&src)[0].contains("field `v` has type `T`"), "{:?}", messages(&src));
}

// ── The rest of the trait machinery is untouched ─────────────────

#[test]
fn a_program_that_declares_its_own_send_keeps_it_as_an_ordinary_trait() {
    let src = "trait Send {\n    fn go(self) int\n}\n\nstruct Holder { cell: Shared<int> }\n\n\
               impl Send for Holder {\n    fn go(self) int { return 1 }\n}\n\nfn main() void {\n}\n";
    assert!(is_clean(src), "{:?}", codes(src));
}

#[test]
fn send_cannot_be_derived() {
    let src = program("@derive(Send)\nstruct A { n: int }\n");
    assert_eq!(codes(&src), vec!["TYPE-116"]);
}

#[test]
fn a_marker_impl_does_not_change_what_a_program_does() {
    let src = program(
        "struct Job { id: int }\n\nimpl Send for Job {\n}\n\n\
         fn main2() void {\n    let j = Job { id = 7 }\n    println(j.id)\n}\n")
        .replace("fn main() void {\n}\n", "fn main() void {\n    main2()\n}\n");
    assert!(is_clean(&src), "{:?}", codes(&src));
    let tokens = ubel_stratum::lexer::tokenize(&src).expect("source should lex");
    let arena = AstArena::new();
    let ast = ubel_stratum_rd::parse(&arena, &tokens, src.clone()).unwrap_or_else(|_| panic!("parse"));
    let ctx = ubel_stratum::sema::analyse(&ast, &arena, src.clone()).unwrap_or_else(|_| panic!("sema"));
    ubel_stratum::builtins::global::io::start_output_capture();
    let mut interp = Interpreter::new(&arena);
    interp.set_int_literal_types(ctx.int_literal_types);
    interp.set_trait_call_sites(ctx.trait_call_sites);
    interp.run_program(&ast).expect("program should run");
    assert_eq!(ubel_stratum::builtins::global::io::take_captured_output(), "7\n");
}
