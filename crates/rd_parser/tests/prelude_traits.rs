//! The prelude traits, slice S2a: `PartialEq`, `Eq`, `PartialOrd`, `Ord`,
//! `Clone` and `Hash` (with the `Hasher` it writes into) exist without being
//! declared, with real methods, as bounds, and as
//! what `==`, `<` and `.clone()` need on a type parameter.
//!
//! What a type satisfies follows what the runtime does with that kind of
//! value (docs/TRAITS_DESIGN.md, "What S2 settled"): the tests pin that table,
//! the exact code, count and position of every diagnostic, and that a call
//! through a bound or directly on a value runs the right native method.
//! Hand-written impls of these traits are `TYPE-129` until slice S2b.

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

fn lines(source: &str) -> Vec<(usize, usize)> {
    check_source(source)
        .diagnostics
        .iter()
        .map(|d| (d.primary_span.line as usize, d.primary_span.column as usize))
        .collect()
}

fn run(source: &str) -> Result<String, String> {
    assert!(matches!(check_source(source).stage, Stage::Clean), "program should check clean");
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

/// A struct deriving all six traits, one deriving only `PartialEq`, one
/// deriving nothing, and a plain enum.
const TYPES: &str = r#"@derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash)
struct All { n: int }
@derive(PartialEq)
struct EqOnly { n: int }
struct Bare { n: int }
enum Color { Red, Green }
"#;

fn with_types(body: &str) -> String {
    format!("{TYPES}{body}")
}

/// The line (1-based) the first line of `body` lands on after `TYPES`.
fn body_line() -> usize {
    TYPES.lines().count() + 1
}

/// `fn need<T: TRAIT>(x: T) void {}` called with `arg`.
fn needs(trait_name: &str, arg: &str) -> String {
    with_types(&format!(
        "fn need<T: {trait_name}>(x: T) void {{ }}\nfn main() void {{ need({arg}) }}\n"
    ))
}

fn satisfied(trait_name: &str, arg: &str) -> bool {
    codes(&needs(trait_name, arg)) == (Stage::Clean, vec![])
}

fn rejected(trait_name: &str, arg: &str) -> bool {
    codes(&needs(trait_name, arg)) == (Stage::Sema, vec!["TYPE-126"])
}

// ── The five traits are usable without being declared ────────────

#[test]
fn the_six_prelude_traits_are_usable_as_bounds() {
    for name in ["PartialEq", "Eq", "PartialOrd", "Ord", "Clone", "Hash"] {
        let src = format!("fn a<T: {name}>(x: T) int {{ return 1 }}\nfn main() void {{ println(a(1)) }}\n");
        assert_eq!(codes(&src), (Stage::Clean, vec![]), "bound {name}");
    }
}

#[test]
fn ordering_is_an_enum_a_program_can_match_on() {
    let src = r#"fn name(o: Ordering) string {
    match o {
        Ordering.Less => { return "less" }
        Ordering.Equal => { return "equal" }
        Ordering.Greater => { return "greater" }
    }
}
fn main() void { println(name(Ordering.Equal)) }
"#;
    assert_eq!(run(src).unwrap(), "equal\n");
}

#[test]
fn a_program_with_no_prelude_use_reports_nothing_from_the_prelude() {
    let src = "fn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Clean, vec![]));
}

// ── Calls through a bound ────────────────────────────────────────

#[test]
fn ord_bound_orders_integers_strings_and_derived_structs() {
    let src = with_types(r#"fn largest<T: Ord>(a: T, b: T) T {
    if a > b { return a }
    return b
}
fn main() void {
    println(largest(3, 9))
    println(largest("pear", "apple"))
    println(largest(All { n = 1 }, All { n = 2 }).n)
}
"#);
    assert_eq!(run(&src).unwrap(), "9\npear\n2\n");
}

