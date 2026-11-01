//! `Iterator<T>` and what a `for` loop can walk, slice S5b.
//!
//! `Iterator<T>` is a generic prelude trait with one method, `fn next(self)
//! T?`. A struct or an enum that implements it can be the iterable of a `for`
//! loop, which calls `next` until it returns `null` and binds each item. The
//! loop is lazy (an endless iterator is stopped by `break`) and advances the
//! iterator itself: a struct is shared, so after the loop it stands where
//! `next` left it.
//!
//! The same slice makes `Queue`, `Stack` and `Dictionary` iterable (a queue
//! front to back, a stack bottom to top as it prints, a dictionary as
//! `(key, value)` pairs in insertion order) and makes a `for` over anything
//! else that cannot be walked a sema error (`TYPE-135`) instead of a panic at
//! run time. An impl of `Iterator` is only for a struct or an enum
//! (`TYPE-129` for a built-in type), and an optional item type is `TYPE-129`
//! because `null` is what ends the loop.
//!
//! See docs/TRAITS_DESIGN.md, "What S5b settled".

use ubel_stratum::ast::arena::AstArena;
use ubel_stratum::interpreter::Interpreter;
use ubel_stratum_rd::{check_source, Stage};

fn codes(source: &str) -> (Stage, Vec<&'static str>) {
    let report = check_source(source);
    let codes = report.diagnostics.iter().map(|d| d.code).collect();
    (report.stage, codes)
}

fn messages(source: &str) -> Vec<String> {
    check_source(source).diagnostics.iter().map(|d| d.message.clone()).collect()
}

fn suggestions(source: &str) -> Vec<Option<String>> {
    check_source(source).diagnostics.iter().map(|d| d.suggestion.clone()).collect()
}

