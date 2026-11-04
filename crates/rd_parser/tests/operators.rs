//! Operator traits, slice S5c.
//!
//! `Add`, `Sub`, `Mul`, `Div`, `Rem`, `BitAnd`, `BitOr`, `BitXor`, `Shl`,
//! `Shr` (generic in the right operand and the result: `Mul<Rhs, Out>`) and
//! `Neg<Out>`, `BitNot<Out>` are prelude traits. A struct or an enum that
//! implements one is what `a * b`, `-a`, `~a` and `a *= b` call. A type can
//! implement one operator trait several times, once per kind of right
//! operand (`Mul<double, Vec2>` and `Mul<Vec2, Vec2>`), and a number or a
//! string on the left can take a struct or an enum on the right
//! (`impl Mul<Vec2, Vec2> for double`, so `2.0 * v` works). Numbers keep the
//! run time's own arithmetic and satisfy the traits natively, so a bound
//! such as `T: Add<T, T>` accepts `int`, `double`, `string` and any impl.
//!
//! Sema reports an operator with no impl behind it (`TYPE-136`), an operator
//! on an unbounded type parameter (`TYPE-131`), and the impl mistakes that
//! belong to this slice (`TYPE-128`, `TYPE-129`, `TYPE-134`).
//!
//! See docs/TRAITS_DESIGN.md, "What S5c settled".

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

/// A `Vec2` with `+`, `*` by a double and `*` by a `Vec2`.
const VEC2: &str = r#"
struct Vec2 {
    x: double,
    y: double,
}

impl Add<Vec2, Vec2> for Vec2 {
    fn add(self, rhs: Vec2) Vec2 {
        return Vec2 { x = self.x + rhs.x, y = self.y + rhs.y }
    }
}

impl Mul<double, Vec2> for Vec2 {
    fn mul(self, rhs: double) Vec2 {
        return Vec2 { x = self.x * rhs, y = self.y * rhs }
    }
}

impl Mul<Vec2, Vec2> for Vec2 {
    fn mul(self, rhs: Vec2) Vec2 {
        return Vec2 { x = self.x * rhs.x, y = self.y * rhs.y }
    }
}
"#;

fn with_vec2(main_body: &str) -> String {
    format!("{VEC2}\nfn main() void {{\n    let a = Vec2 {{ x = 1.0, y = 2.0 }}\n    let b = Vec2 {{ x = 3.0, y = 4.0 }}\n{main_body}\n}}\n")
}

// ── Run time: the operators call the impls ──────────────────────

#[test]
fn add_on_two_structs_calls_the_impl() {
    let src = with_vec2("    let s = a + b\n    println(s.x)\n    println(s.y)");
    assert_eq!(out(&src), "4\n6\n");
}

#[test]
fn one_type_several_impls_of_one_operator_by_right_operand() {
    let src = with_vec2("    let k = a * 2.0\n    println(k.y)\n    let m = a * b\n    println(m.y)");
    assert_eq!(out(&src), "4\n8\n");
}

#[test]
fn a_number_on_the_left_takes_a_struct_on_the_right() {
    let src = format!("{VEC2}\nimpl Mul<Vec2, Vec2> for double {{\n    fn mul(self, rhs: Vec2) Vec2 {{\n        return Vec2 {{ x = self * rhs.x, y = self * rhs.y }}\n    }}\n}}\nfn main() void {{\n    let a = Vec2 {{ x = 1.0, y = 2.0 }}\n    let l = 3.0 * a\n    println(l.y)\n    println(2.5 * 2.0)\n}}\n");
    assert_eq!(out(&src), "6\n5\n");
}

#[test]
fn a_string_on_the_left_takes_a_struct_on_the_right() {
    let src = format!("{VEC2}\nimpl Add<Vec2, string> for string {{\n    fn add(self, rhs: Vec2) string {{\n        return self + \"!\"\n    }}\n}}\nfn main() void {{\n    let a = Vec2 {{ x = 1.0, y = 2.0 }}\n    let s = \"v\" + a\n    println(s)\n    println(\"a\" + \"b\")\n}}\n");
    assert_eq!(out(&src), "v!\nab\n");
}

