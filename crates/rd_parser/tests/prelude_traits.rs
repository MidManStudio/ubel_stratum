//! The prelude traits, slice S2a: `PartialEq`, `Eq`, `PartialOrd`, `Ord`,
//! `Clone` and `Hash` (with the `Hasher` it writes into) exist without being
//! declared, with real methods, as bounds, and as
//! what `==`, `<` and `.clone()` need on a type parameter.
//!
//! What a type satisfies follows what the runtime does with that kind of
//! value (docs/TRAITS_DESIGN.md, "What S2 settled"): the tests pin that table,
//! the exact code, count and position of every diagnostic, and that a call
//! through a bound or directly on a value runs the right native method.
//! Hand-written impls of these traits (slice S2b) are in the last sections:
//! they are checked like any trait impl, and `==`, `<`, `contains`, dictionary
//! keys, `order_by` and the rest call them.

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

// ── Hand-written impls (slice S2b) ───────────────────────────────

/// `Money` compares, orders and hashes by whole dollars, so two amounts in
/// the same dollar are equal even though their cents differ.
const MONEY: &str = r#"struct Money { cents: int }

impl PartialEq for Money {
    fn eq(self, other: Money) bool { return self.cents / 100 == other.cents / 100 }
}

impl Eq for Money {
}

impl Hash for Money {
    fn hash(self, state: Hasher) void {
        let dollars = self.cents / 100
        dollars.hash(state)
    }
}

impl PartialOrd for Money {
    fn partial_cmp(self, other: Money) Ordering? {
        let a = self.cents / 100
        let b = other.cents / 100
        if a < b { return Ordering.Less }
        if a > b { return Ordering.Greater }
        return Ordering.Equal
    }
}

impl Ord for Money {
    fn cmp(self, other: Money) Ordering {
        let a = self.cents / 100
        let b = other.cents / 100
        if a < b { return Ordering.Less }
        if a > b { return Ordering.Greater }
        return Ordering.Equal
    }
}

impl Clone for Money {
    fn clone(self) Money { return Money { cents = self.cents + 1 } }
}
"#;

fn with_money(body: &str) -> String {
    format!("{MONEY}{body}")
}

fn money_line() -> usize {
    MONEY.lines().count() + 1
}

