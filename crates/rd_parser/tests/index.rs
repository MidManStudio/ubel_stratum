//! Indexing, slice S5d.
//!
//! `Index<Idx, Out>` (`fn index(self, idx: Idx) Out`) and `IndexSet<Idx, Val>`
//! (`fn index_set(self, idx: Idx, value: Val)`) are prelude traits (`@tier(low)`).
//! A struct or an enum that implements them is what `a[i]` and `a[i] = v`
//! call, a type has one impl per kind of index (`Index<int, T>` and
//! `Index<string, T>` coexist), `a[i] += v` needs both, and a type may have
//! either alone. `List`, arrays, `Dictionary` and strings satisfy the
//! traits natively, so a bound such as `C: Index<int, int>` accepts a `List`
//! and any impl.
//!
//! The built-ins are typed as well: a `Dictionary` by its key type, giving
//! its value type; a string gives a `char`; a tuple read by a literal gives
//! that item; the index of a `List`, string or tuple has to be an `int`; and
//! what the run time cannot index (a number, a `Queue`, a `Stack`, a `Set`,
//! a `Pool`) or assign through (a string, a tuple) is `TYPE-137`. A negative
//! index on a write counts from the end, as it does on a read.
//!
//! See docs/TRAITS_DESIGN.md, "What S5d settled".

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

fn clean(source: &str) {
    let (stage, c) = codes(source);
    assert!(matches!(stage, Stage::Clean), "expected clean, got {c:?}: {:?}\n{source}", messages(source));
}

fn one_sema(source: &str, code: &str) {
    let (stage, c) = codes(source);
    assert!(matches!(stage, Stage::Sema), "{stage:?} {c:?}\n{source}");
    assert_eq!(c, vec![code], "{:?}\n{source}", messages(source));
}

/// A grid read and written by an `int`, read by a `(int, int)`, and by a
/// `string` key (a column name).
const GRID: &str = r#"
struct Grid {
    cells: List<int>,
    w: int,
}

impl Index<int, int> for Grid {
    fn index(self, idx: int) int {
        return self.cells[idx]
    }
}

impl Index<(int, int), int> for Grid {
    fn index(self, idx: (int, int)) int {
        return self.cells[idx[0] * self.w + idx[1]]
    }
}

impl Index<string, int> for Grid {
    fn index(self, idx: string) int {
        return self.cells[0] + 1000
    }
}

impl IndexSet<int, int> for Grid {
    fn index_set(self, idx: int, value: int) {
        self.cells[idx] = value
    }
}
"#;

fn with_grid(body: &str) -> String {
    format!("{GRID}\nfn main() void {{\n    let g = Grid {{ cells = [1, 2, 3, 4], w = 2 }}\n{body}\n}}\n")
}

// ── Run time: a struct is indexed through its impls ─────────────

#[test]
fn a_read_calls_the_index_impl() {
    assert_eq!(out(&with_grid("    println(g[2])")), "3\n");
}

#[test]
fn a_write_calls_the_index_set_impl() {
    assert_eq!(out(&with_grid("    g[0] = 10\n    println(g[0])\n    println(g.cells[0])")), "10\n10\n");
}

#[test]
fn a_type_has_one_impl_per_kind_of_index() {
    let src = with_grid("    let a: int = 1\n    let b: int = 0\n    let p = (a, b)\n    println(g[p])\n    println(g[\"x\"])\n    println(g[3])");
    assert_eq!(out(&src), "3\n1001\n4\n");
}

#[test]
fn a_compound_assignment_reads_and_writes_through_the_impls() {
    assert_eq!(out(&with_grid("    g[1] += 5\n    println(g[1])\n    g[1] -= 1\n    println(g[1])\n    g[1] *= 3\n    println(g[1])")), "7\n6\n18\n");
}

#[test]
fn an_index_impl_may_return_another_type_and_a_struct() {
    let src = r#"
struct P { x: int }
struct Row { items: List<P> }
impl Index<int, P> for Row {
    fn index(self, idx: int) P { return self.items[idx] }
}
struct Table { rows: List<Row> }
impl Index<int, Row> for Table {
    fn index(self, idx: int) Row { return self.rows[idx] }
}
fn main() void {
    let r0 = Row { items = [P { x = 1 }, P { x = 2 }] }
    let r1 = Row { items = [P { x = 3 }, P { x = 4 }] }
    let t = Table { rows = [r0, r1] }
    println(t[1][0].x)
    t[0][1].x = 20
    println(t[0][1].x)
}
"#;
    assert_eq!(out(src), "3\n20\n");
}