const FLAGS: &str = r#"
struct Flags {
    bits: int,
}
impl BitOr<Flags, Flags> for Flags {
    fn bit_or(self, rhs: Flags) Flags { return Flags { bits = self.bits | rhs.bits } }
}
impl BitAnd<Flags, Flags> for Flags {
    fn bit_and(self, rhs: Flags) Flags { return Flags { bits = self.bits & rhs.bits } }
}
impl BitXor<Flags, Flags> for Flags {
    fn bit_xor(self, rhs: Flags) Flags { return Flags { bits = self.bits ^ rhs.bits } }
}
impl Shl<int, Flags> for Flags {
    fn shl(self, rhs: int) Flags { return Flags { bits = self.bits << rhs } }
}
impl Shr<int, Flags> for Flags {
    fn shr(self, rhs: int) Flags { return Flags { bits = self.bits >> rhs } }
}
impl BitNot<Flags> for Flags {
    fn bit_not(self) Flags { return Flags { bits = ~self.bits } }
}
impl Rem<int, int> for Flags {
    fn rem(self, rhs: int) int { return self.bits % rhs }
}
impl Div<int, Flags> for Flags {
    fn div(self, rhs: int) Flags { return Flags { bits = self.bits / rhs } }
}
impl Sub<Flags, Flags> for Flags {
    fn sub(self, rhs: Flags) Flags { return Flags { bits = self.bits - rhs.bits } }
}
impl Neg<Flags> for Flags {
    fn neg(self) Flags { return Flags { bits = 0 - self.bits } }
}
"#;

fn with_flags(main_body: &str) -> String {
    format!("{FLAGS}\nfn main() void {{\n    let a = Flags {{ bits = 12 }}\n    let b = Flags {{ bits = 10 }}\n{main_body}\n}}\n")
}

#[test]
fn every_bitwise_operator_calls_its_impl() {
    let src = with_flags("    let o = a | b\n    let n = a & b\n    let x = a ^ b\n    let l = a << 2\n    let r = a >> 2\n    println(o.bits)\n    println(n.bits)\n    println(x.bits)\n    println(l.bits)\n    println(r.bits)");
    assert_eq!(out(&src), "14\n8\n6\n48\n3\n");
}

#[test]
fn rem_div_sub_and_the_unary_operators_call_their_impls() {
    let src = with_flags("    println(a % 5)\n    let d = a / 4\n    println(d.bits)\n    let s = a - b\n    println(s.bits)\n    let t = ~a\n    println(t.bits)\n    let n = -a\n    println(n.bits)");
    assert_eq!(out(&src), "2\n3\n2\n-13\n-12\n");
}

#[test]
fn an_operator_impl_may_return_another_type() {
    let src = r#"
struct V { x: int, y: int }
impl Mul<V, int> for V {
    fn mul(self, rhs: V) int { return self.x * rhs.x + self.y * rhs.y }
}
fn main() void {
    let a = V { x = 1, y = 2 }
    let b = V { x = 3, y = 4 }
    let dot: int = a * b
    println(dot)
}
"#;
    assert_eq!(out(src), "11\n");
}

#[test]
fn compound_assignment_goes_through_the_operator_impl() {
    let src = with_flags("    let mut f = a\n    f |= b\n    println(f.bits)\n    f <<= 1\n    println(f.bits)\n    f /= 2\n    println(f.bits)\n    f &= b\n    println(f.bits)\n    f ^= a\n    println(f.bits)\n    f -= b\n    println(f.bits)\n    f >>= 1\n    println(f.bits)");
    assert_eq!(out(&src), "14\n28\n14\n10\n6\n-4\n-2\n");
}

#[test]
fn compound_assignment_with_a_different_right_operand_kind() {
    let src = with_vec2("    let mut v = a\n    v += b\n    v *= 2.0\n    println(v.x)\n    println(v.y)\n    v *= b\n    println(v.y)");
    assert_eq!(out(&src), "8\n12\n48\n");
}

#[test]
fn operators_combine_and_follow_precedence() {
    let src = with_vec2("    let c = a + b * 2.0\n    println(c.x)\n    let d = (a + b) * 0.5 + a\n    println(d.y)");
    assert_eq!(out(&src), "7\n5\n");
}

#[test]
fn an_integer_literal_right_operand_takes_the_integer_impl() {
    let src = r#"
struct V { x: int }
impl Mul<int, V> for V {
    fn mul(self, rhs: int) V { return V { x = self.x * rhs } }
}
fn main() void {
    let a = V { x = 7 }
    let b = a * 3
    println(b.x)
}
"#;
    assert_eq!(out(src), "21\n");
}

#[test]
fn an_integer_literal_takes_the_type_of_the_impl_it_picks() {
    let src = r#"
struct V { x: u32 }
impl Mul<u32, V> for V {
    fn mul(self, rhs: u32) V { return V { x = self.x * rhs } }
}
fn main() void {
    let a = V { x = 7u32 }
    let b = a * 3
    println(b.x)
}
"#;
    assert_eq!(out(src), "21\n");
}

