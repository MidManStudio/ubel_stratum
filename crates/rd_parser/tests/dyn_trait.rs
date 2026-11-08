//! `dyn Trait`, slice S6.
//!
//! `dyn Shape` is a type for a value of any type that implements `Shape`, the
//! type being known only at run time. It may be written in every tier; the
//! tier wrapper comes from where the value is built, like any other type, so
//! `&dyn Shape` and `Unique<dyn Shape>` are what `LOW` code writes. The trait
//! has to be object safe by Rust's rule (`TYPE-138`): every method takes
//! `self`, has no generic parameters of its own and does not mention `Self`
//! outside the receiver, in the trait and in each trait it builds on.
//!
//! A value becomes a `dyn` value by coercion at a typed site: a `let` with a
//! written type, an argument, a return, a struct field, an element handed to
//! a collection, an array literal on the right of a `let`, and `x as dyn T`.
//! A call, an operator, an index and a `for` on a `dyn` value go through the
//! trait as through a bound, at the tier the trait declares.
//!
//! See docs/TRAITS_DESIGN.md, "What S6 settled".

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

const SHAPES: &str = r#"
trait Shape {
    fn area(self) int
    fn name(self) string
}

struct Circle { r: int }
struct Square { side: int }
struct Rock { w: int }

impl Shape for Circle {
    fn area(self) int { return 3 * self.r * self.r }
    fn name(self) string { return "circle" }
}

impl Shape for Square {
    fn area(self) int { return self.side * self.side }
    fn name(self) string { return "square" }
}
"#;

fn with_shapes(body: &str) -> String {
    format!("{SHAPES}\nfn main() void {{\n{body}\n}}\n")
}

fn with_items(items: &str, body: &str) -> String {
    format!("{SHAPES}\n{items}\nfn main() void {{\n{body}\n}}\n")
}

// ── Run time: a list of `dyn` holds different types ─────────────

#[test]
fn a_list_of_dyn_holds_different_types() {
    let src = with_shapes("    let shapes: List<dyn Shape> = List.new()\n    shapes.push(Circle { r = 2 })\n    shapes.push(Square { side = 3 })\n    for s in shapes {\n        println(s.name())\n        println(s.area())\n    }");
    assert_eq!(out(&src), "circle\n12\nsquare\n9\n");
}

#[test]
fn an_array_literal_is_coerced_into_a_list_of_dyn() {
    let src = with_shapes("    let shapes: List<dyn Shape> = [Circle { r = 1 }, Square { side = 2 }]\n    for s in shapes { println(s.area()) }");
    assert_eq!(out(&src), "3\n4\n");
}

#[test]
fn a_single_value_is_coerced_by_a_written_type() {
    let src = with_shapes("    let one: dyn Shape = Square { side = 4 }\n    println(one.area())");
    assert_eq!(out(&src), "16\n");
}

#[test]
fn a_dyn_parameter_and_a_dyn_return() {
    let items = r#"
fn total(shapes: List<dyn Shape>) int {
    let mut t = 0
    for s in shapes { t = t + s.area() }
    return t
}

fn make(k: int) dyn Shape {
    if k == 0 { return Circle { r = 1 } }
    return Square { side = 2 }
}
"#;
    let src = with_items(items, "    let l: List<dyn Shape> = [make(0), make(1)]\n    println(total(l))\n    println(make(0).area())");
    assert_eq!(out(&src), "7\n3\n");
}

#[test]
fn an_argument_is_coerced_to_a_dyn_parameter() {
    let items = "fn show(s: dyn Shape) int { return s.area() }\n";
    let src = with_items(items, "    println(show(Circle { r = 2 }))\n    println(show(Square { side = 5 }))");
    assert_eq!(out(&src), "12\n25\n");
}

#[test]
fn a_struct_field_may_be_dyn() {
    let items = "struct Holder { s: dyn Shape }\n";
    let src = with_items(items, "    let h = Holder { s = Circle { r = 2 } }\n    println(h.s.area())");
    assert_eq!(out(&src), "12\n");
}

#[test]
fn a_dictionary_queue_and_stack_of_dyn() {
    let src = with_shapes("    let d: Dictionary<string, dyn Shape> = Dictionary.new()\n    d.set(\"c\", Circle { r = 3 })\n    println(d.get(\"c\").area())\n    let q: Queue<dyn Shape> = Queue.new()\n    q.enqueue(Circle { r = 2 })\n    println(q.dequeue().area())\n    let s: Stack<dyn Shape> = Stack.new()\n    s.push(Square { side = 3 })\n    println(s.pop().area())");
    assert_eq!(out(&src), "27\n12\n9\n");
}