#[test]
fn a_type_may_have_only_the_read_side() {
    let src = "struct Ro { n: int }\nimpl Index<int, int> for Ro {\n    fn index(self, idx: int) int { return self.n + idx }\n}\nfn main() void {\n    let r = Ro { n = 10 }\n    println(r[5])\n}\n";
    assert_eq!(out(src), "15\n");
}

#[test]
fn a_type_may_have_only_the_write_side() {
    let src = "struct Wo { n: int }\nimpl IndexSet<int, int> for Wo {\n    fn index_set(self, idx: int, value: int) { self.n = idx + value }\n}\nfn main() void {\n    let w = Wo { n = 0 }\n    w[3] = 4\n    println(w.n)\n}\n";
    assert_eq!(out(src), "7\n");
}

#[test]
fn an_enum_can_be_indexed() {
    let src = r#"
enum Day { Mon, Tue }
impl Index<int, string> for Day {
    fn index(self, idx: int) string { return "d" }
}
fn main() void {
    let d = Day.Mon
    println(d[0])
}
"#;
    assert_eq!(out(src), "d\n");
}

#[test]
fn the_methods_can_be_called_by_name() {
    let src = with_grid("    println(g.index(1))\n    g.index_set(1, 20)\n    println(g[1])");
    assert_eq!(out(&src), "2\n20\n");
}

#[test]
fn ownership_wrappers_are_looked_through() {
    let src = format!("{GRID}\n@tier(low)\nfn main() void {{\n    let g = Shared.new(Grid {{ cells = [1, 2, 3, 4], w = 2 }})\n    println(g[1])\n    g[1] = 9\n    println(g[1])\n}}\n");
    assert_eq!(out(&src), "2\n9\n");
}

#[test]
fn an_index_impl_for_a_generic_type_serves_every_instance() {
    let src = r#"
struct Bag<T> { items: List<T> }
impl<T> Index<int, T> for Bag<T> {
    fn index(self, idx: int) T { return self.items[idx] }
}
fn main() void {
    let a = Bag { items = [1, 2, 3] }
    println(a[1])
    let b = Bag { items = ["x", "y"] }
    println(b[1])
}
"#;
    assert_eq!(out(src), "2\ny\n");
}

// ── Bounds ──────────────────────────────────────────────────────

const GENERIC: &str = r#"
fn first<C: Index<int, int>>(c: C) int {
    return c[0]
}
fn put<C: IndexSet<int, int>>(c: C, v: int) {
    c[0] = v
}
fn bump<C: Index<int, int> + IndexSet<int, int>>(c: C) {
    c[0] += 1
}
fn by_name<C: Index<int, int>>(c: C) int {
    return c.index(1)
}
"#;

#[test]
fn a_bound_is_satisfied_by_lists_and_impls() {
    let src = format!("{GRID}{GENERIC}\nfn main() void {{\n    let g = Grid {{ cells = [1, 2, 3, 4], w = 2 }}\n    let l = [10, 20, 30]\n    println(first(g))\n    println(first(l))\n    put(g, 7)\n    put(l, 70)\n    println(first(g))\n    println(first(l))\n    bump(g)\n    bump(l)\n    println(g[0] + l[0])\n    println(by_name(g))\n    println(by_name(l))\n}}\n");
    assert_eq!(out(&src), "1\n10\n7\n70\n79\n2\n20\n");
}

#[test]
fn a_dictionary_and_a_string_satisfy_the_bounds_for_their_own_index() {
    let src = r#"
fn lookup<C: Index<string, int>>(c: C) int { return c["a"] }
fn store<C: IndexSet<string, int>>(c: C) { c["b"] = 5 }
fn letter<C: Index<int, char>>(c: C) char { return c[1] }
fn main() void {
    let d: Dictionary<string, int> = Dictionary.new()
    d.set("a", 3)
    store(d)
    println(lookup(d))
    println(d["b"])
    println(letter("xyz"))
}
"#;
    assert_eq!(out(src), "3\n5\ny\n");
}

#[test]
fn a_string_does_not_satisfy_the_write_bound() {
    let src = "fn store<C: IndexSet<int, char>>(c: C) {\n    c[0] = 'a'\n}\nfn main() void {\n    store(\"abc\")\n}\n";
    one_sema(src, "TYPE-126");
}