#[test]
fn operators_work_inside_methods_and_loops() {
    let src = r#"
struct V { x: int }
impl Add<V, V> for V {
    fn add(self, rhs: V) V { return V { x = self.x + rhs.x } }
}
struct Body { pos: V, vel: V }
impl Body {
    fn step(self) void {
        self.pos = self.pos + self.vel
    }
}
fn main() void {
    let b = Body { pos = V { x = 1 }, vel = V { x = 5 } }
    b.step()
    b.step()
    println(b.pos.x)
    let mut total = V { x = 0 }
    for i in 0 .. 4 {
        total += V { x = i }
    }
    println(total.x)
}
"#;
    assert_eq!(out(src), "11\n6\n");
}

#[test]
fn an_enum_can_be_an_operand() {
    let src = r#"
enum Level { Low, High }
impl Add<Level, Level> for Level {
    fn add(self, rhs: Level) Level { return Level.High }
}
fn main() void {
    let l = Level.Low + Level.Low
    println(l == Level.High)
}
"#;
    assert_eq!(out(src), "true\n");
}

#[test]
fn an_impl_for_a_generic_type_serves_every_instance() {
    let src = r#"
struct Pair<T> { a: T, b: T }
impl<T: Add<T, T>> Add<Pair<T>, Pair<T>> for Pair<T> {
    fn add(self, rhs: Pair<T>) Pair<T> {
        return Pair { a = self.a + rhs.a, b = self.b + rhs.b }
    }
}
fn main() void {
    let p = Pair { a = 1, b = 2 }
    let q = Pair { a = 10, b = 20 }
    let r = p + q
    println(r.a)
    println(r.b)
    let pd = Pair { a = 1.5, b = 2.5 }
    let rd = pd + pd
    println(rd.b)
}
"#;
    assert_eq!(out(src), "11\n22\n5\n");
}

#[test]
fn a_trait_default_method_can_use_an_operator_its_supertrait_gives() {
    let src = r#"
struct V { x: int }
impl Add<V, V> for V {
    fn add(self, rhs: V) V { return V { x = self.x + rhs.x } }
}
trait Doubler: Add<Self, Self> {
    fn twice(self) Self {
        return self + self
    }
}
impl Doubler for V {}
fn main() void {
    let t = V { x = 4 }.twice()
    println(t.x)
}
"#;
    assert_eq!(out(src), "8\n");
}

// ── Numbers and strings keep their own arithmetic ───────────────

#[test]
fn numbers_and_strings_are_untouched_when_the_program_has_operator_impls() {
    let src = format!("{VEC2}\nfn main() void {{\n    let mut n = 5\n    n += 3\n    n <<= 1\n    println(n)\n    let mut s = \"a\"\n    s += \"b\"\n    println(s)\n    println(7 % 4)\n    println(-n)\n    println(~5)\n    println(2.5 * 2.0)\n    let x: u8 = 200u8\n    println(x & 15u8)\n}}\n");
    assert_eq!(out(&src), "16\nab\n3\n-16\n-6\n5\n8\n");
}

// ── Bounds ──────────────────────────────────────────────────────

const GENERIC_SUM: &str = r#"
fn sum<T: Add<T, T>>(a: T, b: T) T {
    return a + b
}
fn scale<T: Mul<double, T>>(a: T, k: double) T {
    return a * k
}
fn both<T: Mul<double, T> + Add<T, T>>(a: T, b: T) T {
    return (a + b) * 2.0
}
fn call_add<T: Add<T, T>>(a: T, b: T) T {
    return a.add(b)
}
fn neg_it<T: Neg<T>>(a: T) T {
    return -a
}
"#;

#[test]
fn a_bound_is_satisfied_by_numbers_strings_and_impls() {
    let src = format!("{VEC2}{GENERIC_SUM}\nfn main() void {{\n    let a = Vec2 {{ x = 1.0, y = 2.0 }}\n    let b = Vec2 {{ x = 3.0, y = 4.0 }}\n    println(sum(a, b).x)\n    println(sum(1, 2))\n    println(sum(1.5, 2.5))\n    println(sum(\"a\", \"b\"))\n    println(scale(a, 3.0).y)\n    println(scale(1.5, 2.0))\n    println(both(a, b).y)\n    println(both(1.5, 2.5))\n}}\n");
    assert_eq!(out(&src), "4\n3\n4\nab\n6\n3\n12\n8\n");
}

#[test]
fn the_operator_method_can_be_called_through_a_bound() {
    let src = format!("{VEC2}{GENERIC_SUM}\nfn main() void {{\n    let a = Vec2 {{ x = 1.0, y = 2.0 }}\n    let b = Vec2 {{ x = 3.0, y = 4.0 }}\n    println(call_add(a, b).y)\n    println(call_add(10, 5))\n    println(neg_it(7))\n    println(neg_it(2.5))\n}}\n");
    assert_eq!(out(&src), "6\n15\n-7\n-2.5\n");
}