#[test]
fn an_optional_dyn() {
    let src = with_shapes("    let o: dyn Shape? = Square { side = 5 }\n    println(o.area())");
    assert_eq!(out(&src), "25\n");
}

#[test]
fn as_dyn_is_a_written_coercion() {
    let src = with_shapes("    let c = Circle { r = 4 } as dyn Shape\n    println(c.area())");
    assert_eq!(out(&src), "48\n");
}

#[test]
fn a_default_method_runs_through_dyn() {
    let src = r#"
trait Shape {
    fn area(self) int
    fn double_area(self) int { return self.area() * 2 }
}
struct Circle { r: int }
impl Shape for Circle { fn area(self) int { return self.r } }
fn main() void {
    let s: dyn Shape = Circle { r = 5 }
    println(s.double_area())
}
"#;
    assert_eq!(out(src), "10\n");
}

#[test]
fn dyn_stays_usable_as_a_name() {
    let src = "fn keep(dyn: int) int { return dyn + 1 }\nfn main() void {\n    let dyn = 5\n    println(keep(dyn))\n}\n";
    assert_eq!(out(src), "6\n");
}

#[test]
fn a_collection_of_wrapped_dyn_values_is_written_like_any_other() {
    let src = with_shapes("    let q: Queue<dyn Shape> = Queue.new()\n    q.enqueue(Square { side = 6 })\n    println(q.dequeue().name())");
    assert_eq!(out(&src), "square\n");
}

#[test]
fn equality_of_dyn_values_is_identity_as_for_any_struct() {
    let src = with_shapes("    let a: dyn Shape = Circle { r = 1 }\n    let b: dyn Shape = Circle { r = 1 }\n    println(a == a)\n    println(a == b)");
    assert_eq!(out(&src), "true\nfalse\n");
}

// ── Supertraits, generic traits and bounds ──────────────────────

const PETS: &str = r#"
trait Named { fn name(self) string }
trait Pet: Named { fn legs(self) int }
trait Convert<T> { fn conv(self) T }

struct Dog { n: string }
struct Bird { n: string }

impl Named for Dog { fn name(self) string { return self.n } }
impl Pet for Dog { fn legs(self) int { return 4 } }
impl Convert<int> for Dog { fn conv(self) int { return 40 } }

impl Named for Bird { fn name(self) string { return self.n } }
impl Pet for Bird { fn legs(self) int { return 2 } }
impl Convert<int> for Bird { fn conv(self) int { return 20 } }

fn describe<T: Named>(x: T) string { return x.name() }
"#;

fn with_pets(body: &str) -> String {
    format!("{PETS}\nfn main() void {{\n{body}\n}}\n")
}

#[test]
fn a_supertrait_method_is_called_through_the_sub_trait() {
    let src = with_pets("    let p: dyn Pet = Dog { n = \"rex\" }\n    println(p.name())\n    println(p.legs())");
    assert_eq!(out(&src), "rex\n4\n");
}

#[test]
fn a_dyn_satisfies_a_bound_of_its_trait_and_of_its_supertraits() {
    let src = with_pets("    let p: dyn Pet = Bird { n = \"tweet\" }\n    println(describe(p))");
    assert_eq!(out(&src), "tweet\n");
}

#[test]
fn a_dyn_of_a_generic_trait_fixes_its_arguments() {
    let src = with_pets("    let cs: List<dyn Convert<int>> = List.new()\n    cs.push(Dog { n = \"a\" })\n    cs.push(Bird { n = \"b\" })\n    let mut t = 0\n    for c in cs { t = t + c.conv() }\n    println(t)");
    assert_eq!(out(&src), "60\n");
}

#[test]
fn a_generic_struct_may_hold_a_dyn_of_its_parameter() {
    let items = "struct Bag<T> { items: List<dyn Convert<T>> }\n";
    let src = format!("{PETS}\n{items}\nfn main() void {{\n    let b: Bag<int> = Bag {{ items = List.new() }}\n    b.items.push(Dog {{ n = \"x\" }})\n    println(b.items.len())\n    println(b.items[0].conv())\n}}\n");
    assert_eq!(out(&src), "1\n40\n");
}

