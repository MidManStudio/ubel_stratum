//! Calling things on struct and enum values: methods, associated
//! functions and function-typed fields.
//!
//! Four problems lived here, each a sema/runtime disagreement or a wrong
//! report. The fixtures show whole programs; these tests pin the exact
//! counts and the lookup order that decide them:
//!
//! * an unknown method on an enum with no `extend` block passed sema and
//!   panicked at runtime;
//! * a valid method declared in an `extend` block on an enum passed sema
//!   and ALSO panicked at runtime (the interpreter dispatched user methods
//!   on structs only), and an associated function (`Color.make()`) was
//!   rejected as an unknown variant;
//! * a struct field of function type could not be called (`c.cb(4)` was
//!   NoSuchMethod);
//! * an unknown name written as a call on an enum was reported twice.
//!
//! See docs/PARKED_IDEAS.md, "Found and fixed while taking down the
//! confirmed bugs".

use ubel_stratum::ast::arena::AstArena;
use ubel_stratum::interpreter::Interpreter;
use ubel_stratum_rd::{check_source, Stage};

fn codes(source: &str) -> (Stage, Vec<&'static str>) {
    let report = check_source(source);
    let codes = report.diagnostics.iter().map(|d| d.code).collect();
    (report.stage, codes)
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
    let result = interp.run_program(&program);
    let output = ubel_stratum::builtins::global::io::take_captured_output();
    result.map(|_| output)
}

const COLOR: &str = "enum Color { Red, Green, Rgb(int, int, int) }\n";

// ── Unknown names on an enum ─────────────────────────────────────

#[test]
fn an_unknown_method_on_a_plain_enum_is_no_such_method() {
    let src = format!("{COLOR}fn main() void {{\n    let c = Color.Red\n    println(c.shade())\n}}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-104"]));
}

#[test]
fn an_unknown_method_on_an_enum_with_an_extend_block_is_still_no_such_method() {
    let src = format!("{COLOR}extend Color {{ fn is_red(self) bool {{ return true }} }}\n\
                       fn main() void {{\n    let c = Color.Red\n    println(c.shade())\n}}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-104"]));
}

#[test]
fn an_unknown_associated_function_is_one_unknown_variant_not_two() {
    // The callee `Color.nothing` and the call `Color.nothing()` used to
    // each report it, one typo shown twice.
    let src = format!("{COLOR}fn main() void {{\n    let b = Color.nothing()\n}}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-109"]));
}

#[test]
fn an_unknown_bare_variant_is_one_unknown_variant() {
    let src = format!("{COLOR}fn main() void {{\n    let a = Color.Blue\n}}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-109"]));
}

#[test]
fn a_known_enum_method_with_the_wrong_argument_count_is_one_error() {
    let src = format!("{COLOR}extend Color {{ fn is_red(self) bool {{ return true }} }}\n\
                       fn main() void {{\n    let c = Color.Red\n    let e = c.is_red(1)\n}}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-102"]));
}

// ── Valid enum methods work at runtime ───────────────────────────

#[test]
fn an_instance_method_on_a_unit_variant_runs() {
    let src = format!("{COLOR}extend Color {{\n    fn is_red(self) bool {{\n        match self {{\n\
                       Color.Red => {{ return true }}\n            _ => {{ return false }}\n        }}\n    }}\n}}\n\
                       fn main() void {{\n    println(Color.Red.is_red())\n    println(Color.Green.is_red())\n}}\n");
    assert_eq!(run(&src).unwrap(), "true\nfalse\n");
}

#[test]
fn an_instance_method_on_a_payload_variant_runs() {
    let src = format!("{COLOR}extend Color {{\n    fn total(self) int {{\n        match self {{\n\
                       Color.Rgb(r, g, b) => {{ return r + g + b }}\n            _ => {{ return 0 }}\n        }}\n    }}\n}}\n\
                       fn main() void {{\n    println(Color.Rgb(1, 2, 3).total())\n}}\n");
    assert_eq!(run(&src).unwrap(), "6\n");
}

#[test]
fn an_associated_function_on_an_enum_runs() {
    let src = format!("{COLOR}extend Color {{ fn make() Color {{ return Color.Green }} }}\n\
                       fn main() void {{\n    let g = Color.make()\n    println(g.is_green())\n}}\n\
                       extend Color {{ fn is_green(self) bool {{\n        match self {{\n\
                       Color.Green => {{ return true }}\n            _ => {{ return false }}\n        }}\n    }} }}\n");
    assert_eq!(run(&src).unwrap(), "true\n");
}

#[test]
fn a_method_that_calls_another_method_on_self_runs() {
    let src = format!("{COLOR}extend Color {{\n    fn base(self) int {{ return 10 }}\n\
                       fn louder(self, by: int) int {{ return self.base() + by }}\n}}\n\
                       fn main() void {{ println(Color.Red.louder(5)) }}\n");
    assert_eq!(run(&src).unwrap(), "15\n");
}

// ── Function-typed fields ────────────────────────────────────────

const CONFIG: &str = "struct Config { cb: fn(int) int, name: string }\n";

#[test]
fn a_function_typed_field_can_be_called() {
    let src = format!("{CONFIG}fn main() void {{\n    let c = Config {{ cb = fn(x: int) x * 2, name = \"n\" }}\n    println(c.cb(4))\n}}\n");
    assert_eq!(run(&src).unwrap(), "8\n");
}

#[test]
fn a_field_holding_a_closure_sees_its_captured_local() {
    let src = format!("{CONFIG}fn main() void {{\n    let offset = 100\n\
                       let k = Config {{ cb = fn(x: int) x + offset, name = \"k\" }}\n    println(k.cb(5))\n}}\n");
    assert_eq!(run(&src).unwrap(), "105\n");
}

#[test]
fn a_generic_holder_of_a_function_can_be_called() {
    let src = "struct Holder<T> { f: fn(T) T }\nfn main() void {\n    let h = Holder { f = fn(x: int) x + 1 }\n    println(h.f(4))\n}\n";
    assert_eq!(run(src).unwrap(), "5\n");
}

#[test]
fn a_real_method_wins_over_a_field_of_the_same_name() {
    let src = "struct Both { run: fn(int) int }\nextend Both { fn run(self, n: int) int { return n + 1000 } }\n\
               fn main() void {\n    let b = Both { run = fn(x: int) x * 2 }\n    println(b.run(4))\n}\n";
    assert_eq!(run(src).unwrap(), "1004\n");
}

#[test]
fn the_wrong_argument_type_to_a_function_field_is_one_mismatch() {
    let src = format!("{CONFIG}fn main() void {{\n    let c = Config {{ cb = fn(x: int) x * 2, name = \"n\" }}\n    let r = c.cb(\"text\")\n}}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-101"]));
}

#[test]
fn the_call_result_is_typed_from_the_fields_return_type() {
    let src = format!("{CONFIG}fn main() void {{\n    let c = Config {{ cb = fn(x: int) x * 2, name = \"n\" }}\n    let r: string = c.cb(4)\n}}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-101"]));
}

#[test]
fn calling_a_field_that_is_not_a_function_is_an_error() {
    let src = format!("{CONFIG}fn main() void {{\n    let c = Config {{ cb = fn(x: int) x * 2, name = \"n\" }}\n    let a = c.name(3)\n}}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-104"]));
}

#[test]
fn a_name_that_is_neither_a_method_nor_a_field_is_still_no_such_method() {
    let src = format!("{CONFIG}fn main() void {{\n    let c = Config {{ cb = fn(x: int) x * 2, name = \"n\" }}\n    let n = c.missing()\n}}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-104"]));
}