#[test]
fn a_parameter_may_be_bounded_once_per_right_operand() {
    let src = format!("{VEC2}\nfn mix<T: Mul<double, T> + Mul<T, T>>(a: T, b: T) T {{\n    return (a * 2.0) * b\n}}\nfn main() void {{\n    let a = Vec2 {{ x = 1.0, y = 2.0 }}\n    let b = Vec2 {{ x = 3.0, y = 4.0 }}\n    println(mix(a, b).y)\n}}\n");
    assert_eq!(out(&src), "16\n");
}

#[test]
fn an_unbounded_parameter_cannot_use_an_operator() {
    let src = "fn bad<T>(a: T, b: T) T {\n    return a + b\n}\nfn main() void {\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-131"]));
    assert_eq!(suggestions(src), vec![Some("declare the bound, as in `<T: Add<T, T>>`".to_string())]);
}

#[test]
fn an_unbounded_parameter_cannot_be_negated_or_compound_assigned() {
    let neg = "fn bad<T>(a: T) T {\n    return -a\n}\nfn main() void {\n}\n";
    assert_eq!(codes(neg), (Stage::Sema, vec!["TYPE-131"]));
    assert_eq!(suggestions(neg), vec![Some("declare the bound, as in `<T: Neg<T>>`".to_string())]);
    let compound = "fn bad<T>(a: T, b: T) void {\n    let mut c = a\n    c += b\n}\nfn main() void {\n}\n";
    assert_eq!(codes(compound), (Stage::Sema, vec!["TYPE-131"]));
}

#[test]
fn a_bound_on_the_wrong_operator_is_not_enough() {
    let src = "fn bad<T: Add<T, T>>(a: T, b: T) T {\n    return a - b\n}\nfn main() void {\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-131"]));
}

#[test]
fn a_type_without_the_impl_does_not_satisfy_the_bound() {
    let src = format!("{VEC2}{GENERIC_SUM}\nfn main() void {{\n    println(sum(true, false))\n}}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
}

#[test]
fn an_impl_for_another_right_operand_does_not_satisfy_the_bound() {
    let src = r#"
struct V { x: int }
impl Mul<V, V> for V {
    fn mul(self, rhs: V) V { return V { x = self.x * rhs.x } }
}
fn scale<T: Mul<double, T>>(a: T, k: double) T {
    return a * k
}
fn main() void {
    let v = V { x = 1 }
    let s = scale(v, 2.0)
}
"#;
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-126"]));
}

#[test]
fn two_bounds_with_one_right_operand_and_two_results_conflict() {
    let src = "fn bad<T: Add<T, T> + Add<T, int>>(a: T) T {\n    return a\n}\nfn main() void {\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-134"]));
    let s = suggestions(src).remove(0).unwrap();
    assert!(s.contains("right operand"), "{s}");
}

// ── Sema: an operator with no impl behind it ────────────────────

#[test]
fn a_missing_impl_names_the_pair_and_the_impls_there_are() {
    let src = with_vec2("    let r = a - b");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-136"]));
    let m = messages(&src).remove(0);
    assert!(m.contains("`-`") && m.contains("`Vec2` and `Vec2`") && m.contains("Sub<R, Out>"), "{m}");
    let s = suggestions(&src).remove(0).unwrap();
    assert!(s.contains("impl Sub<Vec2, Out> for Vec2"), "{s}");
}

#[test]
fn a_right_operand_with_no_impl_lists_the_ones_that_exist() {
    let src = with_vec2("    let r = a * \"x\"");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-136"]));
    let s = suggestions(&src).remove(0).unwrap();
    assert!(s.contains("double, Vec2"), "{s}");
}

#[test]
fn an_integer_literal_does_not_fit_a_double_impl() {
    let src = with_vec2("    let r = a * 2");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-136"]));
    assert!(messages(&src)[0].contains("`Vec2` and `int`"), "{:?}", messages(&src));
}

#[test]
fn a_unary_operator_with_no_impl_is_reported() {
    let src = with_vec2("    let r = -a");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-136"]));
    let m = messages(&src).remove(0);
    assert!(m.contains("Neg<Out>"), "{m}");
    let src = with_vec2("    let r = ~a");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-136"]));
}

#[test]
fn a_number_on_the_left_needs_its_own_impl() {
    let src = with_vec2("    let r = 2.0 * a");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-136"]));
    let s = suggestions(&src).remove(0).unwrap();
    assert!(s.contains("impl Mul<Vec2, Out> for double"), "{s}");
}