#[test]
fn a_hand_written_impl_of_every_prelude_trait_is_accepted() {
    let src = with_money("fn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Clean, vec![]));
}

#[test]
fn equals_and_not_equals_call_the_impls_eq() {
    let src = with_money(r#"fn main() void {
    let a = Money { cents = 150 }
    let b = Money { cents = 199 }
    let c = Money { cents = 250 }
    println(a == b)
    println(a != b)
    println(a == c)
    println(a != c)
}
"#);
    assert_eq!(run(&src).unwrap(), "true\nfalse\nfalse\ntrue\n");
}

#[test]
fn an_impls_own_ne_overrides_the_default() {
    let src = r#"struct Flag { on: bool }
impl PartialEq for Flag {
    fn eq(self, other: Flag) bool { return true }
    fn ne(self, other: Flag) bool { return true }
}
fn main() void {
    let a = Flag { on = true }
    let b = Flag { on = false }
    println(a == b)
    println(a != b)
}
"#;
    assert_eq!(run(src).unwrap(), "true\ntrue\n");
}

#[test]
fn the_ordering_operators_call_the_impl() {
    let src = with_money(r#"fn main() void {
    let a = Money { cents = 150 }
    let b = Money { cents = 199 }
    let c = Money { cents = 250 }
    println(a < c)
    println(a < b)
    println(a <= b)
    println(c > a)
    println(c >= c)
    println(a.lt(c))
    println(a.cmp(c) == Ordering.Less)
    println(c.partial_cmp(a) == Ordering.Greater)
}
"#);
    assert_eq!(run(&src).unwrap(), "true\nfalse\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\n");
}

#[test]
fn an_impls_own_lt_overrides_the_default() {
    let src = r#"struct N { v: int }
@derive(PartialEq)
struct M { v: int }
impl PartialEq for N {
    fn eq(self, other: N) bool { return self.v == other.v }
}
impl PartialOrd for N {
    fn partial_cmp(self, other: N) Ordering? { return Ordering.Equal }
    fn lt(self, other: N) bool { return true }
}
fn main() void {
    let a = N { v = 1 }
    let b = N { v = 1 }
    println(a < b)
    println(a <= b)
    println(a > b)
}
"#;
    assert_eq!(run(src).unwrap(), "true\ntrue\nfalse\n");
}

#[test]
fn a_call_through_a_bound_runs_the_impl() {
    let src = with_money(r#"fn same<T: PartialEq>(a: T, b: T) bool { return a == b and a.eq(b) }
fn biggest<T: Ord>(a: T, b: T) T {
    if a > b { return a }
    return b
}
fn dup<T: Clone>(x: T) T { return x.clone() }
fn main() void {
    let a = Money { cents = 150 }
    let b = Money { cents = 199 }
    let c = Money { cents = 250 }
    println(same(a, b))
    println(same(a, c))
    println(biggest(a, c).cents)
    println(dup(a).cents)
}
"#);
    assert_eq!(run(&src).unwrap(), "true\nfalse\n250\n151\n");
}

#[test]
fn hashing_goes_through_the_impls_hash() {
    let src = with_money(r#"fn digest<T: Hash>(x: T) u64 {
    let h = Hasher.new()
    x.hash(h)
    return h.finish()
}
fn main() void {
    let a = Money { cents = 150 }
    let b = Money { cents = 199 }
    let c = Money { cents = 250 }
    println(digest(a) == digest(b))
    println(digest(a) == digest(c))
}
"#);
    assert_eq!(run(&src).unwrap(), "true\nfalse\n");
}

#[test]
fn clone_calls_the_impls_clone_directly_on_a_value() {
    let src = with_money("fn main() void {\n    let a = Money { cents = 5 }\n    println(a.clone().cents)\n}\n");
    assert_eq!(run(&src).unwrap(), "6\n");
}

#[test]
fn contains_on_every_collection_uses_the_impls_eq() {
    let src = with_money(r#"fn main() void {
    let a = Money { cents = 150 }
    let b = Money { cents = 199 }
    let far = Money { cents = 900 }
    let l = [a]
    println(l.contains(b))
    println(l.contains(far))
    let q: Queue<Money> = Queue.new()
    q.enqueue(a)
    println(q.contains(b))
    let st: Stack<Money> = Stack.new()
    st.push(a)
    println(st.contains(b))
    println(st.contains(far))
}
"#);
    assert_eq!(run(&src).unwrap(), "true\nfalse\ntrue\ntrue\nfalse\n");
}

#[test]
fn dictionary_keys_are_found_by_the_impls_eq() {
    let src = with_money(r#"fn main() void {
    let a = Money { cents = 150 }
    let b = Money { cents = 199 }
    let c = Money { cents = 250 }
    let d: Dictionary<Money, string> = Dictionary.new()
    d.set(a, "one")
    println(d.get(b))
    println(d.contains_key(c))
    d.set(b, "again")
    println(d.len())
    println(d.get(a))
    d[c] = "three"
    println(d[c])
    d[a] = "replaced"
    println(d.len())
    println(d[b])
}
"#);
    assert_eq!(run(&src).unwrap(), "one\nfalse\n1\nagain\nthree\n2\nreplaced\n");
}

#[test]
fn order_by_and_group_by_use_the_impls() {
    let src = r#"@derive(PartialEq)
struct Job { name: string, rank: Rank }
struct Rank { n: int }
impl PartialEq for Rank {
    fn eq(self, other: Rank) bool { return self.n / 10 == other.n / 10 }
}
impl PartialOrd for Rank {
    fn partial_cmp(self, other: Rank) Ordering? {
        let a = other.n / 10
        let b = self.n / 10
        if a < b { return Ordering.Less }
        if a > b { return Ordering.Greater }
        return Ordering.Equal
    }
}
fn main() void {
    let jobs = [Job { name = "a", rank = Rank { n = 1 } }, Job { name = "b", rank = Rank { n = 95 } }, Job { name = "c", rank = Rank { n = 50 } }]
    let sorted = jobs.query().order_by(fn(j) j.rank).to_list()
    println(sorted[0].name)
    println(sorted[2].name)
    let groups = jobs.query().group_by(fn(j) j.rank)
    println(groups.len())
}
"#;
    assert_eq!(run(src).unwrap(), "b\na\n3\n");
}

#[test]
fn a_derived_struct_compares_a_field_by_that_fields_impl() {
    let src = with_money(r#"@derive(PartialEq, Eq, PartialOrd, Ord)
struct Wallet { m: Money, n: int }
fn main() void {
    let a = Wallet { m = Money { cents = 150 }, n = 1 }
    let b = Wallet { m = Money { cents = 199 }, n = 1 }
    let c = Wallet { m = Money { cents = 250 }, n = 1 }
    println(a == b)
    println(a == c)
    println(a < c)
    println(a < b)
    println(a <= b)
}
"#);
    assert_eq!(run(&src).unwrap(), "true\nfalse\ntrue\nfalse\ntrue\n");
}

#[test]
fn an_enum_may_order_itself_and_replace_its_automatic_eq() {
    let src = r#"enum Level { Low, Mid, High }
fn rank(l: Level) int {
    match l {
        Level.Low => { return 0 }
        Level.Mid => { return 1 }
        Level.High => { return 2 }
    }
}
impl PartialOrd for Level {
    fn partial_cmp(self, other: Level) Ordering? {
        let a = rank(self)
        let b = rank(other)
        if a < b { return Ordering.Less }
        if a > b { return Ordering.Greater }
        return Ordering.Equal
    }
}
impl PartialEq for Level {
    fn eq(self, other: Level) bool { return rank(self) / 2 == rank(other) / 2 }
}
fn main() void {
    println(Level.Low < Level.High)
    println(Level.High <= Level.Mid)
    println(Level.Mid.gt(Level.Low))
    println(Level.Mid == Level.Low)
    println(Level.High == Level.Low)
}
"#;
    assert_eq!(run(src).unwrap(), "true\nfalse\ntrue\ntrue\nfalse\n");
}

#[test]
fn an_enum_without_an_ordering_impl_still_cannot_be_ordered() {
    let src = "enum Level { Low, High }\nfn main() void { println(Level.Low < Level.High) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-118"]));
}