#[test]
fn a_type_with_another_index_kind_does_not_satisfy_the_bound() {
    let src = "struct S { n: int }\nimpl Index<string, int> for S {\n    fn index(self, idx: string) int { return 1 }\n}\nfn first<C: Index<int, int>>(c: C) int { return c[0] }\nfn main() void {\n    let s = S { n = 1 }\n    println(first(s))\n}\n";
    one_sema(src, "TYPE-126");
}

#[test]
fn an_unbounded_parameter_cannot_be_indexed() {
    let read = "fn bad<T>(a: T) int {\n    return a[0]\n}\nfn main() void {\n}\n";
    one_sema(read, "TYPE-131");
    assert_eq!(suggestions(read), vec![Some("declare the bound, as in `<T: Index<Idx, Out>>`".to_string())]);
    let write = "fn bad<T>(a: T) {\n    a[0] = 1\n}\nfn main() void {\n}\n";
    one_sema(write, "TYPE-131");
    assert_eq!(suggestions(write), vec![Some("declare the bound, as in `<T: IndexSet<Idx, Val>>`".to_string())]);
}

#[test]
fn a_read_bound_does_not_allow_a_write() {
    let src = "fn bad<C: Index<int, int>>(c: C) {\n    c[0] = 1\n}\nfn main() void {\n}\n";
    one_sema(src, "TYPE-131");
}

// ── Sema: a struct with no impl for the index ───────────────────

#[test]
fn a_missing_read_impl_names_the_index_and_the_kinds_there_are() {
    let src = with_grid("    let r = g[1.5]");
    one_sema(&src, "TYPE-136");
    let m = messages(&src).remove(0);
    assert!(m.contains("cannot index `Grid` with `double`") && m.contains("Index<Idx, Out>"), "{m}");
    let s = suggestions(&src).remove(0).unwrap();
    assert!(s.contains("int") && s.contains("string"), "{s}");
}

#[test]
fn a_struct_with_no_index_trait_says_what_to_add() {
    let src = "struct S { n: int }\nfn main() void {\n    let s = S { n = 1 }\n    let r = s[0]\n}\n";
    one_sema(src, "TYPE-136");
    let s = suggestions(src).remove(0).unwrap();
    assert!(s.contains("impl Index<int, Out> for S"), "{s}");
}

#[test]
fn a_missing_write_impl_is_reported_for_assignment() {
    let src = "struct Ro { n: int }\nimpl Index<int, int> for Ro {\n    fn index(self, idx: int) int { return 1 }\n}\nfn main() void {\n    let r = Ro { n = 1 }\n    r[0] = 5\n}\n";
    one_sema(src, "TYPE-136");
    let m = messages(src).remove(0);
    assert!(m.contains("cannot assign through an index of `Ro`") && m.contains("IndexSet<Idx, Val>"), "{m}");
    let s = suggestions(src).remove(0).unwrap();
    assert!(s.contains("impl IndexSet<int, Val> for Ro"), "{s}");
}

#[test]
fn a_compound_assignment_needs_both_sides() {
    let ro = "struct Ro { n: int }\nimpl Index<int, int> for Ro {\n    fn index(self, idx: int) int { return 1 }\n}\nfn main() void {\n    let r = Ro { n = 1 }\n    r[0] += 5\n}\n";
    one_sema(ro, "TYPE-136");
    let wo = "struct Wo { n: int }\nimpl IndexSet<int, int> for Wo {\n    fn index_set(self, idx: int, value: int) { }\n}\nfn main() void {\n    let w = Wo { n = 1 }\n    w[0] += 5\n}\n";
    one_sema(wo, "TYPE-136");
}

#[test]
fn the_written_value_has_to_be_the_impls_value_type() {
    let src = with_grid("    g[0] = \"x\"");
    one_sema(&src, "TYPE-101");
}

#[test]
fn the_result_of_a_read_is_the_impls_out_type() {
    let src = with_grid("    let s: string = g[0]");
    one_sema(&src, "TYPE-101");
}

// ── Sema: impl mistakes ─────────────────────────────────────────

#[test]
fn a_second_impl_for_one_kind_of_index_overlaps() {
    let src = format!("{GRID}\nimpl Index<int, double> for Grid {{\n    fn index(self, idx: int) double {{ return 1.0 }}\n}}\nfn main() void {{\n}}\n");
    let (_, c) = codes(&src);
    assert_eq!(c.first().copied(), Some("TYPE-128"));
}