#[test]
fn a_built_in_type_may_be_behind_dyn() {
    let src = r#"
trait Show { fn show(self) string }
impl Show for int { fn show(self) string { return "int!" } }
impl Show for string { fn show(self) string { return "str!" } }
fn main() void {
    let a: dyn Show = 5
    let b: dyn Show = "hi"
    println(a.show())
    println(b.show())
    let d: dyn Display = 7
    println(d.to_string())
}
"#;
    assert_eq!(out(src), "int!\nstr!\n7\n");
}

// ── Operators, index, `for` and `Display` through dyn ───────────

#[test]
fn operators_index_for_and_display_work_on_a_dyn_of_their_trait() {
    let src = r#"
struct V { x: int }
impl Add<V, V> for V { fn add(self, rhs: V) V { return V { x = self.x + rhs.x } } }
impl Index<int, int> for V { fn index(self, idx: int) int { return self.x * 10 + idx } }
impl Display for V { fn to_string(self) string { return "V" } }

struct Counter { n: int, max: int }
impl Iterator<int> for Counter {
    fn next(self) int? {
        if self.n >= self.max { return null }
        self.n = self.n + 1
        return self.n * 10
    }
}

fn main() void {
    let a: dyn Add<V, V> = V { x = 5 }
    let sum = a + V { x = 7 }
    println(sum.x)
    let i: dyn Index<int, int> = V { x = 3 }
    println(i[4])
    let it: dyn Iterator<int> = Counter { n = 0, max = 3 }
    for v in it { println(v) }
    let d: dyn Display = V { x = 1 }
    println(d.to_string())
    println(d)
}
"#;
    assert_eq!(out(src), "12\n34\n10\n20\n30\nV\nV\n");
}

#[test]
fn the_wrong_right_operand_of_a_dyn_operator_is_type_136() {
    let src = r#"
struct V { x: int }
impl Add<V, V> for V { fn add(self, rhs: V) V { return V { x = self.x + rhs.x } } }
fn main() void {
    let a: dyn Add<V, V> = V { x = 5 }
    let s = a + 3
}
"#;
    one_sema(src, "TYPE-136");
}

#[test]
fn a_for_over_a_dyn_without_iterator_is_type_135() {
    let src = with_shapes("    let a: dyn Shape = Circle { r = 1 }\n    for x in a { println(x) }");
    one_sema(&src, "TYPE-135");
}

#[test]
fn ordering_a_dyn_is_type_118() {
    let src = with_shapes("    let a: dyn Shape = Circle { r = 1 }\n    println(a < a)");
    one_sema(&src, "TYPE-118");
}

// ── Aliases and declaration order ───────────────────────────────

#[test]
fn an_alias_of_a_list_of_dyn_and_a_trait_declared_later() {
    let src = r#"
type Shapes = List<dyn Shape>

fn total(xs: Shapes) int {
    let mut t = 0
    for s in xs { t = t + s.area() }
    return t
}

struct Circle { r: int }
impl Shape for Circle { fn area(self) int { return self.r } }

trait Shape {
    fn area(self) int
    fn double_area(self) int { return self.area() * 2 }
}

fn main() void {
    let xs: Shapes = [Circle { r = 2 }, Circle { r = 5 }]
    println(total(xs))
    for s in xs { println(s.double_area()) }
}
"#;
    assert_eq!(out(src), "7\n4\n10\n");
}

// ── Tiers ───────────────────────────────────────────────────────

const BODY: &str = r#"
@tier(low)
trait Body { fn mass(self) int }

@tier(low) {
    struct Rock { m: int }
    impl Body for Rock {
        fn mass(self) int { return self.m }
    }
}

@tier(low)
fn weigh(b: &dyn Body) int { return b.mass() }

@tier(low)
fn own(b: Unique<dyn Body>) int { return b.mass() }

@tier(low)
fn low_unique() int {
    let u: Unique<dyn Body> = Unique.new(Rock { m = 2 })
    return own(u)
}

@tier(mid)
fn in_arena() int {
    with arena(256) {
        let r = Rock { m = 3 }
        return weigh(&r)
    }
}
"#;

#[test]
fn dyn_is_written_in_low_and_mid_behind_the_tier_wrapper() {
    let src = format!("{BODY}\nfn main() void {{\n    let r = Rock {{ m = 7 }}\n    println(weigh(&r))\n    println(low_unique())\n    println(in_arena())\n}}\n");
    assert_eq!(out(&src), "7\n2\n3\n");
}