#[test]
fn cmp_through_a_bound_returns_the_ordering_enum() {
    let src = with_types(r#"fn order<T: Ord>(a: T, b: T) string {
    match a.cmp(b) {
        Ordering.Less => { return "less" }
        Ordering.Equal => { return "equal" }
        Ordering.Greater => { return "greater" }
    }
}
fn main() void {
    println(order(1, 2))
    println(order("b", "a"))
    println(order(All { n = 4 }, All { n = 4 }))
}
"#);
    assert_eq!(run(&src).unwrap(), "less\ngreater\nequal\n");
}

#[test]
fn eq_and_ne_through_a_bound() {
    let src = with_types(r#"fn same<T: PartialEq>(a: T, b: T) bool { return a.eq(b) and !a.ne(b) and a == b }
fn main() void {
    println(same(All { n = 1 }, All { n = 1 }))
    println(same(All { n = 1 }, All { n = 2 }))
    println(same(EqOnly { n = 3 }, EqOnly { n = 3 }))
}
"#);
    assert_eq!(run(&src).unwrap(), "true\nfalse\ntrue\n");
}

#[test]
fn clone_through_a_bound_copies_a_derived_struct() {
    let src = with_types(r#"fn dup<T: Clone>(x: T) T { return x.clone() }
fn main() void {
    let a = All { n = 7 }
    let b = dup(a)
    println(b.n)
}
"#);
    assert_eq!(run(&src).unwrap(), "7\n");
}

#[test]
fn the_ordering_methods_through_a_bound() {
    let src = r#"fn all<T: PartialOrd>(a: T, b: T) string {
    return $"{a.lt(b)} {a.le(b)} {a.gt(b)} {a.ge(b)}"
}
fn main() void {
    println(all(1, 2))
    println(all(2, 2))
    println(all(3, 2))
}
"#;
    assert_eq!(run(src).unwrap(), "true true false false\nfalse true false true\nfalse false true true\n");
}

#[test]
fn partial_cmp_through_a_bound_is_an_optional_ordering() {
    let src = r#"fn first<T: PartialOrd>(a: T, b: T) bool {
    let c = a.partial_cmp(b)
    return c == Ordering.Less
}
fn main() void {
    println(first(1.5, 2.5))
    println(first(2.5, 1.5))
}
"#;
    assert_eq!(run(src).unwrap(), "true\nfalse\n");
}

// ── Supertraits: a bound implies the traits under it ─────────────

#[test]
fn ord_implies_the_traits_beneath_it() {
    let src = r#"fn all<T: Ord>(a: T, b: T) bool {
    return a == b or a != b or a < b or a <= b or a.eq(b) or a.lt(b) or a.partial_cmp(b) == Ordering.Less
}
fn main() void { println(all(1, 2)) }
"#;
    assert_eq!(codes(src), (Stage::Clean, vec![]));
}

#[test]
fn eq_implies_partial_eq_and_partial_ord_implies_partial_eq() {
    let src = r#"fn e<T: Eq>(a: T, b: T) bool { return a == b and a.ne(b) }
fn p<T: PartialOrd>(a: T, b: T) bool { return a == b and a < b }
fn main() void { println(e(1, 2) or p(1, 2)) }
"#;
    assert_eq!(codes(src), (Stage::Clean, vec![]));
}

#[test]
fn eq_does_not_imply_ordering() {
    let src = "fn f<T: Eq>(a: T, b: T) bool { return a < b }\nfn main() void { println(f(1, 2)) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-131"]));
}

#[test]
fn partial_eq_alone_has_no_cmp() {
    let src = "fn f<T: PartialEq>(a: T, b: T) bool { return a.cmp(b) == Ordering.Less }\nfn main() void { println(f(1, 2)) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-104"]));
}

// ── What each kind of type satisfies ─────────────────────────────

#[test]
fn integers_and_strings_satisfy_all_six() {
    for arg in ["1", "\"s\""] {
        for t in ["PartialEq", "Eq", "PartialOrd", "Ord", "Clone", "Hash"] {
            assert!(satisfied(t, arg), "{arg}: {t}");
        }
    }
}