#[test]
fn compound_assignment_needs_the_impl_too() {
    let src = with_vec2("    let mut v = a\n    v -= b");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-136"]));
}

#[test]
fn compound_assignment_needs_a_result_of_the_type_assigned_to() {
    let src = r#"
struct V { x: int }
impl Mul<int, int> for V {
    fn mul(self, rhs: int) int { return self.x * rhs }
}
fn main() void {
    let mut v = V { x = 1 }
    v *= 3
}
"#;
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-101"]));
}

#[test]
fn a_number_with_a_mismatched_right_operand_is_still_a_type_mismatch() {
    let src = "fn main() void {\n    let mut n = 5\n    n += 1.5\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-101"]));
}

#[test]
fn a_struct_inside_generic_arguments_still_needs_the_impl() {
    let src = r#"
struct V { x: int }
fn main() void {
    let a = V { x = 1 }
    let b = a + a
}
"#;
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-136"]));
}

#[test]
fn a_mismatched_instance_of_a_generic_right_operand_is_a_type_mismatch() {
    let src = r#"
struct Pair<T> { a: T }
impl Add<Pair<int>, Pair<int>> for Pair<int> {
    fn add(self, rhs: Pair<int>) Pair<int> { return self }
}
fn main() void {
    let p = Pair { a = 1 }
    let q = Pair { a = 1.5 }
    let r = p + q
}
"#;
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-101"]));
}

#[test]
fn each_missing_operator_is_reported_once() {
    let src = with_vec2("    let r1 = a - b\n    let r2 = a / b\n    let r3 = a % b");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-136", "TYPE-136", "TYPE-136"]));
}

// ── Sema: impl mistakes ─────────────────────────────────────────

#[test]
fn a_second_impl_for_the_same_kind_of_right_operand_overlaps() {
    let src = format!("{VEC2}\nimpl Mul<double, Vec2> for Vec2 {{\n    fn mul(self, rhs: double) Vec2 {{ return self }}\n}}\nfn main() void {{\n}}\n");
    let (stage, c) = codes(&src);
    assert!(matches!(stage, Stage::Sema));
    assert_eq!(c.first().copied(), Some("TYPE-128"));
    assert!(messages(&src)[0].contains("kind `double`"), "{:?}", messages(&src));
}

#[test]
fn two_unary_impls_overlap() {
    let src = r#"
struct V { x: int }
impl Neg<V> for V {
    fn neg(self) V { return self }
}
impl Neg<V> for V {
    fn neg(self) V { return self }
}
fn main() void {
}
"#;
    let (_, c) = codes(src);
    assert_eq!(c.first().copied(), Some("TYPE-128"));
}

#[test]
fn right_operands_of_one_run_time_kind_cannot_be_told_apart() {
    let src = r#"
struct Box<T> { v: T }
impl Mul<Box<int>, int> for Box<int> {
    fn mul(self, rhs: Box<int>) int { return 1 }
}
impl Mul<Box<double>, int> for Box<int> {
    fn mul(self, rhs: Box<double>) int { return 2 }
}
fn main() void {
}
"#;
    let (_, c) = codes(src);
    assert_eq!(c.first().copied(), Some("TYPE-128"));
}

#[test]
fn an_operator_impl_for_a_number_with_a_number_on_the_right_is_rejected() {
    let src = "impl Add<int, int> for int {\n    fn add(self, rhs: int) int { return 0 }\n}\nfn main() void {\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-129"]));
    assert!(messages(src)[0].contains("operator trait"), "{:?}", messages(src));
}

#[test]
fn an_operator_impl_for_a_collection_is_rejected() {
    let src = r#"
struct V { x: int }
impl Add<V, V> for List<int> {
    fn add(self, rhs: V) V { return rhs }
}
fn main() void {
}
"#;
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-129"]));
}

#[test]
fn an_operator_impl_for_a_tuple_is_rejected() {
    let src = r#"
struct V { x: int }
impl Add<V, V> for (int, int) {
    fn add(self, rhs: V) V { return rhs }
}
fn main() void {
}
"#;
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-129"]));
}

#[test]
fn a_bare_type_parameter_as_the_right_operand_is_rejected() {
    let src = r#"
struct Box<T> { v: T }
impl<T> Mul<T, Box<T>> for Box<T> {
    fn mul(self, rhs: T) Box<T> { return self }
}
fn main() void {
}
"#;
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-129"]));
    assert!(messages(src)[0].contains("bare type parameter"), "{:?}", messages(src));
}

#[test]
fn an_operator_trait_takes_its_type_arguments() {
    let src = r#"
struct V { x: int }
impl Add<V> for V {
    fn add(self, rhs: V) V { return self }
}
fn main() void {
}
"#;
    let (stage, c) = codes(src);
    assert!(matches!(stage, Stage::Sema));
    assert!(!c.is_empty());
}