#[test]
fn a_low_caller_cannot_reach_a_mid_trait_method_through_dyn() {
    let src = r#"
@tier(mid)
trait Shape { fn area(self) int }
@tier(mid)
struct Circle { r: int }
impl Shape for Circle { fn area(self) int { return self.r } }
@tier(low)
fn peek(s: &dyn Shape) int { return s.area() }
fn main() void {
    let c = Circle { r = 1 }
    println(peek(&c))
}
"#;
    one_sema(src, "TIER-005");
}

#[test]
fn a_low_caller_cannot_reach_a_high_trait_method_through_dyn() {
    let src = r#"
trait Shape { fn area(self) int }
struct Circle { r: int }
impl Shape for Circle { fn area(self) int { return self.r } }
@tier(low)
fn peek(s: &dyn Shape) int { return s.area() }
fn main() void {
    let c = Circle { r = 1 }
    println(peek(&c))
}
"#;
    one_sema(src, "TIER-005");
}

#[test]
fn a_coercion_does_not_add_a_wrapper() {
    let src = with_shapes("    let c = Circle { r = 1 }\n    let u: Unique<dyn Shape> = c");
    one_sema(&src, "TYPE-101");
}

// ── Object safety, TYPE-138 ─────────────────────────────────────

#[test]
fn a_method_without_self_is_not_object_safe() {
    let src = "trait Animal {\n    fn kind() string\n    fn legs(self) int\n}\nfn f(x: dyn Animal) {}\nfn main() void {}\n";
    one_sema(src, "TYPE-138");
    let m = messages(src);
    assert!(m[0].contains("`Animal.kind` has no `self` receiver"), "{m:?}");
    assert!(!m[0].contains("legs"), "{m:?}");
}

#[test]
fn a_method_that_mentions_self_is_not_object_safe() {
    let src = "trait Cmp { fn same(self, other: Self) bool }\nfn f(x: dyn Cmp) {}\nfn main() void {}\n";
    one_sema(src, "TYPE-138");
    assert!(messages(src)[0].contains("`Cmp.same` mentions `Self` outside its receiver"));
}

#[test]
fn a_method_returning_self_is_not_object_safe() {
    let src = "trait Dup { fn dup(self) Self }\nfn f(x: dyn Dup) {}\nfn main() void {}\n";
    one_sema(src, "TYPE-138");
}

#[test]
fn a_generic_method_is_not_object_safe() {
    let src = "trait Gen { fn map<U>(self, x: U) U }\nfn f(x: dyn Gen) {}\nfn main() void {}\n";
    one_sema(src, "TYPE-138");
    assert!(messages(src)[0].contains("`Gen.map` has generic parameters of its own"));
}

#[test]
fn a_supertrait_that_is_not_object_safe_makes_the_trait_not_object_safe() {
    let src = "trait Base { fn same(self, o: Self) bool }\ntrait Child: Base { fn n(self) int }\nfn f(x: dyn Child) {}\nfn main() void {}\n";
    one_sema(src, "TYPE-138");
    assert!(messages(src)[0].contains("`Base.same`"));
}

#[test]
fn every_offending_method_is_listed_in_one_diagnostic() {
    let src = "trait Bad {\n    fn a() int\n    fn b<U>(self, x: U) U\n    fn c(self, o: Self) bool\n    fn ok(self) int\n}\nfn f(x: dyn Bad) {}\nfn main() void {}\n";
    one_sema(src, "TYPE-138");
    let m = &messages(src)[0];
    assert!(m.contains("`Bad.a`") && m.contains("`Bad.b`") && m.contains("`Bad.c`"), "{m}");
    assert!(!m.contains("Bad.ok"), "{m}");
}

#[test]
fn the_comparison_and_clone_prelude_traits_are_not_object_safe() {
    for name in ["PartialEq", "Eq", "PartialOrd", "Ord", "Clone", "Hash"] {
        let src = format!("fn f(x: dyn {name}) {{}}\nfn main() void {{}}\n");
        one_sema(&src, "TYPE-138");
    }
}

#[test]
fn display_iterator_and_the_operator_traits_are_object_safe() {
    for ty in ["dyn Display", "dyn Iterator<int>", "dyn Add<int, int>", "dyn Index<int, int>", "dyn IndexSet<int, int>"] {
        clean(&format!("fn f(x: {ty}) {{}}\nfn main() void {{}}\n"));
    }
}