#[test]
fn a_user_type_with_an_impl_satisfies_the_bound_and_one_without_does_not() {
    let src = with_money(r#"struct Plain { n: int }
fn need<T: Ord>(x: T) void { }
fn main() void {
    need(Money { cents = 1 })
    need(Plain { n = 1 })
}
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(lines(&src)[0].0, money_line() + 4);
}

#[test]
fn a_derive_and_an_impl_of_the_same_trait_overlap() {
    let src = "@derive(PartialEq)\nstruct A { n: int }\nimpl PartialEq for A {\n    fn eq(self, other: A) bool { return true }\n}\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-128"]));
    assert_eq!(lines(src), vec![(3, 1)]);
}

#[test]
fn an_impl_needs_the_traits_it_builds_on_and_reports_the_first_missing() {
    let cases = [
        ("Ord", "fn cmp(self, other: B) Ordering { return Ordering.Equal }", "PartialOrd"),
        ("PartialOrd", "fn partial_cmp(self, other: B) Ordering? { return Ordering.Equal }", "PartialEq"),
        ("Hash", "fn hash(self, state: Hasher) void { }", "Eq"),
    ];
    for (name, method, missing) in cases {
        let src = format!("struct B {{ n: int }}\nimpl {name} for B {{\n    {method}\n}}\nfn main() void {{ println(1) }}\n");
        assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-132"]), "impl {name}");
        assert!(messages(&src)[0].contains(missing), "impl {name}: {:?}", messages(&src));
        assert_eq!(lines(&src), vec![(2, 1)], "impl {name}");
    }
}