#[test]
fn the_operator_method_must_be_there_and_match() {
    let missing = "struct V { x: int }\nimpl Neg<V> for V {\n}\nfn main() void {\n}\n";
    let (stage, c) = codes(missing);
    assert!(matches!(stage, Stage::Sema));
    assert_eq!(c.len(), 1);
    let wrong = "struct V { x: int }\nimpl Add<V, V> for V {\n    fn add(self, rhs: int) V { return self }\n}\nfn main() void {\n}\n";
    let (stage, c) = codes(wrong);
    assert!(matches!(stage, Stage::Sema));
    assert_eq!(c.len(), 1);
}

// ── Shadowing: a program's own trait named like a prelude one ───

#[test]
fn a_program_trait_named_add_takes_the_name_and_the_operator_is_unchecked() {
    let src = r#"
trait Add<T> {
    fn add(self, other: T) T
}
struct V { x: int }
impl Add<int> for V {
    fn add(self, other: int) int { return self.x + other }
}
fn main() void {
    let v = V { x = 2 }
    println(v.add(3))
}
"#;
    clean(src);
    assert_eq!(out(src), "5\n");
}

// ── The run time ────────────────────────────────────────────────

#[test]
fn an_operator_impl_runs_at_run_time_for_an_unknown_static_type() {
    // The operands have a type sema knows; the impl is found by the kind of
    // the values, which is also what happens inside a generic body.
    let src = format!("{VEC2}{GENERIC_SUM}\nfn main() void {{\n    let a = Vec2 {{ x = 1.0, y = 2.0 }}\n    println(sum(a, a).y)\n    println(sum(2, 3))\n}}\n");
    assert_eq!(out(&src), "4\n5\n");
}

// ── Every operator goes through its own trait ───────────────────

const BINARY: &[(&str, &str, &str)] = &[
    ("Add", "add", "+"), ("Sub", "sub", "-"), ("Mul", "mul", "*"), ("Div", "div", "/"),
    ("Rem", "rem", "%"), ("BitAnd", "bit_and", "&"), ("BitOr", "bit_or", "|"),
    ("BitXor", "bit_xor", "^"), ("Shl", "shl", "<<"), ("Shr", "shr", ">>"),
];

/// A struct whose only operator impl is `trait_name`, adding `marker` to the
/// field, so a result shows which impl ran.
fn only_impl(trait_name: &str, method: &str, marker: usize) -> String {
    format!("struct V {{ x: int }}\nimpl {trait_name}<int, V> for V {{\n    fn {method}(self, rhs: int) V {{ return V {{ x = self.x + {marker} + rhs }} }}\n}}\n")
}

#[test]
fn each_binary_operator_calls_its_own_trait() {
    for (k, (t, m, op)) in BINARY.iter().enumerate() {
        let marker = (k + 1) * 100;
        let src = format!("{}fn main() void {{\n    let v = V {{ x = 10 }}\n    let r = v {op} 1\n    println(r.x)\n}}\n", only_impl(t, m, marker));
        assert_eq!(out(&src), format!("{}\n", 10 + marker + 1), "{t} for `{op}`");
    }
}

#[test]
fn each_compound_assignment_calls_its_own_trait() {
    for (k, (t, m, op)) in BINARY.iter().enumerate() {
        let marker = (k + 1) * 100;
        let src = format!("{}fn main() void {{\n    let mut v = V {{ x = 10 }}\n    v {op}= 1\n    println(v.x)\n}}\n", only_impl(t, m, marker));
        assert_eq!(out(&src), format!("{}\n", 10 + marker + 1), "{t} for `{op}=`");
    }
}

#[test]
fn an_operator_needs_its_own_trait_not_a_neighbour() {
    for (k, (_, _, op)) in BINARY.iter().enumerate() {
        let (t, m, _) = BINARY[(k + 1) % BINARY.len()];
        let src = format!("{}fn main() void {{\n    let v = V {{ x = 10 }}\n    let r = v {op} 1\n}}\n", only_impl(t, m, 1));
        assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-136"]), "`{op}` with only {t}");
        let src = format!("{}fn main() void {{\n    let mut v = V {{ x = 10 }}\n    v {op}= 1\n}}\n", only_impl(t, m, 1));
        assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-136"]), "`{op}=` with only {t}");
    }
}