#[test]
fn the_diagnostic_names_the_trait_and_suggests_a_separate_trait() {
    let src = "trait Animal { fn kind() string }\nfn f(x: dyn Animal) {}\nfn main() void {}\n";
    assert!(messages(src)[0].contains("the trait `Animal` cannot be used as `dyn Animal`"));
    let s = suggestions(src)[0].clone().expect("a suggestion");
    assert!(s.contains("separate trait"), "{s}");
}

#[test]
fn a_trait_declared_after_its_dyn_use_is_checked_once() {
    let src = "fn bad(x: dyn Later) {}\ntrait Later {\n    fn make() int\n    fn size(self) int\n}\nfn main() void {}\n";
    one_sema(src, "TYPE-138");
}

#[test]
fn each_written_dyn_type_is_reported_where_it_is_written() {
    let src = "trait Later { fn make() int }\nfn a(x: dyn Later) {}\nfn b(x: dyn Later) {}\nfn main() void {}\n";
    let (_, c) = codes(src);
    assert_eq!(c, vec!["TYPE-138", "TYPE-138"]);
}

#[test]
fn a_dyn_in_a_method_signature_is_reported_once() {
    let src = "trait Later { fn make() int }\nstruct S { a: int }\nimpl S { fn take(self, x: dyn Later) int { return 1 } }\nfn main() void {}\n";
    one_sema(src, "TYPE-138");
}

// ── Naming the trait ────────────────────────────────────────────

#[test]
fn dyn_of_an_unknown_name_or_a_non_trait_is_type_122() {
    one_sema("fn f(x: dyn Nope) {}\nfn main() void {}\n", "TYPE-122");
    one_sema("struct Circle { r: int }\nfn f(x: dyn Circle) {}\nfn main() void {}\n", "TYPE-122");
}

#[test]
fn dyn_of_a_generic_trait_needs_its_arguments() {
    one_sema("trait Convert<T> { fn conv(self) T }\nfn f(x: dyn Convert) {}\nfn main() void {}\n", "TYPE-108");
    one_sema("trait Shape { fn area(self) int }\nfn f(x: dyn Shape<int>) {}\nfn main() void {}\n", "TYPE-108");
}

// ── Coercion: what is accepted and what is not ──────────────────

#[test]
fn a_type_without_the_impl_is_not_accepted_at_any_typed_site() {
    let sites = [
        "    let y: dyn Shape = Rock { w = 2 }",
        "    let l: List<dyn Shape> = List.new()\n    l.push(Rock { w = 1 })",
        "    let l: List<dyn Shape> = List.new()\n    l.push(5)",
        "    let l: List<dyn Shape> = [Circle { r = 1 }, Rock { w = 1 }]",
        "    let d: dyn Shape = 5",
        "    let c = 5 as dyn Shape",
    ];
    for body in sites {
        one_sema(&with_shapes(body), "TYPE-126");
    }
}

#[test]
fn an_argument_a_return_and_a_field_need_the_impl() {
    one_sema(&with_items("fn take(s: dyn Shape) {}\n", "    take(Rock { w = 1 })"), "TYPE-126");
    one_sema(&with_items("fn make() dyn Shape { return Rock { w = 1 } }\n", "    let m = make()"), "TYPE-126");
    one_sema(&with_items("struct H { s: dyn Shape }\n", "    let h = H { s = Rock { w = 1 } }"), "TYPE-126");
}

#[test]
fn every_element_taking_collection_method_checks_a_dyn_element() {
    let bodies = [
        "    let q: Queue<dyn Shape> = Queue.new()\n    q.enqueue(5)",
        "    let s: Stack<dyn Shape> = Stack.new()\n    s.push(5)",
        "    let l: List<dyn Shape> = [Circle { r = 1 }]\n    l.set(0, 5)",
        "    let d: Dictionary<string, dyn Shape> = Dictionary.new()\n    d.set(\"k\", 5)",
    ];
    for body in bodies {
        one_sema(&with_shapes(body), "TYPE-126");
    }
}

#[test]
fn the_arguments_of_the_trait_have_to_agree() {
    let src = r#"
trait Convert<T> { fn conv(self) T }
struct Dog { n: int }
impl Convert<string> for Dog { fn conv(self) string { return "d" } }
fn main() void {
    let cs: List<dyn Convert<int>> = List.new()
    cs.push(Dog { n = 1 })
}
"#;
    one_sema(src, "TYPE-126");
}