#[test]
fn sized_integers_satisfy_all_six() {
    let src = with_types(r#"fn need<T: Ord + Clone + Hash>(x: T) void { }
fn main() void {
    let a: u8 = 1u8
    let b: i64 = 2
    need(a)
    need(b)
}
"#);
    assert_eq!(codes(&src), (Stage::Clean, vec![]));
}

#[test]
fn floats_are_not_eq_or_ord_because_of_nan() {
    for arg in ["1.5", "2.5f"] {
        assert!(satisfied("PartialEq", arg), "{arg}: PartialEq");
        assert!(satisfied("PartialOrd", arg), "{arg}: PartialOrd");
        assert!(satisfied("Clone", arg), "{arg}: Clone");
        assert!(rejected("Eq", arg), "{arg}: Eq");
        assert!(rejected("Ord", arg), "{arg}: Ord");
        assert!(rejected("Hash", arg), "{arg}: Hash");
    }
}

#[test]
fn bool_and_char_are_eq_and_hash_but_not_ordered() {
    for arg in ["true", "'c'"] {
        assert!(satisfied("PartialEq", arg), "{arg}: PartialEq");
        assert!(satisfied("Eq", arg), "{arg}: Eq");
        assert!(satisfied("Hash", arg), "{arg}: Hash");
        assert!(satisfied("Clone", arg), "{arg}: Clone");
        assert!(rejected("PartialOrd", arg), "{arg}: PartialOrd");
        assert!(rejected("Ord", arg), "{arg}: Ord");
    }
}

#[test]
fn a_struct_satisfies_exactly_the_traits_it_derives() {
    assert!(satisfied("Ord", "All { n = 1 }"));
    assert!(satisfied("Clone", "All { n = 1 }"));
    assert!(satisfied("Hash", "All { n = 1 }"));
    assert!(satisfied("PartialEq", "EqOnly { n = 1 }"));
    assert!(rejected("Eq", "EqOnly { n = 1 }"));
    assert!(rejected("Clone", "EqOnly { n = 1 }"));
    assert!(rejected("Hash", "EqOnly { n = 1 }"));
    for t in ["PartialEq", "Eq", "PartialOrd", "Ord", "Clone", "Hash"] {
        assert!(rejected(t, "Bare { n = 1 }"), "Bare: {t}");
    }
}

#[test]
fn an_enum_is_partial_eq_eq_and_hash_only() {
    assert!(satisfied("PartialEq", "Color.Red"));
    assert!(satisfied("Eq", "Color.Red"));
    assert!(satisfied("Hash", "Color.Red"));
    assert!(rejected("Ord", "Color.Red"));
    assert!(rejected("PartialOrd", "Color.Red"));
    assert!(rejected("Clone", "Color.Red"));
}

#[test]
fn collections_satisfy_none_because_equality_on_them_is_identity() {
    for arg in ["[1, 2]", "List.new()"] {
        for t in ["PartialEq", "Ord", "Clone", "Hash"] {
            let src = format!("fn need<T: {t}>(x: T) void {{ }}\nfn main() void {{ need({arg}) }}\n");
            assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]), "{arg}: {t}");
        }
    }
}

/// `need` called with a tuple bound to a variable first (a tuple literal is
/// not parsed as a nested call argument).
fn tuple_needs(trait_name: &str, tuple: &str) -> (Stage, Vec<&'static str>) {
    codes(&with_types(&format!(
        "fn need<T: {trait_name}>(x: T) void {{ }}\nfn main() void {{\n    let t = {tuple}\n    need(t)\n}}\n"
    )))
}

#[test]
fn a_tuple_is_eq_and_clone_when_its_elements_are_and_never_ordered() {
    let clean = (Stage::Clean, vec![]);
    let rejected = (Stage::Sema, vec!["TYPE-126"]);
    assert_eq!(tuple_needs("PartialEq", "(1, \"a\")"), clean);
    assert_eq!(tuple_needs("Eq", "(1, \"a\")"), clean);
    assert_eq!(tuple_needs("Clone", "(1, \"a\")"), clean);
    assert_eq!(tuple_needs("Hash", "(1, \"a\")"), clean);
    assert_eq!(tuple_needs("Hash", "(1, 2.5)"), rejected);
    assert_eq!(tuple_needs("Ord", "(1, \"a\")"), rejected);
    assert_eq!(tuple_needs("PartialEq", "(1, 2.5)"), clean);
    assert_eq!(tuple_needs("Eq", "(1, 2.5)"), rejected);
}