fn run(source: &str) -> Result<String, String> {
    assert!(matches!(check_source(source).stage, Stage::Clean), "program should check clean: {:?}", messages(source));
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

/// The 1-based line and column of each diagnostic's primary span.
fn lines(source: &str) -> Vec<(usize, usize)> {
    check_source(source).diagnostics.iter()
        .map(|d| (d.primary_span.line as usize, d.primary_span.column as usize))
        .collect()
}

fn assert_clean(source: &str) {
    assert_eq!(codes(source), (Stage::Clean, vec![]), "{source}");
}

fn assert_rejected(source: &str, code: &'static str) {
    assert_eq!(codes(source), (Stage::Sema, vec![code]), "{source}");
}

/// A struct that counts from 1 to `limit`.
const COUNTER: &str = "struct Counter { n: int, limit: int }\n\
                       impl Iterator<int> for Counter {\n\
                           fn next(self) int? {\n\
                               if self.n >= self.limit { return null }\n\
                               self.n = self.n + 1\n\
                               return self.n\n\
                           }\n\
                       }\n";

fn with_counter(rest: &str) -> String {
    format!("{COUNTER}{rest}")
}

// ── A loop over a user iterator ──────────────────────────────────────────────

#[test]
fn a_loop_calls_next_until_it_returns_null() {
    let src = with_counter("fn main() void {\n\
                                let c = Counter { n = 0, limit = 3 }\n\
                                for x in c { println(x) }\n\
                            }\n");
    assert_eq!(out(&src), "1\n2\n3\n");
}

#[test]
fn the_loop_advances_the_iterator_itself() {
    let src = with_counter("fn main() void {\n\
                                let c = Counter { n = 0, limit = 3 }\n\
                                for x in c { }\n\
                                println(c.n)\n\
                                println(c.next() == null)\n\
                            }\n");
    assert_eq!(out(&src), "3\ntrue\n");
}

#[test]
fn an_iterator_can_be_resumed_after_a_break() {
    let src = with_counter("fn main() void {\n\
                                let c = Counter { n = 0, limit = 5 }\n\
                                for x in c { if x == 2 { break } }\n\
                                for y in c { println(y) }\n\
                            }\n");
    assert_eq!(out(&src), "3\n4\n5\n");
}

#[test]
fn an_empty_iterator_never_runs_the_body() {
    let src = with_counter("fn main() void {\n\
                                let c = Counter { n = 0, limit = 0 }\n\
                                for x in c { println(\"body\") }\n\
                                println(\"after\")\n\
                            }\n");
    assert_eq!(out(&src), "after\n");
}

#[test]
fn continue_and_break_work_in_a_loop_over_an_iterator() {
    let src = with_counter("fn main() void {\n\
                                let c = Counter { n = 0, limit = 10 }\n\
                                for x in c {\n\
                                    if x == 2 { continue }\n\
                                    if x == 5 { break }\n\
                                    println(x)\n\
                                }\n\
                            }\n");
    assert_eq!(out(&src), "1\n3\n4\n");
}

#[test]
fn a_return_inside_the_loop_leaves_the_function() {
    let src = with_counter("fn first_even(c: Counter) int {\n\
                                for x in c { if x % 2 == 0 { return x } }\n\
                                return -1\n\
                            }\n\
                            fn main() void {\n\
                                println(first_even(Counter { n = 0, limit = 10 }))\n\
                                println(first_even(Counter { n = 0, limit = 1 }))\n\
                            }\n");
    assert_eq!(out(&src), "2\n-1\n");
}

#[test]
fn loops_nest_over_separate_iterators() {
    let src = with_counter("fn main() void {\n\
                                let a = Counter { n = 0, limit = 2 }\n\
                                for i in a {\n\
                                    let b = Counter { n = 0, limit = 2 }\n\
                                    for j in b { println(i, j) }\n\
                                }\n\
                            }\n");
    assert_eq!(out(&src), "1 1\n1 2\n2 1\n2 2\n");
}

#[test]
fn the_loop_is_lazy_so_an_endless_iterator_is_stopped_by_break() {
    let src = "struct Nat { n: int }\n\
               impl Iterator<int> for Nat {\n\
                   fn next(self) int? { self.n = self.n + 1 return self.n }\n\
               }\n\
               fn main() void {\n\
                   let s = Nat { n = 0 }\n\
                   for x in s { if x > 3 { break } println(x) }\n\
                   println(s.n)\n\
               }\n";
    assert_eq!(out(src), "1\n2\n3\n4\n");
}

#[test]
fn the_item_can_be_a_string_or_a_struct() {
    let src = "struct P { x: int, y: int }\n\
               struct Walk { i: int }\n\
               impl Iterator<P> for Walk {\n\
                   fn next(self) P? {\n\
                       if self.i >= 2 { return null }\n\
                       self.i = self.i + 1\n\
                       return P { x = self.i, y = self.i * 10 }\n\
                   }\n\
               }\n\
               struct Words { done: bool }\n\
               impl Iterator<string> for Words {\n\
                   fn next(self) string? {\n\
                       if self.done { return null }\n\
                       self.done = true\n\
                       return \"hi\"\n\
                   }\n\
               }\n\
               fn main() void {\n\
                   let w = Walk { i = 0 }\n\
                   for p in w { println(p.x, p.y) }\n\
                   let ws = Words { done = false }\n\
                   for s in ws { println(s.to_upper()) }\n\
               }\n";
    assert_eq!(out(src), "1 10\n2 20\nHI\n");
}

#[test]
fn a_generic_struct_can_implement_iterator() {
    let src = "struct Once<T> { value: T, done: bool }\n\
               impl<T> Iterator<T> for Once<T> {\n\
                   fn next(self) T? {\n\
                       if self.done { return null }\n\
                       self.done = true\n\
                       return self.value\n\
                   }\n\
               }\n\
               fn main() void {\n\
                   let a = Once { value = \"hi\", done = false }\n\
                   for s in a { println(s) }\n\
                   let b = Once { value = 7, done = false }\n\
                   for n in b { println(n + 1) }\n\
               }\n";
    assert_eq!(out(src), "hi\n8\n");
}

#[test]
fn an_enum_can_implement_iterator() {
    // An enum value cannot advance itself, so this one ends at once.
    let src = "enum Never { A, B }\n\
               impl Iterator<int> for Never {\n\
                   fn next(self) int? { return null }\n\
               }\n\
               fn main() void {\n\
                   let e = Never.A\n\
                   for x in e { println(x) }\n\
                   println(\"done\")\n\
               }\n";
    assert_eq!(out(src), "done\n");
}

#[test]
fn a_type_parameter_with_an_iterator_bound_can_be_looped_over() {
    let src = with_counter("fn total<I: Iterator<int>>(it: I) int {\n\
                                let sum = 0\n\
                                for x in it { sum = sum + x }\n\
                                return sum\n\
                            }\n\
                            fn main() void {\n\
                                println(total(Counter { n = 0, limit = 4 }))\n\
                            }\n");
    assert_eq!(out(&src), "10\n");
}

#[test]
fn the_item_type_of_a_bound_is_the_loop_variable_type() {
    let src = with_counter("fn bad<I: Iterator<int>>(it: I) void {\n\
                                for x in it { let s: string = x }\n\
                            }\n\
                            fn main() void { }\n");
    assert_rejected(&src, "TYPE-101");
}

#[test]
fn a_loop_over_a_reference_or_an_ownership_wrapper_uses_the_value() {
    let src = with_counter("@tier(low)\n\
                            fn main() void {\n\
                                let u = Unique.new(Counter { n = 0, limit = 2 })\n\
                                for x in u { println(x) }\n\
                                let ul = Unique.new([5, 6])\n\
                                for y in ul { println(y) }\n\
                                let sh = Shared.new([8, 9])\n\
                                for z in sh { println(z) }\n\
                            }\n");
    assert_eq!(out(&src), "1\n2\n5\n6\n8\n9\n");
}

#[test]
fn a_loop_over_a_borrowed_iterator_advances_the_original() {
    let src = with_counter("fn main() void {\n\
                                let c = Counter { n = 0, limit = 2 }\n\
                                let r = &c\n\
                                for x in r { println(x) }\n\
                                println(c.n)\n\
                            }\n");
    assert_eq!(out(&src), "1\n2\n2\n");
}

#[test]
fn the_loop_calls_the_trait_method_not_an_inherent_one_of_the_same_name() {
    let src = "struct S { n: int }\n\
               extend S { fn next(self) int { return 100 } }\n\
               impl Iterator<int> for S {\n\
                   fn next(self) int? {\n\
                       if self.n >= 2 { return null }\n\
                       self.n = self.n + 1\n\
                       return self.n\n\
                   }\n\
               }\n\
               fn main() void {\n\
                   let s = S { n = 0 }\n\
                   for x in s { println(x) }\n\
                   println(s.next())\n\
               }\n";
    assert_eq!(out(src), "1\n2\n100\n");
}

#[test]
fn the_loop_variable_has_the_item_type() {
    let src = with_counter("fn main() void {\n\
                                let c = Counter { n = 0, limit = 1 }\n\
                                for x in c { let s: string = x }\n\
                            }\n");
    assert_rejected(&src, "TYPE-101");
    let ok = with_counter("fn main() void {\n\
                               let c = Counter { n = 0, limit = 1 }\n\
                               for x in c { let n: int = x + 1 }\n\
                           }\n");
    assert_clean(&ok);
}

#[test]
fn a_loop_in_a_function_of_any_tier_can_walk_an_iterator() {
    // `next` is LOW, and a LOW method can be called from every tier.
    let src = with_counter("@tier(high)\n\
                            fn walk_high() void { let c = Counter { n = 0, limit = 1 } for x in c { println(\"high\", x) } }\n\
                            @tier(mid)\n\
                            fn walk_mid() void { let c = Counter { n = 0, limit = 1 } for x in c { println(\"mid\", x) } }\n\
                            @tier(low)\n\
                            fn walk_low() void { let c = Counter { n = 0, limit = 1 } for x in c { println(\"low\", x) } }\n\
                            fn main() void { walk_high() walk_mid() walk_low() }\n");
    assert_eq!(out(&src), "high 1\nmid 1\nlow 1\n");
}

// ── Queue, Stack and Dictionary are iterable ─────────────────────────────────

#[test]
fn a_queue_is_walked_front_to_back() {
    let src = "fn main() void {\n\
                   let q = Queue.new()\n\
                   q.enqueue(1)\n\
                   q.enqueue(2)\n\
                   q.enqueue(3)\n\
                   println(q)\n\
                   for x in q { println(x) }\n\
               }\n";
    assert_eq!(out(src), "Queue[1, 2, 3]\n1\n2\n3\n");
}

#[test]
fn a_stack_is_walked_bottom_to_top_as_it_prints() {
    let src = "fn main() void {\n\
                   let s = Stack.new()\n\
                   s.push(1)\n\
                   s.push(2)\n\
                   s.push(3)\n\
                   println(s)\n\
                   for x in s { println(x) }\n\
                   println(s.peek())\n\
               }\n";
    assert_eq!(out(src), "Stack[1, 2, 3]\n1\n2\n3\n3\n");
}

#[test]
fn a_dictionary_is_walked_as_pairs_in_insertion_order() {
    let src = "fn main() void {\n\
                   let d = Dictionary.new()\n\
                   d.set(\"b\", 2)\n\
                   d.set(\"a\", 1)\n\
                   d.set(\"c\", 3)\n\
                   for (k, v) in d { println(k, v) }\n\
               }\n";
    assert_eq!(out(src), "b 2\na 1\nc 3\n");
}

#[test]
fn a_dictionary_loop_variable_is_a_pair() {
    let src = "fn main() void {\n\
                   let d: Dictionary<string, int> = Dictionary.new()\n\
                   d.set(\"a\", 1)\n\
                   for p in d { let n: int = p }\n\
               }\n";
    assert_rejected(src, "TYPE-101");
    let ok = "fn main() void {\n\
                  let d: Dictionary<string, int> = Dictionary.new()\n\
                  d.set(\"a\", 1)\n\
                  for p in d { println(p) }\n\
              }\n";
    assert_eq!(out(ok), "(a, 1)\n");
}

#[test]
fn the_items_of_a_queue_and_a_stack_have_the_element_type() {
    let q = "fn main() void {\n\
                 let q: Queue<int> = Queue.new()\n\
                 for x in q { let s: string = x }\n\
             }\n";
    assert_rejected(q, "TYPE-101");
    let s = "fn main() void {\n\
                 let s: Stack<string> = Stack.new()\n\
                 for x in s { let n: int = x }\n\
             }\n";
    assert_rejected(s, "TYPE-101");
}

#[test]
fn a_loop_over_a_collection_visits_a_snapshot() {
    let src = "fn main() void {\n\
                   let q = Queue.new()\n\
                   q.enqueue(1)\n\
                   q.enqueue(2)\n\
                   for x in q { q.enqueue(9) println(x) }\n\
                   println(q.len())\n\
                   let s = Stack.new()\n\
                   s.push(1)\n\
                   for y in s { s.push(8) println(y) }\n\
                   println(s.len())\n\
               }\n";
    assert_eq!(out(src), "1\n2\n4\n1\n2\n");
}

#[test]
fn the_other_built_in_iterables_still_work() {
    let src = "fn main() void {\n\
                   let a = [1, 2]\n\
                   for x in a { println(x) }\n\
                   let t = (3, 4)\n\
                   for e in t { println(e) }\n\
                   for c in \"ab\" { println(c) }\n\
                   for i in 0 .. 2 { println(i) }\n\
                   let il = InlineList.new(3)\n\
                   il.push(7)\n\
                   for z in il { println(z) }\n\
               }\n";
    assert_eq!(out(src), "1\n2\n3\n4\na\nb\n0\n1\n7\n");
}

#[test]
fn a_loop_over_a_fallible_collection_walks_the_collection() {
    let src = "fn get() List<int>! {\n\
                   let l = [1, 2]\n\
                   return l\n\
               }\n\
               fn main() void { for x in get() { println(x) } }\n";
    assert_eq!(out(src), "1\n2\n");
}

#[test]
fn a_loop_over_an_optional_collection_walks_the_collection() {
    let src = "fn get() List<int>? {\n\
                   let l = [1, 2]\n\
                   return l\n\
               }\n\
               fn main() void { for x in get() { println(x) } }\n";
    assert_eq!(out(src), "1\n2\n");
}

// ── A `for` over a value that cannot be walked: TYPE-135 ─────────────────────

#[test]
fn a_loop_over_a_scalar_is_rejected_by_sema() {
    for (decl, what) in [
        ("let v = 5", "an integer"),
        ("let v = true", "a bool"),
        ("let v = 1.5", "a float"),
        ("let v = 'a'", "a char"),
    ] {
        let src = format!("fn main() void {{ {decl} for x in v {{ println(x) }} }}\n");
        assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-135"]), "{what}");
    }
}

#[test]
fn a_loop_over_a_literal_is_rejected_by_sema() {
    assert_rejected("fn main() void { for x in 5 { println(x) } }\n", "TYPE-135");
}

#[test]
fn the_message_and_suggestion_for_a_value_with_no_iteration() {
    let src = "fn main() void { let b = true for x in b { println(x) } }\n";
    assert_rejected(src, "TYPE-135");
    assert_eq!(messages(src), vec!["cannot iterate over a value of type `bool`".to_string()]);
    let help = suggestions(src)[0].clone().unwrap();
    assert!(help.contains("a `for` loop walks a List"), "{help}");
    assert!(help.contains("implements `Iterator<T>`"), "{help}");
}

#[test]
fn a_loop_over_an_optional_scalar_is_rejected() {
    let src = "fn main() void {\n\
                   let n: int? = 5\n\
                   for y in n { println(y) }\n\
               }\n";
    assert_rejected(src, "TYPE-135");
}

#[test]
fn a_loop_over_a_struct_without_an_iterator_impl_is_rejected() {
    let src = "struct S { n: int }\n\
               fn main() void { let s = S { n = 1 } for x in s { println(x) } }\n";
    assert_rejected(src, "TYPE-135");
    assert!(messages(src)[0].contains("type `S`"), "{:?}", messages(src));
    assert!(suggestions(src)[0].as_deref().unwrap().contains("impl Iterator<Item> for S"), "{:?}", suggestions(src));
}

#[test]
fn a_loop_over_an_enum_without_an_iterator_impl_is_rejected() {
    let src = "enum E { A, B }\n\
               fn main() void { let e = E.A for x in e { println(x) } }\n";
    assert_rejected(src, "TYPE-135");
}

#[test]
fn a_loop_over_a_linqerizer_is_rejected() {
    let src = "fn main() void {\n\
                   let l = [1, 2, 3]\n\
                   let q = l.query()\n\
                   for x in q { println(x) }\n\
               }\n";
    assert_rejected(src, "TYPE-135");
}

#[test]
fn a_loop_over_a_function_is_rejected() {
    let src = "fn f() int { return 1 }\n\
               fn main() void { for x in f { println(x) } }\n";
    assert_rejected(src, "TYPE-135");
}

#[test]
fn an_unbounded_type_parameter_cannot_be_looped_over() {
    let src = "fn f<T>(x: T) void { for y in x { println(y) } }\n\
               fn main() void { f(1) }\n";
    assert_rejected(src, "TYPE-135");
    assert!(messages(src)[0].contains("type parameter `T`"), "{:?}", messages(src));
    assert!(suggestions(src)[0].as_deref().unwrap().contains("<T: Iterator<Item>>"), "{:?}", suggestions(src));
}

#[test]
fn a_bound_that_is_not_iterator_does_not_make_a_parameter_iterable() {
    let src = "trait Named { fn name(self) string }\n\
               fn f<T: Named>(x: T) void { for y in x { println(y) } }\n\
               fn main() void { }\n";
    assert_rejected(src, "TYPE-135");
}

#[test]
fn a_loop_over_a_value_of_an_unknown_type_is_not_an_error() {
    // The list's element type is not known yet; the loop still walks a list.
    assert_clean("fn main() void { let l = List.new() for x in l { } }\n");
}

#[test]
fn an_earlier_error_does_not_add_a_loop_error() {
    let src = "fn main() void { for x in missing { println(x) } }\n";
    let (_, found) = codes(src);
    assert_eq!(found, vec!["NAME-001"]);
}

#[test]
fn a_loop_over_an_expression_that_already_failed_reports_nothing_more() {
    // A dereference of a non-reference and an `await` on a non-task are typed
    // as the error sentinel after they are reported. A loop over one used to
    // crash the type checker; it adds no diagnostic of its own.
    let deref = "fn main() void { let n = 5 for x in *n { println(x) } }\n";
    assert_eq!(codes(deref), (Stage::Sema, vec!["TYPE-114"]));
    let awaited = "fn main() void { let n = 5 for x in await n { println(x) } }\n";
    assert_eq!(codes(awaited), (Stage::Sema, vec!["TYPE-106"]));
}

#[test]
fn the_diagnostics_point_at_the_iterable_and_at_the_impl() {
    // TYPE-135 at the iterable expression (line 3, column 14 is the `b`).
    let src = "fn main() void {\n    let b = true\n    for x in b { println(x) }\n}\n";
    assert_eq!(lines(src), vec![(3, 14)]);

    // TYPE-129 for an optional item, at the `impl` (line 2, column 1).
    let optional = "struct W { n: int }\n\
                    impl Iterator<int?> for W { fn next(self) int?? { return null } }\n\
                    fn main() void { }\n";
    assert_eq!(lines(optional), vec![(2, 1)]);

    // TYPE-129 for an impl on a built-in type, at the `impl` (line 1).
    let builtin = "impl Iterator<int> for int { fn next(self) int? { return null } }\nfn main() void { }\n";
    assert_eq!(lines(builtin), vec![(1, 1)]);

    // TYPE-129 for an item made optional by a generic argument, at the loop's
    // iterable (line 12, column 10 is the `w`).
    let generic = "struct Once<T> { value: T, done: bool }\n\
                   impl<T> Iterator<T> for Once<T> {\n\
                       fn next(self) T? {\n\
                           if self.done { return null }\n\
                           self.done = true\n\
                           return self.value\n\
                       }\n\
                   }\n\
                   fn main() void {\n\
                       let o: int? = 5\n\
                       let w = Once { value = o, done = false }\n\
                       for x in w { println(x) }\n\
                   }\n";
    assert_eq!(lines(generic), vec![(12, 10)]);
}

#[test]
fn an_own_trait_named_iterator_is_not_the_one_a_loop_uses() {
    let src = "trait Iterator<T> { fn next(self) T? }\n\
               struct C { n: int }\n\
               impl Iterator<int> for C { fn next(self) int? { return null } }\n\
               fn main() void { let c = C { n = 1 } for x in c { println(x) } }\n";
    assert_rejected(src, "TYPE-135");
    // It is still an ordinary trait, so calling its method is fine.
    let ok = "trait Iterator<T> { fn next(self) T? }\n\
              struct C { n: int }\n\
              impl Iterator<int> for C { fn next(self) int? { return 7 } }\n\
              fn main() void { let c = C { n = 1 } println(c.next()) }\n";
    assert_eq!(out(ok), "7\n");
}

// ── Optional items: TYPE-129 ─────────────────────────────────────────────────

#[test]
fn an_iterator_over_optional_items_is_rejected_once_where_it_is_written() {
    let src = "struct W { n: int }\n\
               impl Iterator<int?> for W { fn next(self) int?? { return null } }\n\
               fn main() void {\n\
                   let w = W { n = 1 }\n\
                   for x in w { println(x) }\n\
               }\n";
    assert_rejected(src, "TYPE-129");
    assert!(messages(src)[0].contains("optional items"), "{:?}", messages(src));
}

#[test]
fn an_item_that_becomes_optional_through_a_generic_argument_is_rejected_at_the_loop() {
    let src = "struct Once<T> { value: T, done: bool }\n\
               impl<T> Iterator<T> for Once<T> {\n\
                   fn next(self) T? {\n\
                       if self.done { return null }\n\
                       self.done = true\n\
                       return self.value\n\
                   }\n\
               }\n\
               fn main() void {\n\
                   let o: int? = 5\n\
                   let w = Once { value = o, done = false }\n\
                   for x in w { println(x) }\n\
               }\n";
    assert_rejected(src, "TYPE-129");
}

#[test]
fn a_bound_with_an_optional_item_is_rejected_at_the_loop() {
    let src = "fn f<I: Iterator<int?>>(it: I) void { for x in it { println(x) } }\n\
               fn main() void { }\n";
    assert_rejected(src, "TYPE-129");
}

// ── An impl of Iterator is for a struct or an enum ───────────────────────────

#[test]
fn an_impl_for_a_built_in_type_is_rejected() {
    for target in ["int", "string", "List<int>", "[3]int", "Dictionary<string, int>", "(int, int)", "Queue<int>", "Stack<int>"] {
        let src = format!("impl Iterator<int> for {target} {{ fn next(self) int? {{ return null }} }}\nfn main() void {{ }}\n");
        assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-129"]), "{target}");
        assert!(messages(&src)[0].contains("built-in type"), "{target}: {:?}", messages(&src));
    }
}

#[test]
fn an_extend_block_for_a_built_in_type_can_still_have_a_method_called_next() {
    // Only an `impl Iterator for` a built-in type is refused; a plain method is not.
    let src = "extend int { fn next(self) int { return self + 1 } }\n\
               fn main() void { let n = 4 println(n.next()) }\n";
    assert_eq!(out(src), "5\n");
}

#[test]
fn an_impl_with_the_wrong_signature_is_rejected() {
    let src = "struct C { n: int }\n\
               impl Iterator<int> for C { fn next(self) int { return 1 } }\n\
               fn main() void { }\n";
    assert_rejected(src, "TYPE-125");
}

#[test]
fn an_impl_without_next_is_rejected() {
    let src = "struct C { n: int }\n\
               impl Iterator<int> for C { }\n\
               fn main() void { }\n";
    assert_rejected(src, "TYPE-123");
}

#[test]
fn a_second_impl_for_other_arguments_is_an_overlap() {
    let src = "struct C { n: int }\n\
               impl Iterator<int> for C { fn next(self) int? { return null } }\n\
               impl Iterator<string> for C { fn next(self) string? { return null } }\n\
               fn main() void { }\n";
    assert_rejected(src, "TYPE-128");
}

// ── Tiers ────────────────────────────────────────────────────────────────────

#[test]
fn next_is_low_so_an_impl_written_with_another_tier_is_rejected() {
    let src = "struct C { n: int }\n\
               impl Iterator<int> for C {\n\
                   @tier(high)\n\
                   fn next(self) int? { return null }\n\
               }\n\
               fn main() void { }\n";
    assert_rejected(src, "TIER-016");
    let ok = "struct C { n: int }\n\
              impl Iterator<int> for C {\n\
                  @tier(low)\n\
                  fn next(self) int? { return null }\n\
              }\n\
              fn main() void { }\n";
    assert_clean(ok);
}

#[test]
fn a_next_body_is_low_so_it_cannot_build_a_collection() {
    let src = "struct C { n: int }\n\
               impl Iterator<int> for C {\n\
                   fn next(self) int? { let l = List.new() return null }\n\
               }\n\
               fn main() void { }\n";
    assert_rejected(src, "TIER-009");
}

#[test]
fn a_next_body_is_low_so_it_cannot_read_a_static() {
    let src = "static COUNT: int = 0\n\
               struct C { n: int }\n\
               impl Iterator<int> for C {\n\
                   fn next(self) int? { return COUNT }\n\
               }\n\
               fn main() void { }\n";
    assert_rejected(src, "TIER-015");
}