#[test]
fn eq_needs_partial_eq() {
    let src = "struct B { n: int }\nimpl Eq for B {\n}\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-132"]));
}

#[test]
fn an_impl_missing_a_required_method_is_type_123() {
    let src = "struct B { n: int }\nimpl PartialEq for B {\n}\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-123"]));
}

#[test]
fn an_impl_with_the_wrong_signature_is_type_125() {
    let src = "struct B { n: int }\nimpl PartialEq for B {\n    fn eq(self, other: int) bool { return true }\n}\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-125"]));
}

#[test]
fn a_prelude_impl_for_a_built_in_type_that_has_it_natively_is_type_128() {
    // `int` is `PartialEq` already; tests/builtin_impls.rs covers the rest.
    let src = "impl PartialEq for int {\n    fn eq(self, other: int) bool { return true }\n}\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-128"]));
}

#[test]
fn types_without_impls_behave_as_before() {
    // `==` on a struct with no PartialEq is still reference identity, and a
    // derived comparison still goes field by field.
    let src = with_types(r#"fn main() void {
    let a = Bare { n = 1 }
    let b = Bare { n = 1 }
    println(a == b)
    println(a == a)
    println(All { n = 1 } == All { n = 1 })
    println(All { n = 1 } < All { n = 2 })
}
"#);
    assert_eq!(run(&src).unwrap(), "false\ntrue\ntrue\ntrue\n");
}

// ── Nested positions: enum payloads, derived Hash, derived Clone ──

/// An enum with `Money` payloads, a struct deriving `Hash` and `Clone` over a
/// `Money` field, one holding every kind of collection of `Money`, and the
/// helpers the tests below call. `Money` clones with one extra cent and
/// hashes and compares by whole dollars, so each result shows which code ran.
const NESTED: &str = r#"enum Pay {
    Cash(Money),
    Card(Money),
    Check { amount: Money },
    Nothing
}

@derive(PartialEq, Eq, Hash, Clone)
struct Wallet {
    amount: Money,
    note: string,
}

@derive(Clone)
struct Holder {
    items: List<Money>,
    ledger: Dictionary<string, Money>,
    line: Queue<Money>,
    pile: Stack<Money>,
    pay: Pay,
    cash: Pay,
}

@derive(PartialEq, Eq, Hash, Clone)
struct Plain {
    n: int,
    s: string,
}

fn digest<T: Hash>(x: T) u64 {
    let h = Hasher.new()
    x.hash(h)
    return h.finish()
}

fn dup<T: Clone>(x: T) T {
    return x.clone()
}

fn cents_of(p: Pay) int {
    match p {
        Cash(m) => return m.cents,
        Card(m) => return m.cents,
        Check { amount } => return amount.cents,
        Nothing => return 0,
    }
}
"#;

fn with_nested(body: &str) -> String {
    with_money(&format!("{NESTED}{body}"))
}

#[test]
fn an_enum_payload_is_compared_through_its_types_eq() {
    let src = with_nested(r#"fn main() void {
    let a = Pay.Cash(Money { cents = 150 })
    let b = Pay.Cash(Money { cents = 199 })
    let c = Pay.Cash(Money { cents = 250 })
    println(a == b)
    println(a == c)
    println(a != c)
    println(Pay.Nothing == Pay.Nothing)
    println(a == Pay.Nothing)
    let x = Pay.Check { amount = Money { cents = 101 } }
    let y = Pay.Check { amount = Money { cents = 150 } }
    println(x == y)
    println(a == x)
    println(a == Pay.Card(Money { cents = 150 }))
    println(x == Pay.Check { amount = Money { cents = 250 } })
}
"#);
    // The last two: the same payload under another variant is not equal, and
    // a struct payload that differs by a whole dollar is not equal.
    assert_eq!(run(&src).unwrap(), "true\nfalse\ntrue\ntrue\nfalse\ntrue\nfalse\nfalse\nfalse\n");
}