#[test]
fn a_value_reached_through_an_ownership_wrapper_has_the_traits_of_the_value() {
    // `Unique.new` is only valid in LOW tier, and LOW may call only LOW.
    let src = with_types(r#"@tier(low)
fn need<T: Ord>(x: T) void { }
@tier(low)
fn go() void {
    let u = Unique.new(All { n = 1 })
    need(u)
}
fn main() void { go() }
"#);
    assert_eq!(codes(&src), (Stage::Clean, vec![]));
}

#[test]
fn an_unsatisfied_prelude_bound_is_reported_once_at_the_argument() {
    let src = with_types("fn need<T: Ord>(x: T) void { }\nfn main() void { need(2.5) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(lines(&src).len(), 1);
    assert_eq!(lines(&src)[0].0, body_line() + 1);
    assert!(messages(&src)[0].contains("Ord"), "got {:?}", messages(&src));
}

// ── Direct calls on a value ──────────────────────────────────────

#[test]
fn prelude_methods_can_be_called_directly_on_built_in_and_derived_values() {
    let src = with_types(r#"fn main() void {
    let five = 5
    let s = "a"
    let fl = 1.5
    let a = All { n = 1 }
    println(five.cmp(3) == Ordering.Greater)
    println(s.lt("b"))
    println(fl.partial_cmp(2.5) == Ordering.Less)
    println(a.eq(All { n = 1 }))
    println(a.clone().n)
    println(a.ne(All { n = 2 }))
}
"#);
    assert_eq!(run(&src).unwrap(), "true\ntrue\ntrue\ntrue\n1\ntrue\n");
}

#[test]
fn a_method_of_the_type_itself_wins_over_the_prelude_method() {
    let src = with_types(r#"extend All { fn eq(self, other: All) bool { return false } }
fn main() void {
    let a = All { n = 1 }
    println(a.eq(All { n = 1 }))
    println(a == All { n = 1 })
}
"#);
    assert_eq!(run(&src).unwrap(), "false\ntrue\n");
}

#[test]
fn a_prelude_method_on_a_type_without_the_trait_is_no_such_method() {
    let src = with_types("fn main() void {\n    let b = Bare { n = 1 }\n    println(b.cmp(b))\n}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-104"]));
}

#[test]
fn a_collection_has_no_prelude_methods() {
    let src = "fn main() void {\n    let l = [1, 2]\n    println(l.eq(l))\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-104"]));
}

#[test]
fn the_qualified_form_works_for_a_prelude_trait() {
    let src = with_types("fn main() void { println(Ord.cmp(1, 2) == Ordering.Less) }\n");
    assert_eq!(run(&src).unwrap(), "true\n");
}

// ── Operators on a type parameter ────────────────────────────────

#[test]
fn each_operator_on_an_unbounded_parameter_is_type_131_at_the_expression() {
    for (op, col) in [("==", 35), ("!=", 35), ("<", 35), ("<=", 35), (">", 35), (">=", 35)] {
        let src = format!("fn f<T>(a: T, b: T) bool {{ return a {op} b }}\nfn main() void {{ println(1) }}\n");
        assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-131"]), "operator {op}");
        assert_eq!(lines(&src), vec![(1, col)], "operator {op}");
    }
}

#[test]
fn type_131_names_the_operator_the_parameter_and_the_trait() {
    let src = "fn f<T: PartialEq>(a: T, b: T) bool { return a <= b }\nfn main() void { println(1) }\n";
    let msgs = messages(src);
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].contains("`<=`") && msgs[0].contains("`T`") && msgs[0].contains("`PartialOrd`"), "got {msgs:?}");
}