#[test]
fn the_result_type_of_each_operator_comes_from_its_own_impl() {
    // `Rem` gives an int here and every other trait gives a V, so a typed let
    // finds an operator resolved through the wrong trait.
    let src = "struct V { x: int }\nimpl Rem<int, int> for V {\n    fn rem(self, rhs: int) int { return self.x % rhs }\n}\nimpl Sub<int, V> for V {\n    fn sub(self, rhs: int) V { return self }\n}\nfn main() void {\n    let v = V { x = 10 }\n    let a: int = v % 3\n    let b: V = v - 3\n    println(a)\n}\n";
    assert_eq!(out(src), "1\n");
}

#[test]
fn neg_and_bit_not_are_separate_traits() {
    let neg = "struct V { x: int }\nimpl Neg<V> for V {\n    fn neg(self) V { return V { x = 0 - self.x } }\n}\nfn main() void {\n    let v = V { x = 4 }\n    let n = -v\n    println(n.x)\n}\n";
    assert_eq!(out(neg), "-4\n");
    let neg_for_not = neg.replace("let n = -v", "let n = ~v");
    assert_eq!(codes(&neg_for_not), (Stage::Sema, vec!["TYPE-136"]));
    let not = "struct V { x: int }\nimpl BitNot<V> for V {\n    fn bit_not(self) V { return V { x = ~self.x } }\n}\nfn main() void {\n    let v = V { x = 4 }\n    let n = ~v\n    println(n.x)\n}\n";
    assert_eq!(out(not), "-5\n");
    let not_for_neg = not.replace("let n = ~v", "let n = -v");
    assert_eq!(codes(&not_for_neg), (Stage::Sema, vec!["TYPE-136"]));
}

// ── What numbers satisfy natively, and what they do not ─────────

#[test]
fn an_unsigned_number_cannot_be_negated_through_a_bound() {
    let src = "fn flip<T: Neg<T>>(a: T) T {\n    return -a\n}\nfn main() void {\n    println(flip(5u32))\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-126"]));
}

#[test]
fn a_float_does_not_satisfy_a_bitwise_bound() {
    let src = "fn mask<T: BitAnd<T, T>>(a: T, b: T) T {\n    return a & b\n}\nfn main() void {\n    println(mask(1.5, 2.5))\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-126"]));
    let ok = "fn mask<T: BitAnd<T, T>>(a: T, b: T) T {\n    return a & b\n}\nfn main() void {\n    println(mask(12, 10))\n}\n";
    assert_eq!(out(ok), "8\n");
}

#[test]
fn a_string_satisfies_add_and_nothing_else() {
    let src = "fn take<T: Sub<T, T>>(a: T, b: T) T {\n    return a - b\n}\nfn main() void {\n    println(take(\"a\", \"b\"))\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-126"]));
}

#[test]
fn bit_not_and_the_other_operator_methods_work_through_a_bound_on_numbers() {
    let src = "fn inv<T: BitNot<T>>(a: T) T {\n    return ~a\n}\nfn shift<T: Shl<T, T>>(a: T, b: T) T {\n    return a.shl(b)\n}\nfn main() void {\n    println(inv(5))\n    println(inv(5u8))\n    println(shift(3, 2))\n}\n";
    assert_eq!(out(src), "-6\n250\n12\n");
}

#[test]
fn the_unbounded_check_names_the_operator_it_found() {
    for (t, _, op) in BINARY {
        let src = format!("fn bad<T>(a: T, b: T) T {{\n    return a {op} b\n}}\nfn main() void {{\n}}\n");
        assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-131"]), "{op}");
        assert!(messages(&src)[0].contains(t), "{op}: {:?}", messages(&src));
    }
}

// ── Explicit calls and tiers ────────────────────────────────────

#[test]
fn the_method_can_be_called_by_name_on_a_type_with_several_impls() {
    let src = with_vec2("    let k = a.mul(3.0)\n    println(k.y)\n    let s = a.add(b)\n    println(s.x)");
    assert_eq!(out(&src), "6\n4\n");
}

#[test]
fn an_operator_impl_method_is_low_and_a_written_tier_must_match() {
    let src = "struct V { x: int }\nimpl Add<V, V> for V {\n    @tier(high)\n    fn add(self, rhs: V) V { return self }\n}\nfn main() void {\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TIER-016"]));
    let ok = "struct V { x: int }\nimpl Add<V, V> for V {\n    @tier(low)\n    fn add(self, rhs: V) V { return self }\n}\nfn main() void {\n}\n";
    clean(ok);
}

#[test]
fn an_operator_can_be_used_from_every_tier() {
    let src = r#"
struct V { x: int }
impl Add<V, V> for V {
    fn add(self, rhs: V) V { return V { x = self.x + rhs.x } }
}
@tier(high)
fn from_high(a: V, b: V) V { return a + b }
@tier(mid)
fn from_mid(a: V, b: V) V { return a + b }
@tier(low)
fn from_low(a: V, b: V) V { return a + b }
fn main() void {
    let a = V { x = 1 }
    let b = V { x = 2 }
    println(from_high(a, b).x + from_mid(a, b).x + from_low(a, b).x)
}
"#;
    assert_eq!(out(src), "9\n");
}