#[test]
fn there_is_no_conversion_between_dyn_types_or_back_to_the_type() {
    let two = "trait Named { fn name(self) string }\n";
    one_sema(&with_items(two, "    let a: dyn Shape = Circle { r = 1 }\n    let b: dyn Named = a"), "TYPE-101");
    one_sema(&with_shapes("    let a: dyn Shape = Circle { r = 1 }\n    let c: Circle = a"), "TYPE-101");
}

#[test]
fn a_list_of_a_type_is_not_a_list_of_dyn() {
    one_sema(&with_shapes("    let l: List<dyn Shape> = List.new()\n    let k: List<Circle> = l"), "TYPE-101");
}

#[test]
fn a_dyn_return_is_not_an_int() {
    let items = "fn id(s: dyn Shape) dyn Shape { return s }\n";
    one_sema(&with_items(items, "    let n: int = id(Circle { r = 1 })"), "TYPE-101");
}

#[test]
fn a_method_the_trait_lacks_is_type_104_and_names_the_dyn_type() {
    let src = with_shapes("    let a: dyn Shape = Circle { r = 1 }\n    a.nope()");
    one_sema(&src, "TYPE-104");
    assert!(messages(&src)[0].contains("`dyn Shape`"), "{:?}", messages(&src));
}

#[test]
fn impl_and_extend_for_a_dyn_type_are_type_129() {
    one_sema(&with_items("impl Shape for dyn Shape { fn area(self) int { return 0 } fn name(self) string { return \"\" } }\n", ""), "TYPE-129");
    one_sema(&with_items("extend dyn Shape { fn twice(self) int { return 2 } }\n", ""), "TYPE-129");
}

// ── Further coercion sites and checks ───────────────────────────

#[test]
fn an_assignment_coerces_to_a_dyn_binding() {
    let src = with_shapes("    let mut a: dyn Shape = Circle { r = 1 }\n    a = Square { side = 9 }\n    println(a.area())");
    assert_eq!(out(&src), "81\n");
}

#[test]
fn an_enum_payload_may_be_dyn() {
    let src = r#"
trait Shape { fn area(self) int }
struct Circle { r: int }
impl Shape for Circle { fn area(self) int { return self.r } }
enum Slot { Full(dyn Shape), Empty }
fn main() void {
    let s = Slot.Full(Circle { r = 6 })
    match s {
        Slot.Full(x) => println(x.area()),
        Slot.Empty => println(0),
    }
}
"#;
    assert_eq!(out(src), "6\n");
}

#[test]
fn a_dyn_does_not_satisfy_a_bound_of_a_trait_it_lacks() {
    let items = "trait Named { fn name(self) string }\nfn describe<T: Named>(x: T) string { return x.name() }\n";
    one_sema(&with_items(items, "    let s: dyn Shape = Circle { r = 1 }\n    println(describe(s))"), "TYPE-126");
}

#[test]
fn dyn_types_with_different_arguments_are_different_types() {
    let src = r#"
trait Convert<T> { fn conv(self) T }
struct Dog { n: int }
impl Convert<int> for Dog { fn conv(self) int { return 1 } }
fn main() void {
    let a: dyn Convert<int> = Dog { n = 1 }
    let b: dyn Convert<string> = a
}
"#;
    one_sema(src, "TYPE-101");
}

#[test]
fn a_for_over_a_dyn_does_not_suggest_an_impl_for_the_dyn_type() {
    let dyn_src = with_shapes("    let a: dyn Shape = Circle { r = 1 }\n    for x in a { println(x) }");
    let s = suggestions(&dyn_src)[0].clone().expect("a suggestion");
    assert!(!s.contains("add `impl Iterator"), "{s}");
    assert!(s.contains("`Iterator<T>`"), "{s}");
    let plain = with_items("struct Plain { a: int }\n", "    let p = Plain { a = 1 }\n    for y in p { println(y) }");
    let s = suggestions(&plain)[0].clone().expect("a suggestion");
    assert!(s.contains("add `impl Iterator<Item> for Plain"), "{s}");
}

#[test]
fn the_arguments_of_a_dyn_are_checked_for_undeclared_lifetimes() {
    let src = "trait Convert<T> { fn conv(self) T }\nfn f(x: dyn Convert<&a int>) int { return 1 }\nfn main() void {}\n";
    one_sema(src, "LIFETIME-001");
}