#[test]
fn operators_on_concrete_types_are_unchanged() {
    // `==` on a struct without PartialEq stays reference identity.
    let src = with_types(r#"fn main() void {
    let a = Bare { n = 1 }
    let b = Bare { n = 1 }
    println(a == b)
    println(a == a)
    println(1 < 2)
}
"#);
    assert_eq!(run(&src).unwrap(), "false\ntrue\ntrue\n");
}

#[test]
fn an_operator_inside_a_generic_struct_method_needs_the_declared_bound() {
    let src = r#"struct Pair<T> {
    a: T,
    b: T,
    fn same(self) bool { return self.a == self.b }
}
struct Pair2<T: PartialEq> {
    a: T,
    b: T,
    fn same(self) bool { return self.a == self.b }
}
fn main() void { println(1) }
"#;
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-131"]));
    assert_eq!(lines(src)[0].0, 4);
}

// ── Hand-written impls wait for S2b ──────────────────────────────

#[test]
fn a_hand_written_impl_of_each_prelude_trait_is_type_129() {
    for name in ["PartialEq", "Eq", "PartialOrd", "Ord", "Clone", "Hash"] {
        let src = format!("struct P {{ n: int }}\nimpl {name} for P {{ }}\nfn main() void {{ println(1) }}\n");
        assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-129"]), "impl {name}");
        assert_eq!(lines(&src)[0].0, 2, "impl {name}");
    }
}

#[test]
fn a_hand_written_impl_for_an_enum_is_type_129_too() {
    let src = "enum C { A }\nimpl PartialEq for C {\n    fn eq(self, other: C) bool { return true }\n}\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-129"]));
}

// ── Shadowing ────────────────────────────────────────────────────

#[test]
fn a_user_trait_with_a_prelude_name_shadows_it() {
    let src = r#"trait Ord { fn rank(self) int }
struct S { v: int }
impl Ord for S { fn rank(self) int { return self.v } }
fn top<T: Ord>(x: T) int { return x.rank() }
fn main() void { println(top(S { v = 5 })) }
"#;
    assert_eq!(run(src).unwrap(), "5\n");
}

#[test]
fn shadowing_a_prelude_trait_removes_the_traits_built_on_it() {
    // `Ord` is the user's, so `PartialOrd`'s dependents are not affected,
    // but a user `PartialEq` takes `Eq`, `PartialOrd` and `Ord` with it.
    let src = r#"trait PartialEq { fn same(self) bool }
fn f<T: Ord>(x: T) int { return 1 }
fn main() void { println(1) }
"#;
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-122"]));
}

#[test]
fn a_user_ordering_enum_is_the_users_and_the_ordering_traits_step_aside() {
    let src = r#"enum Ordering { Before, After }
fn main() void {
    let o = Ordering.After
    println(o == Ordering.After)
}
"#;
    assert_eq!(run(src).unwrap(), "true\n");
}

#[test]
fn a_user_ordering_enum_removes_ord_and_partial_ord_but_not_clone() {
    let src = r#"enum Ordering { Before, After }
fn a<T: Ord>(x: T) int { return 1 }
fn b<T: Clone>(x: T) int { return 1 }
fn main() void { println(1) }
"#;
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-122"]));
}

// ── Hash and the Hasher ──────────────────────────────────────────

/// `digest(x)` hashes one value into a fresh `Hasher`.
const DIGEST: &str = r#"fn digest<T: Hash>(x: T) u64 {
    let h = Hasher.new()
    x.hash(h)
    return h.finish()
}
"#;