#[test]
fn the_read_and_the_write_trait_do_not_overlap_each_other() {
    clean(&format!("{GRID}\nfn main() void {{\n}}\n"));
}

#[test]
fn an_index_impl_for_a_built_in_type_is_rejected() {
    for target in ["List<int>", "string", "int", "(int, int)", "Dictionary<string, int>"] {
        let src = format!("impl Index<int, int> for {target} {{\n    fn index(self, idx: int) int {{ return 1 }}\n}}\nfn main() void {{\n}}\n");
        one_sema(&src, "TYPE-129");
        assert!(messages(&src)[0].contains("Index or IndexSet"), "{target}: {:?}", messages(&src));
    }
    let src = "struct V { x: int }\nimpl Index<V, int> for double {\n    fn index(self, idx: V) int { return 1 }\n}\nfn main() void {\n}\n";
    one_sema(src, "TYPE-129");
}

#[test]
fn a_bare_type_parameter_as_the_index_is_rejected() {
    let src = "struct Box<T> { v: T }\nimpl<T> Index<T, T> for Box<T> {\n    fn index(self, idx: T) T { return idx }\n}\nfn main() void {\n}\n";
    one_sema(src, "TYPE-129");
}

#[test]
fn every_index_trait_is_low_so_a_written_high_tier_is_rejected() {
    let read = "struct V { x: int }\nimpl Index<int, int> for V {\n    @tier(high)\n    fn index(self, idx: int) int { return 1 }\n}\nfn main() void {\n}\n";
    one_sema(read, "TIER-016");
    let write = "struct V { x: int }\nimpl IndexSet<int, int> for V {\n    @tier(high)\n    fn index_set(self, idx: int, value: int) { }\n}\nfn main() void {\n}\n";
    one_sema(write, "TIER-016");
}

#[test]
fn a_program_trait_named_index_takes_the_name_and_the_index_is_unchecked() {
    let src = "trait Index<T> {\n    fn at(self, k: T) int\n}\nstruct V { x: int }\nimpl Index<int> for V {\n    fn at(self, k: int) int { return 1 }\n}\nfn main() void {\n    let v = V { x = 1 }\n    println(v.at(2))\n}\n";
    clean(src);
    assert_eq!(out(src), "1\n");
}

// ── Built-ins: typed, and the ones the run time cannot index ────

#[test]
fn a_dictionary_is_indexed_by_its_key_and_gives_its_value() {
    let ok = "fn main() void {\n    let d: Dictionary<string, int> = Dictionary.new()\n    d.set(\"a\", 1)\n    d[\"b\"] = 2\n    d[\"a\"] += 10\n    let v: int = d[\"a\"]\n    println(v)\n    println(d[\"b\"])\n}\n";
    assert_eq!(out(ok), "11\n2\n");
    let value = "fn main() void {\n    let d: Dictionary<string, int> = Dictionary.new()\n    let v: string = d[\"a\"]\n}\n";
    one_sema(value, "TYPE-101");
    let key = "fn main() void {\n    let d: Dictionary<string, int> = Dictionary.new()\n    let v = d[1]\n}\n";
    one_sema(key, "TYPE-101");
    let stored = "fn main() void {\n    let d: Dictionary<string, int> = Dictionary.new()\n    d[\"a\"] = \"x\"\n}\n";
    one_sema(stored, "TYPE-101");
}

#[test]
fn a_string_gives_a_char_and_is_read_only() {
    let ok = "fn main() void {\n    let s = \"abc\"\n    let c: char = s[1]\n    println(c)\n}\n";
    assert_eq!(out(ok), "b\n");
    let wrong = "fn main() void {\n    let s = \"abc\"\n    let c: int = s[1]\n}\n";
    one_sema(wrong, "TYPE-101");
    let write = "fn main() void {\n    let s = \"abc\"\n    s[0] = 'z'\n}\n";
    one_sema(write, "TYPE-137");
    assert!(messages(write)[0].contains("assign through an index"), "{:?}", messages(write));
    let compound = "fn main() void {\n    let s = \"abc\"\n    s[0] += 'z'\n}\n";
    assert!(codes(compound).1.contains(&"TYPE-137"), "{:?}", codes(compound));
}