#[test]
fn an_arena_borrow_is_not_elided_for_a_function_returning_dyn() {
    let head = r#"
@tier(mid)
trait Shape { fn area(self) int }
@tier(mid) {
    struct Circle { r: int }
    struct Square { side: int }
    impl Shape for Circle { fn area(self) int { return self.r } }
    impl Shape for Square { fn area(self) int { return self.side } }
}

@tier(mid)
fn grab(c: &Circle) dyn Shape { return Square { side = 1 } }

@tier(mid)
fn size(c: &Circle) int { return c.r }
"#;
    let ok = format!("{head}\n@tier(mid)\nfn run() int {{\n    with arena(256) {{\n        let r = Circle {{ r = 3 }}\n        return size(&r)\n    }}\n}}\nfn main() void {{ println(run()) }}\n");
    assert_eq!(out(&ok), "3\n");
    let bad = format!("{head}\n@tier(mid)\nfn run() int {{\n    with arena(256) {{\n        let r = Circle {{ r = 3 }}\n        let s = grab(&r)\n        return 1\n    }}\n}}\nfn main() void {{ println(run()) }}\n");
    one_sema(&bad, "TIER-004");
}

#[test]
fn shared_and_syncshared_values_may_be_dyn_in_low() {
    let src = r#"
@tier(low)
trait Body { fn mass(self) int }
@tier(low) {
    struct Rock { m: int }
    impl Body for Rock { fn mass(self) int { return self.m } }
}
@tier(low)
fn sh() int {
    let s: Shared<dyn Body> = Shared.new(Rock { m = 4 })
    return s.mass()
}
@tier(low)
fn sy() int {
    let s: SyncShared<dyn Body> = SyncShared.new(Rock { m = 5 })
    return s.mass()
}
fn main() void {
    println(sh())
    println(sy())
}
"#;
    assert_eq!(out(src), "4\n5\n");
}

#[test]
fn the_arguments_of_a_dyn_are_matched_against_a_bound() {
    let src = r#"
trait Convert<T> { fn conv(self) T }
struct Dog { n: int }
impl Convert<int> for Dog { fn conv(self) int { return 1 } }
fn want<T: Convert<string>>(x: T) int { return 1 }
fn main() void {
    let a: dyn Convert<int> = Dog { n = 1 }
    println(want(a))
}
"#;
    one_sema(src, "TYPE-126");
}

#[test]
fn the_item_of_a_for_over_a_dyn_iterator_is_typed() {
    let src = r#"
struct Counter { n: int, max: int }
impl Iterator<int> for Counter {
    fn next(self) int? {
        if self.n >= self.max { return null }
        self.n = self.n + 1
        return self.n
    }
}
fn main() void {
    let it: dyn Iterator<int> = Counter { n = 0, max = 2 }
    for v in it { let s: string = v }
}
"#;
    one_sema(src, "TYPE-101");
}

#[test]
fn a_struct_variant_payload_may_be_dyn() {
    let src = r#"
trait Shape { fn area(self) int }
struct Circle { r: int }
impl Shape for Circle { fn area(self) int { return self.r } }
enum Slot { Full { s: dyn Shape }, Empty }
fn main() void {
    let s = Slot.Full { s = Circle { r = 6 } }
    match s {
        Slot.Full { s } => println(s.area()),
        Slot.Empty => println(0),
    }
}
"#;
    assert_eq!(out(src), "6\n");
}

#[test]
fn self_inside_another_type_still_counts_as_self() {
    let src = "trait Pairs { fn all(self) List<Self> }\ntrait Convert<T> { fn conv(self) T }\ntrait WithDyn { fn take(self, c: dyn Convert<Self>) int }\nfn a(x: dyn Pairs) {}\nfn b(x: dyn WithDyn) {}\nfn main() void {}\n";
    let (_, c) = codes(src);
    assert_eq!(c, vec!["TYPE-138", "TYPE-138"]);
    let m = messages(src);
    assert!(m[0].contains("`Pairs.all`") && m[1].contains("`WithDyn.take`"), "{m:?}");
}

#[test]
fn a_dyn_return_type_is_reported_once() {
    let src = "trait Bad { fn make() int }\nstruct S { a: int }\nimpl Bad for S { fn make() int { return 1 } }\nfn g() dyn Bad { return S { a = 1 } }\nfn main() void {}\n";
    one_sema(src, "TYPE-138");
}