#[test]
fn an_enum_is_found_by_its_payloads_eq_in_contains_and_dictionaries() {
    let src = with_nested(r#"fn main() void {
    let l = [Pay.Cash(Money { cents = 150 })]
    println(l.contains(Pay.Cash(Money { cents = 199 })))
    println(l.contains(Pay.Cash(Money { cents = 250 })))
    let d: Dictionary<Pay, string> = Dictionary.new()
    d.set(Pay.Cash(Money { cents = 150 }), "one")
    d.set(Pay.Cash(Money { cents = 199 }), "uno")
    d.set(Pay.Nothing, "none")
    println(d.get(Pay.Cash(Money { cents = 101 })))
    println(d.contains_key(Pay.Cash(Money { cents = 250 })))
    println(d.len())
}
"#);
    assert_eq!(run(&src).unwrap(), "true\nfalse\nuno\nfalse\n2\n");
}

#[test]
fn derived_clone_calls_a_fields_clone_directly_and_through_a_bound() {
    let src = with_nested(r#"fn main() void {
    let w = Wallet { amount = Money { cents = 10 }, note = "n" }
    println(w.clone().amount.cents)
    println(dup(w).amount.cents)
    println(w.amount.cents)
    let t = (Money { cents = 199 }, 1)
    println(digest(dup(t)) == digest(t))
}
"#);
    // The last line: the tuple's `Money` was cloned (199 became 200, a
    // different dollar), so the two digests differ.
    assert_eq!(run(&src).unwrap(), "11\n11\n10\nfalse\n");
}

#[test]
fn derived_clone_reaches_every_collection_and_an_enum_payload() {
    let src = with_nested(r#"fn main() void {
    let items: List<Money> = List.new()
    items.push(Money { cents = 1 })
    let ledger: Dictionary<string, Money> = Dictionary.new()
    ledger.set("k", Money { cents = 7 })
    let line: Queue<Money> = Queue.new()
    line.enqueue(Money { cents = 20 })
    let pile: Stack<Money> = Stack.new()
    pile.push(Money { cents = 30 })
    let h = Holder { items = items, ledger = ledger, line = line, pile = pile, pay = Pay.Check { amount = Money { cents = 40 } }, cash = Pay.Cash(Money { cents = 50 }) }
    let k = h.clone()
    println(k.items.get(0).cents)
    println(k.ledger.get("k").cents)
    println(k.line.dequeue().cents)
    println(k.pile.pop().cents)
    println(cents_of(k.pay))
    println(cents_of(k.cash))
    println(h.items.get(0).cents)
    println(h.line.dequeue().cents)
}
"#);
    assert_eq!(run(&src).unwrap(), "2\n8\n21\n31\n41\n51\n1\n20\n");
}

#[test]
fn derived_hash_agrees_with_eq_through_fields_enum_payloads_and_tuples() {
    let src = with_nested(r#"fn main() void {
    let a = Wallet { amount = Money { cents = 150 }, note = "n" }
    let b = Wallet { amount = Money { cents = 199 }, note = "n" }
    let c = Wallet { amount = Money { cents = 250 }, note = "n" }
    let d = Wallet { amount = Money { cents = 150 }, note = "m" }
    println(a == b)
    println(digest(a) == digest(b))
    println(digest(a) == digest(c))
    println(digest(a) == digest(d))
    let p = Pay.Cash(Money { cents = 150 })
    let q = Pay.Cash(Money { cents = 199 })
    let r = Pay.Cash(Money { cents = 250 })
    println(digest(p) == digest(q))
    println(digest(p) == digest(r))
    let x = Pay.Check { amount = Money { cents = 101 } }
    let y = Pay.Check { amount = Money { cents = 150 } }
    println(digest(x) == digest(y))
    println(digest(p) == digest(x))
    let t1 = (Money { cents = 150 }, 1)
    let t2 = (Money { cents = 199 }, 1)
    let t3 = (Money { cents = 250 }, 1)
    println(digest(t1) == digest(t2))
    println(digest(t1) == digest(t3))
}
"#);
    assert_eq!(
        run(&src).unwrap(),
        "true\ntrue\nfalse\nfalse\ntrue\nfalse\ntrue\nfalse\ntrue\nfalse\n"
    );
}