// ── Unknown static types and the explicit methods on numbers ────

#[test]
fn closure_parameters_with_no_type_still_reach_the_impl_at_run_time() {
    let src = with_vec2("    let f = fn(p, q) p + q\n    let z = f(a, b)\n    println(z.x)\n    let g = fn(p, q) p + q\n    println(g(2, 3))");
    assert_eq!(out(&src), "4\n5\n");
}

#[test]
fn a_right_operand_not_known_yet_is_decided_by_the_only_impl() {
    let src = r#"
struct V { x: double }
impl Mul<double, V> for V {
    fn mul(self, rhs: double) V { return V { x = self.x * rhs } }
}
fn main() void {
    let v = V { x = 2.0 }
    let g = fn(k) v * k
    println(g(3.0).x)
}
"#;
    assert_eq!(out(src), "6\n");
}

#[test]
fn neg_and_bit_not_can_be_called_by_name_on_numbers_through_a_bound() {
    let src = "fn flip<T: Neg<T>>(a: T) T {\n    return a.neg()\n}\nfn inv<T: BitNot<T>>(a: T) T {\n    return a.bit_not()\n}\nfn main() void {\n    println(flip(7))\n    println(flip(2.5))\n    println(inv(5))\n    println(inv(5u8))\n    println(inv(5u32))\n}\n";
    assert_eq!(out(src), "-7\n-2.5\n-6\n250\n4294967290\n");
}

#[test]
fn every_operator_trait_is_low_so_a_written_high_tier_is_rejected() {
    for (t, m, _) in BINARY {
        let src = format!("struct V {{ x: int }}\nimpl {t}<int, V> for V {{\n    @tier(high)\n    fn {m}(self, rhs: int) V {{ return self }}\n}}\nfn main() void {{\n}}\n");
        assert_eq!(codes(&src), (Stage::Sema, vec!["TIER-016"]), "{t}");
    }
    for (t, m) in [("Neg", "neg"), ("BitNot", "bit_not")] {
        let src = format!("struct V {{ x: int }}\nimpl {t}<V> for V {{\n    @tier(high)\n    fn {m}(self) V {{ return self }}\n}}\nfn main() void {{\n}}\n");
        assert_eq!(codes(&src), (Stage::Sema, vec!["TIER-016"]), "{t}");
    }
}

// ── A field of an expression that already failed ────────────────

#[test]
fn a_field_of_an_expression_that_already_failed_reports_nothing_more() {
    // `await` on a non-task and a dereference of a non-reference give the
    // error type, and the field read after them used to index the type table
    // with it and panic the type checker.
    let awaited = "struct P { x: int }\n@tier(high)\nfn main() void {\n    let n = 5\n    let t = await n\n    println(t.x)\n}\n";
    let (stage, c) = codes(awaited);
    assert!(matches!(stage, Stage::Sema));
    assert_eq!(c.len(), 1, "{c:?}");
    let deref = "struct P { x: int }\nfn main() void {\n    let n = 5\n    let t = *n\n    println(t.x)\n}\n";
    let (stage, c) = codes(deref);
    assert!(matches!(stage, Stage::Sema));
    assert_eq!(c.len(), 1, "{c:?}");
}

#[test]
fn a_field_of_a_reported_operator_reports_nothing_more() {
    let src = with_vec2("    let c = a - b\n    println(c.x)\n    println(c.nothing)");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-136"]));
}

#[test]
fn an_operator_on_an_expression_that_already_failed_reports_nothing_more() {
    // `*n` on a non-reference gives the error type; each operator form with
    // it on either side adds no second error (and none crashes the checker).
    let head = "struct P { x: int }\nstruct V { x: int }\nimpl Add<V, V> for V {\n    fn add(self, rhs: V) V { return self }\n}\nimpl Neg<V> for V {\n    fn neg(self) V { return self }\n}\nfn main() void {\n    let n = 5\n    let t = *n\n    let v = V { x = 1 }\n";
    for body in ["    let a = t + 1", "    let a = 1 + t", "    let a = v + t", "    let a = t + v", "    let a = -t", "    let a = ~t", "    let mut w = v\n    w += t", "    let a = t * t"] {
        let src = format!("{head}{body}\n}}\n");
        let (stage, c) = codes(&src);
        assert!(matches!(stage, Stage::Sema), "{body}: {stage:?}");
        assert_eq!(c.len(), 1, "{body}: {c:?}");
    }
}