#[test]
fn a_low_function_borrows_a_local_as_dyn() {
    let src = r#"
@tier(low)
trait Body { fn mass(self) int }
@tier(low) {
    struct Rock { m: int }
    impl Body for Rock { fn mass(self) int { return self.m } }
}
@tier(low)
fn weigh(b: &dyn Body) int { return b.mass() }
@tier(low)
fn local() int {
    let r = Rock { m = 9 }
    return weigh(&r)
}
@tier(low)
fn local_mut() int {
    let mut r = Rock { m = 4 }
    let d: &dyn Body = &r
    return d.mass()
}
fn main() void {
    println(local())
    println(local_mut())
}
"#;
    assert_eq!(out(src), "9\n4\n");
}

#[test]
fn an_optional_of_a_type_is_an_optional_dyn() {
    let src = with_shapes("    let a: Circle? = Circle { r = 1 }\n    let b: dyn Shape? = a\n    println(b.area())");
    assert_eq!(out(&src), "3\n");
}

#[test]
fn a_wrapper_the_dyn_type_lacks_is_not_removed() {
    let src = with_shapes("    println(1)").replace("fn main() void {", "fn f(u: Unique<Circle>) { let d: dyn Shape = u }\nfn g(c: &Circle) { let d: dyn Shape = c }\nfn main() void {");
    let (_, c) = codes(&src);
    assert_eq!(c, vec!["TYPE-101", "TYPE-101"]);
}

#[test]
fn list_set_takes_a_dyn_element() {
    let src = with_shapes("    let l: List<dyn Shape> = [Circle { r = 1 }]\n    l.set(0, Square { side = 3 })\n    println(l[0].area())");
    assert_eq!(out(&src), "9\n");
}

#[test]
fn an_inline_list_of_dyn() {
    let ok = with_shapes("    let il: InlineList<dyn Shape> = InlineList.new(4)\n    il.push(Circle { r = 7 })\n    for x in il { println(x.area()) }");
    assert_eq!(out(&ok), "147\n");
    let bad = with_shapes("    let il: InlineList<dyn Shape> = InlineList.new(4)\n    il.push(5)");
    one_sema(&bad, "TYPE-126");
}

#[test]
fn a_method_argument_is_coerced_to_a_dyn_parameter() {
    let items = "struct Host { a: int }\nimpl Host { fn run(self, s: dyn Shape) int { return s.area() } }\n";
    let ok = with_items(items, "    let h = Host { a = 1 }\n    println(h.run(Square { side = 4 }))");
    assert_eq!(out(&ok), "16\n");
    let bad = with_items(items, "    let h = Host { a = 1 }\n    println(h.run(Rock { w = 1 }))");
    one_sema(&bad, "TYPE-126");
}

#[test]
fn an_array_literal_of_borrowed_dyn_is_coerced_element_by_element() {
    let src = with_shapes("    let c = Circle { r = 1 }\n    let s = Square { side = 2 }\n    let l: List<&dyn Shape> = [&c, &s]\n    for x in l { println(x.area()) }");
    assert_eq!(out(&src), "3\n4\n");
}

#[test]
fn an_untyped_lambda_parameter_becomes_a_dyn_parameter() {
    let src = with_items("fn take(d: dyn Shape) int { return d.area() }\n", "    let f = fn(x) take(x)\n    println(f(Circle { r = 1 }))");
    assert_eq!(out(&src), "3\n");
}

#[test]
fn an_arena_value_does_not_become_a_plain_dyn_value() {
    let src = r#"
@tier(mid)
trait Shape { fn area(self) int }
@tier(mid) {
    struct Circle { r: int }
    impl Shape for Circle { fn area(self) int { return self.r } }
}
@tier(mid)
fn run() int {
    with arena(256) {
        let d: dyn Shape = Circle { r = 1 }
        return d.area()
    }
}
fn main() void { println(run()) }
"#;
    one_sema(src, "TIER-004");
}

#[test]
fn shared_and_syncshared_values_are_not_unwrapped_into_a_dyn_value() {
    let src = "trait Shape { fn area(self) int }\nstruct Circle { r: int }\nimpl Shape for Circle { fn area(self) int { return self.r } }\n@tier(low)\nfn f(a: Shared<Circle>, b: SyncShared<Circle>) {\n    let d: dyn Shape = a\n    let e: dyn Shape = b\n}\nfn main() void {}\n";
    let (_, c) = codes(src);
    assert_eq!(c, vec!["TYPE-101", "TYPE-101"]);
}