#[test]
fn a_tuple_read_by_a_literal_gives_that_item() {
    let ok = "fn main() void {\n    let t = (1, \"a\")\n    let s: string = t[1]\n    let n: int = t[0]\n    println(s)\n    println(n)\n}\n";
    assert_eq!(out(ok), "a\n1\n");
    let wrong = "fn main() void {\n    let t = (1, \"a\")\n    let n: int = t[1]\n}\n";
    one_sema(wrong, "TYPE-101");
    let by_variable = "fn main() void {\n    let t = (1, \"a\")\n    let i = 0\n    let x = t[i]\n}\n";
    clean(by_variable);
    let write = "fn main() void {\n    let t = (1, \"a\")\n    t[0] = 2\n}\n";
    one_sema(write, "TYPE-137");
}

#[test]
fn the_index_of_a_list_a_string_or_a_tuple_has_to_be_an_int() {
    let list = "fn main() void {\n    let l = [1, 2, 3]\n    let a = l[\"x\"]\n}\n";
    one_sema(list, "TYPE-101");
    assert!(messages(list)[0].contains("expected `int`, found `string`"), "{:?}", messages(list));
    let sized = "fn main() void {\n    let l = [1, 2, 3]\n    let u: u32 = 1u32\n    let a = l[u]\n}\n";
    one_sema(sized, "TYPE-101");
    let long = "fn main() void {\n    let l = [1, 2, 3]\n    let i: i64 = 1i64\n    println(l[i])\n}\n";
    assert_eq!(out(long), "2\n");
    let string = "fn main() void {\n    let s = \"abc\"\n    let c = s[true]\n}\n";
    one_sema(string, "TYPE-101");
    let write = "fn main() void {\n    let l = [1, 2, 3]\n    l[\"x\"] = 1\n}\n";
    one_sema(write, "TYPE-101");
}

#[test]
fn a_list_write_has_to_be_of_the_element_type() {
    let src = "fn main() void {\n    let l = [1, 2, 3]\n    l[1] = \"x\"\n}\n";
    one_sema(src, "TYPE-101");
}

#[test]
fn what_the_run_time_cannot_index_is_type_137() {
    let queue = "fn main() void {\n    let q = Queue.new()\n    q.enqueue(1)\n    let b = q[0]\n}\n";
    one_sema(queue, "TYPE-137");
    assert!(messages(queue)[0].contains("Queue"), "{:?}", messages(queue));
    let stack = "fn main() void {\n    let s = Stack.new()\n    s.push(1)\n    let b = s[0]\n}\n";
    one_sema(stack, "TYPE-137");
    let number = "fn main() void {\n    let n = 5\n    let c = n[0]\n}\n";
    one_sema(number, "TYPE-137");
    let set = "fn f(s: Set<int>) int {\n    return s[0]\n}\nfn main() void {\n}\n";
    one_sema(set, "TYPE-137");
    let write = "fn main() void {\n    let q = Queue.new()\n    q[0] = 1\n}\n";
    one_sema(write, "TYPE-137");
}

#[test]
fn a_negative_index_counts_from_the_end_on_a_write_as_on_a_read() {
    let src = "fn main() void {\n    let l = [1, 2, 3]\n    println(l[-1])\n    l[-1] = 9\n    println(l[2])\n    l[-3] = 7\n    println(l[0])\n    l[-1] += 1\n    println(l[2])\n}\n";
    assert_eq!(out(src), "3\n9\n7\n10\n");
}

#[test]
fn a_negative_index_past_the_start_is_a_clear_panic() {
    let src = "fn main() void {\n    let l = [1, 2, 3]\n    l[-5] = 1\n}\n";
    let err = run(src).unwrap_err();
    assert!(err.contains("index -5 out of bounds (len 3)"), "{err}");
    let big = "fn main() void {\n    let l = [1, 2, 3]\n    l[3] = 1\n}\n";
    let err = run(big).unwrap_err();
    assert!(err.contains("index 3 out of bounds (len 3)"), "{err}");
}

#[test]
fn list_and_dictionary_indexing_keep_working_untouched() {
    let src = "fn main() void {\n    let l = [1, 2, 3]\n    l[1] = l[0] + l[2]\n    println(l[1])\n    let d: Dictionary<int, string> = Dictionary.new()\n    d[5] = \"five\"\n    println(d[5])\n}\n";
    assert_eq!(out(src), "4\nfive\n");
}

#[test]
fn an_array_and_a_slice_are_indexed_by_an_int() {
    let src = "fn first(s: [3]int) int {\n    return s[0]\n}\nfn main() void {\n}\n";
    clean(src);
    let bad = "fn first(s: [3]int) int {\n    return s[\"x\"]\n}\nfn main() void {\n}\n";
    one_sema(bad, "TYPE-101");
}