#[test]
fn types_without_impls_hash_and_clone_as_before() {
    let src = with_nested(r#"fn main() void {
    let a = Plain { n = 1, s = "x" }
    let b = Plain { n = 1, s = "x" }
    let c = Plain { n = 2, s = "x" }
    println(digest(a) == digest(b))
    println(digest(a) == digest(c))
    println(a.clone() == a)
    println(dup(c) == c)
}
"#);
    assert_eq!(run(&src).unwrap(), "true\nfalse\ntrue\ntrue\n");
}

#[test]
fn a_unique_field_is_compared_hashed_and_cloned_through_the_impl() {
    let src = with_money(r#"@derive(PartialEq, Eq, Hash, Clone)
struct Slot { m: Unique<Money> }

@tier(low)
fn digest<T: Hash>(x: T) u64 {
    let h = Hasher.new()
    x.hash(h)
    return h.finish()
}

@tier(low)
fn main() void {
    let a = Slot { m = Unique.new(Money { cents = 150 }) }
    let b = Slot { m = Unique.new(Money { cents = 199 }) }
    let c = Slot { m = Unique.new(Money { cents = 250 }) }
    println(a == b)
    println(a == c)
    println(digest(a) == digest(b))
    println(digest(a) == digest(c))
    let d = Slot { m = Unique.new(Money { cents = 199 }) }
    let e = d.clone()
    println(digest(e) == digest(d))
}
"#);
    assert_eq!(run(&src).unwrap(), "true\nfalse\ntrue\nfalse\nfalse\n");
}

// ── Closing the gaps the S2b mutation pass found ──────────────────

#[test]
fn a_tuple_is_compared_element_by_element_through_the_impls_eq() {
    let src = with_money(r#"fn main() void {
    let t1 = (Money { cents = 150 }, Money { cents = 1 })
    let t2 = (Money { cents = 199 }, Money { cents = 50 })
    let t3 = (Money { cents = 250 }, Money { cents = 1 })
    let l = [t1]
    println(l.contains(t2))
    println(l.contains(t3))
}
"#);
    assert_eq!(run(&src).unwrap(), "true\nfalse\n");
}

#[test]
fn a_derived_ordering_moves_on_to_the_next_field_when_one_compares_equal() {
    let src = with_money(r#"@derive(PartialEq, Eq, PartialOrd, Ord)
struct Bid { who: Money, n: int }

fn main() void {
    let a = Bid { who = Money { cents = 150 }, n = 1 }
    let b = Bid { who = Money { cents = 199 }, n = 2 }
    let c = Bid { who = Money { cents = 250 }, n = 0 }
    let d = Bid { who = Money { cents = 199 }, n = 1 }
    println(a < b)
    println(b < c)
    println(c < a)
    println(a.lt(b))
    println(a.eq(b))
    println(a.eq(d))
    println(a.cmp(b) == Ordering.Less)
    println(c.cmp(a) == Ordering.Greater)
    println(a.partial_cmp(c) == Ordering.Less)
}
"#);
    // `a.eq(d)`: the same dollar and the same `n`, so equal by the field's impl.
    assert_eq!(run(&src).unwrap(), "true\ntrue\nfalse\ntrue\nfalse\ntrue\ntrue\ntrue\ntrue\n");
}

#[test]
fn the_comparison_methods_are_false_when_a_field_has_no_order() {
    let src = r#"struct Odd { n: int }

impl PartialEq for Odd {
    fn eq(self, other: Odd) bool { return true }
}

impl PartialOrd for Odd {
    fn partial_cmp(self, other: Odd) Ordering? { return null }
}

@derive(PartialEq, PartialOrd)
struct Row { o: Odd }

fn main() void {
    let a = Row { o = Odd { n = 1 } }
    let b = Row { o = Odd { n = 2 } }
    println(a.lt(b))
    println(a.le(b))
    println(a.gt(b))
    println(a.ge(b))
}
"#;
    assert_eq!(run(src).unwrap(), "false\nfalse\nfalse\nfalse\n");
}

#[test]
fn a_unique_operand_and_a_derived_ordering_over_unique_fields_use_the_impl() {
    let src = with_money(r#"@derive(PartialEq, Eq, PartialOrd, Ord)
struct Slot { m: Unique<Money> }

@tier(low)
fn main() void {
    let u = Unique.new(Money { cents = 150 })
    let v = Unique.new(Money { cents = 199 })
    println(u == v)
    let a = Slot { m = Unique.new(Money { cents = 150 }) }
    let c = Slot { m = Unique.new(Money { cents = 250 }) }
    println(a < c)
    println(c < a)
}
"#);
    assert_eq!(run(&src).unwrap(), "true\ntrue\nfalse\n");
}

#[test]
fn assigning_through_a_dictionary_index_finds_the_key_by_the_impls_eq() {
    let src = with_money(r#"fn main() void {
    let d: Dictionary<Money, int> = Dictionary.new()
    d.set(Money { cents = 150 }, 1)
    d[Money { cents = 199 }] = 5
    println(d.len())
    println(d[Money { cents = 101 }])
}
"#);
    assert_eq!(run(&src).unwrap(), "1\n5\n");
}

#[test]
fn group_by_merges_keys_that_are_equal_by_the_impl() {
    let src = with_money(r#"fn main() void {
    let ms = [Money { cents = 150 }, Money { cents = 199 }, Money { cents = 250 }]
    let g = ms.query().group_by(fn(m) m)
    println(g.len())
}
"#);
    assert_eq!(run(&src).unwrap(), "2\n");
}

#[test]
fn contains_on_an_inline_list_uses_the_impls_eq() {
    let src = with_money(r#"@tier(low)
fn main() void {
    let v = InlineList.new(3)
    v.push(Money { cents = 150 })
    println(v.contains(Money { cents = 199 }))
    println(v.contains(Money { cents = 500 }))
}
"#);
    assert_eq!(run(&src).unwrap(), "true\nfalse\n");
}

#[test]
fn clone_called_where_the_receivers_type_is_not_yet_known_still_uses_the_impl() {
    let src = with_money(r#"@derive(Clone)
struct Wallet { amount: Money }

fn main() void {
    let ws = [Wallet { amount = Money { cents = 10 } }]
    let cs = ws.query().select(fn(w) w.clone()).to_list()
    println(cs.get(0).amount.cents)
}
"#);
    assert_eq!(run(&src).unwrap(), "11\n");
}

#[test]
fn an_impl_written_after_the_signature_that_needs_it_is_still_found() {
    // The signatures are collected before the impls are registered, so a
    // dictionary key or a bounded struct argument naming a type whose impl
    // comes later in the file must wait for the impls rather than fail.
    let dict = r#"struct Key { n: int }

fn count(d: Dictionary<Key, int>) int { return d.len() }

impl PartialEq for Key {
    fn eq(self, other: Key) bool { return self.n == other.n }
}

impl Eq for Key {
}

impl Hash for Key {
    fn hash(self, state: Hasher) void { self.n.hash(state) }
}

fn main() void { println(1) }
"#;
    assert_eq!(codes(dict), (Stage::Clean, vec![]));

    let bound = r#"trait Shape {
    fn area(self) int
}

struct Boxed<T: Shape> { inner: T }

struct Circle { r: int }

fn make(b: Boxed<Circle>) int { return 1 }

impl Shape for Circle {
    fn area(self) int { return self.r }
}

fn main() void { println(1) }
"#;
    assert_eq!(codes(bound), (Stage::Clean, vec![]));
}