#[test]
fn hashing_is_deterministic_and_separates_values() {
    let src = with_types(&format!("{DIGEST}fn main() void {{
    println(digest(5) == digest(5))
    println(digest(5) == digest(6))
    println(digest(\"a\") == digest(\"b\"))
    println(digest(All {{ n = 1 }}) == digest(All {{ n = 1 }}))
    println(digest(All {{ n = 1 }}) == digest(All {{ n = 2 }}))
    println(digest(Color.Red) == digest(Color.Green))
}}\n"));
    assert_eq!(run(&src).unwrap(), "true\nfalse\nfalse\ntrue\nfalse\nfalse\n");
}

#[test]
fn hashing_is_order_sensitive_and_accumulates_in_the_hasher() {
    let src = r#"fn main() void {
    let n = 7
    let s = "z"
    let h = Hasher.new()
    n.hash(h)
    s.hash(h)
    let g = Hasher.new()
    s.hash(g)
    n.hash(g)
    println(h.finish() == g.finish())
    println(h.finish() == h.finish())
    println(Hasher.new().finish() == Hasher.new().finish())
}
"#;
    assert_eq!(run(src).unwrap(), "false\ntrue\ntrue\n");
}

#[test]
fn a_tuple_and_a_tuple_of_the_same_values_hash_alike() {
    let src = format!("{DIGEST}fn main() void {{
    let a = (1, \"x\")
    let b = (1, \"x\")
    let c = (2, \"x\")
    println(digest(a) == digest(b))
    println(digest(a) == digest(c))
}}\n");
    assert_eq!(run(&src).unwrap(), "true\nfalse\n");
}

#[test]
fn hash_finish_is_a_u64() {
    let src = "fn main() void {\n    let v: u64 = Hasher.new().finish()\n    println(v == Hasher.new().finish())\n}\n";
    assert_eq!(run(src).unwrap(), "true\n");
}

#[test]
fn hash_without_a_hasher_argument_is_a_type_error() {
    let src = with_types("fn main() void {\n    let a = All { n = 1 }\n    a.hash(3)\n}\n");
    let (stage, c) = codes(&src);
    assert_eq!(stage, Stage::Sema);
    assert_eq!(c.len(), 1, "got {c:?}");
}

#[test]
fn hash_on_a_type_without_the_trait_is_no_such_method() {
    let src = with_types("fn main() void {\n    let b = Bare { n = 1 }\n    b.hash(Hasher.new())\n}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-104"]));
}

#[test]
fn hash_implies_eq_and_partial_eq_inside_a_body() {
    let src = r#"fn same<T: Hash>(a: T, b: T) bool { return a == b and a.eq(b) }
fn main() void { println(same(1, 1)) }
"#;
    assert_eq!(run(src).unwrap(), "true\n");
}

#[test]
fn a_derive_of_hash_without_eq_is_still_a_prerequisite_error() {
    let src = "@derive(Hash)\nstruct S { n: int }\nfn main() void { println(1) }\n";
    let (stage, c) = codes(src);
    assert_eq!(stage, Stage::Sema);
    assert!(c.contains(&"TYPE-117"), "got {c:?}");
}

#[test]
fn a_user_hasher_struct_takes_hash_with_it() {
    let src = r#"struct Hasher { n: int }
fn f<T: Hash>(x: T) int { return 1 }
fn main() void { println(1) }
"#;
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-122"]));
}

// ── Dictionary keys must be Hash ─────────────────────────────────

#[test]
fn dictionary_keys_of_hashable_types_are_accepted() {
    let src = with_types(r#"fn main() void {
    let a: Dictionary<string, int> = Dictionary.new()
    let b: Dictionary<int, int> = Dictionary.new()
    let c: Dictionary<bool, int> = Dictionary.new()
    let d: Dictionary<char, int> = Dictionary.new()
    let e: Dictionary<All, int> = Dictionary.new()
    let f: Dictionary<Color, int> = Dictionary.new()
    let u: Dictionary<u8, int> = Dictionary.new()
    println(1)
}
"#);
    assert_eq!(codes(&src), (Stage::Clean, vec![]));
}

#[test]
fn a_dictionary_annotation_with_a_bad_key_is_one_diagnostic_at_the_annotation() {
    let src = with_types("fn main() void {\n    let d: Dictionary<Bare, int> = Dictionary.new()\n    println(1)\n}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(lines(&src), vec![(body_line() + 1, 12)]);
    assert!(messages(&src)[0].contains("Hash") && messages(&src)[0].contains("Bare"), "got {:?}", messages(&src));
}

#[test]
fn a_float_key_is_rejected_because_of_nan() {
    let src = "fn main() void {\n    let d: Dictionary<float, int> = Dictionary.new()\n    println(1)\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-126"]));
}