// ── A value whose type is not known yet, and one that failed ────

#[test]
fn a_value_with_no_type_yet_is_not_an_error() {
    let src = "fn main() void {\n    let f = fn(c, i) c[i]\n    let l = [4, 5]\n    println(f(l, 1))\n}\n";
    assert_eq!(out(src), "5\n");
}

#[test]
fn an_index_of_an_expression_that_already_failed_adds_nothing() {
    let head = "struct P { x: int }\nfn main() void {\n    let n = 5\n    let t = *n\n    let l = [1, 2]\n";
    for body in ["    let a = t[0]", "    let a = l[t]", "    let a = t[t]"] {
        let src = format!("{head}{body}\n}}\n");
        let (stage, c) = codes(&src);
        assert!(matches!(stage, Stage::Sema), "{body}");
        assert!(c.len() <= 2, "{body}: {c:?}");
    }
}

// ── Tuples now unify, which a tuple-indexed impl needs ──────────

#[test]
fn two_tuple_types_unify_item_by_item() {
    let src = "fn take(k: (int, int)) int { return k[0] + k[1] }\nfn main() void {\n    let a: int = 3\n    let b: int = 4\n    let p = (a, b)\n    println(take(p))\n}\n";
    assert_eq!(out(src), "7\n");
    let arity = "fn take(k: (int, int)) int { return 1 }\nfn main() void {\n    let a: int = 3\n    let p = (a, a, a)\n    println(take(p))\n}\n";
    one_sema(arity, "TYPE-101");
    let item = "fn take(k: (int, int)) int { return 1 }\nfn main() void {\n    let a: int = 3\n    let p = (a, \"x\")\n    println(take(p))\n}\n";
    one_sema(item, "TYPE-101");
}

// ── Wrappers, explicit calls through a bound ────────────────────

#[test]
fn a_unique_value_is_indexed_and_assigned_through_once() {
    let read = format!("{GRID}\n@tier(low)\nfn main() void {{\n    let g = Unique.new(Grid {{ cells = [1, 2, 3, 4], w = 2 }})\n    println(g[1])\n}}\n");
    assert_eq!(out(&read), "2\n");
    let write = format!("{GRID}\n@tier(low)\nfn main() void {{\n    let g = Unique.new(Grid {{ cells = [1, 2, 3, 4], w = 2 }})\n    g[1] = 9\n}}\n");
    assert_eq!(out(&write), "");
}

#[test]
fn an_optional_value_is_looked_through() {
    clean("fn f(l: List<int>?) int {\n    return l[0]\n}\nfn main() void {\n}\n");
}

#[test]
fn the_write_method_can_be_called_by_name_through_a_bound() {
    let src = format!("{GRID}\nfn setn<C: IndexSet<int, int>>(c: C) {{\n    c.index_set(1, 20)\n}}\nfn getn<C: Index<int, int>>(c: C) int {{\n    return c.index(1)\n}}\nfn main() void {{\n    let g = Grid {{ cells = [1, 2, 3, 4], w = 2 }}\n    let l = [1, 2, 3]\n    setn(g)\n    setn(l)\n    println(getn(g))\n    println(getn(l))\n    let d: Dictionary<int, int> = Dictionary.new()\n    setn(d)\n    println(getn(d))\n}}\n");
    assert_eq!(out(&src), "20\n20\n20\n");
}

#[test]
fn a_dictionary_key_of_the_wrong_type_is_expected_and_found_the_right_way_round() {
    let src = "fn main() void {\n    let d: Dictionary<string, int> = Dictionary.new()\n    let v = d[1]\n}\n";
    assert!(messages(src)[0].contains("expected `string`, found `int`"), "{:?}", messages(src));
}

#[test]
fn indexing_a_struct_when_the_program_declares_its_own_index_is_unchecked() {
    // Each of the two traits is shadowed on its own: the read is left to the
    // run time, and the write still needs the prelude's `IndexSet`.
    let read = "trait Index<T> {\n    fn at(self, k: T) int\n}\nstruct V { x: int }\nfn main() void {\n    let v = V { x = 1 }\n    let a = v[0]\n}\n";
    clean(read);
    let write = read.replace("let a = v[0]", "v[0] = 1");
    one_sema(&write, "TYPE-136");
}