#[test]
fn an_inferred_dictionary_checks_the_key_it_is_given() {
    let src = with_types(r#"fn main() void {
    let a = Dictionary.new()
    a.set(All { n = 1 }, 5)
    let b = Dictionary.new()
    b.set(Bare { n = 1 }, 5)
    let c = Dictionary.new()
    println(c.contains_key(Bare { n = 2 }))
    let d = Dictionary.new()
    println(d.get(2.5))
}
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126", "TYPE-126", "TYPE-126"]));
    let found = lines(&src);
    assert_eq!(found[0].0, body_line() + 4);
    assert_eq!(found[1].0, body_line() + 6);
    assert_eq!(found[2].0, body_line() + 8);
}

#[test]
fn an_unused_unannotated_dictionary_is_not_an_error() {
    let src = "fn main() void {\n    let d = Dictionary.new()\n    println(1)\n}\n";
    assert_eq!(codes(src), (Stage::Clean, vec![]));
}

#[test]
fn a_bad_key_named_twice_on_one_line_is_one_diagnostic_and_on_two_lines_is_two() {
    let one = with_types("fn main() void {\n    let d: Dictionary<Bare, int> = Dictionary.new()\n    println(1)\n}\n");
    assert_eq!(codes(&one), (Stage::Sema, vec!["TYPE-126"]));
    let two = with_types("fn main() void {\n    let d: Dictionary<Bare, int> =\n        Dictionary.new()\n    println(1)\n}\n");
    assert_eq!(codes(&two), (Stage::Sema, vec!["TYPE-126", "TYPE-126"]));
}

#[test]
fn a_dictionary_in_a_signature_is_checked() {
    let src = with_types("fn f(d: Dictionary<Bare, int>) void { }\nfn g(x: Dictionary<float, int>) void { }\nfn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126", "TYPE-126"]));
    assert_eq!(lines(&src), vec![(body_line(), 9), (body_line() + 1, 9)]);
}

#[test]
fn a_dictionary_nested_in_another_type_is_checked() {
    let src = "fn f(l: List<Dictionary<float, int>>) void { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-126"]));
}

#[test]
fn a_generic_key_needs_the_hash_bound() {
    let bad = "fn f<K>(d: Dictionary<K, int>) void { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(bad), (Stage::Sema, vec!["TYPE-126"]));
    let good = "fn f<K: Hash>(d: Dictionary<K, int>) void { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(good), (Stage::Clean, vec![]));
}

#[test]
fn a_dictionary_alias_is_checked_where_it_is_used_not_where_it_is_declared() {
    let src = r#"type Pairs<K, V> = Dictionary<K, V>
type Names = Dictionary<float, int>
fn main() void {
    let a: Pairs<string, int> = Dictionary.new()
    let b: Pairs<float, int> = Dictionary.new()
    let c: Names = Dictionary.new()
    println(1)
}
"#;
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-126", "TYPE-126"]));
    assert_eq!(lines(src), vec![(5, 12), (6, 12)]);
}

#[test]
fn a_hashable_dictionary_still_works_at_run_time() {
    let src = with_types(r#"fn main() void {
    let d: Dictionary<All, int> = Dictionary.new()
    d.set(All { n = 1 }, 10)
    d.set(All { n = 2 }, 20)
    println(d.get(All { n = 2 }))
    println(d.contains_key(All { n = 3 }))
}
"#);
    assert_eq!(run(&src).unwrap(), "20\nfalse\n");
}

// ── @derive still validates as before ────────────────────────────

#[test]
fn derive_prerequisites_are_still_enforced() {
    let src = "@derive(Ord)\nstruct S { n: int }\nfn main() void { println(1) }\n";
    let (stage, c) = codes(src);
    assert_eq!(stage, Stage::Sema);
    assert!(c.contains(&"TYPE-117"), "got {c:?}");
}
