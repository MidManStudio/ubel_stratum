//! Programs that passed sema and panicked when they ran.
//!
//! Operator validity for built-in types used to be left to the interpreter:
//! `[1] + [2]`, `true & false`, `1.5 & 2.5`, `-x` for a `u8`, `!5` and a call of
//! a number all type-checked and then stopped with "arithmetic not supported on
//! List", "cannot negate u8" and the like. They are now compile errors:
//!
//! - `TYPE-139` `OperatorNotSupported`: a built-in operator on a built-in type
//!   it is not defined for. The table of what each operator accepts is in
//!   `sema/type_infer/builtin_ops.rs`.
//! - `TYPE-140` `NotCallable`: `n()` for an `int`, a string, a collection.
//! - `TYPE-141` `TupleIndexOutOfRange`: a literal index past the end of a tuple.
//! - `TYPE-142` `NotAStruct`: a struct literal through an alias of `int`.
//!
//! The valid forms stay valid: every test here that expects an error has a
//! sibling that runs the nearest correct program.
//!
//! See docs/DIAGNOSTICS_RULES.md, `TYPE-139` to `TYPE-142`.

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

fn suggestion(source: &str) -> String {
    check_source(source).diagnostics[0].suggestion.clone().unwrap_or_default()
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
    format!("fn main() void {{\n{body}\n}}\n")
}

/// The program must stop at sema with exactly this one code.
fn one(source: &str, code: &str) -> String {
    let (stage, c) = codes(source);
    assert!(matches!(stage, Stage::Sema), "{stage:?} {c:?}\n{source}");
    assert_eq!(c, vec![code], "{:?}\n{source}", messages(source));
    messages(source).remove(0)
}

// ── TYPE-139: binary operators ───────────────────────────────────────────────

/// (body of main, the operator named in the message, the type named in it)
const BAD_BINARY: &[(&str, &str, &str)] = &[
    ("    let c = [1] + [2]\n    println(c.len())",           "+",  "List"),
    ("    println(true + false)",                              "+",  "bool"),
    ("    println('a' + 'b')",                                 "+",  "char"),
    ("    println((1, 2) + (3, 4))",                           "+",  "("),
    ("    println(\"a\" - \"b\")",                             "-",  "string"),
    ("    println(\"a\" * \"b\")",                             "*",  "string"),
    ("    println(true / false)",                              "/",  "bool"),
    ("    println(true % false)",                              "%",  "bool"),
    ("    println(true & false)",                              "&",  "bool"),
    ("    println(true | false)",                              "|",  "bool"),
    ("    println(true ^ false)",                              "^",  "bool"),
    ("    println(1.5 & 2.5)",                                 "&",  "double"),
    ("    println(\"a\" & \"b\")",                             "&",  "string"),
];

#[test]
fn a_binary_operator_on_a_type_it_is_not_defined_for_is_one_error() {
    for (body, op, ty) in BAD_BINARY {
        let src = main_with(body);
        let m = one(&src, "TYPE-139");
        assert!(m.contains(&format!("operator `{op}`")), "{m}\n{src}");
        assert!(m.contains(ty), "{m}\n{src}");
    }
}

#[test]
fn shifts_name_their_own_operator() {
    let m = one(&main_with("    println(1.5 << 2.5)"), "TYPE-139");
    assert!(m.contains("operator `<<`") && m.contains("double"), "{m}");
    let m = one(&main_with("    println(true >> false)"), "TYPE-139");
    assert!(m.contains("operator `>>`") && m.contains("bool"), "{m}");
}

#[test]
fn a_dictionary_sum_is_an_error() {
    let src = "fn main() void {\n    let a: Dictionary<string, int> = Dictionary.new()\n    let b: Dictionary<string, int> = Dictionary.new()\n    println((a + b).len())\n}\n";
    let m = one(src, "TYPE-139");
    assert!(m.contains("Dictionary"), "{m}");
}

#[test]
fn a_rejected_operation_has_no_type_for_what_uses_it() {
    // The result of the bad `+` is not a `List`, so binding it to a `string`
    // adds no second error.
    let src = main_with("    let s: string = [1] + [2]\n    println(s)");
    assert_eq!(codes(&src).1, vec!["TYPE-139"], "{:?}", messages(&src));
    let src = main_with("    let n: int = true & false\n    println(n)");
    assert_eq!(codes(&src).1, vec!["TYPE-139"], "{:?}", messages(&src));
}

#[test]
fn every_built_in_collection_is_rejected() {
    for (decl, ty) in [
        ("let a: Queue<int> = Queue.new()",         "Queue"),
        ("let a: Stack<int> = Stack.new()",         "Stack"),
        ("let a: InlineList<int> = InlineList.new(2)", "InlineList"),
        ("let a: Dictionary<string, int> = Dictionary.new()", "Dictionary"),
    ] {
        let src = main_with(&format!("    {decl}\n    let b = a + a"));
        let m = one(&src, "TYPE-139");
        assert!(m.contains(ty), "{m}\n{src}");
    }
}

#[test]
fn a_fallible_value_is_not_a_number() {
    let src = "fn half(x: int) int! {\n    return x / 2\n}\nfn main() void {\n    let a = half(4) + half(2)\n}\n";
    one(src, "TYPE-139");
}

#[test]
fn the_compound_form_has_the_same_suggestion() {
    assert_eq!(
        suggestion(&main_with("    let mut a = [1]\n    a += [2]\n    println(a.len())")),
        "`+` works on numbers, and on a string with a string"
    );
    assert_eq!(
        suggestion(&main_with("    let mut b = true\n    b &= false\n    println(b)")),
        "for booleans use `and` and `or`"
    );
    assert_eq!(
        suggestion(&main_with("    let mut s = \"a\"\n    s -= \"b\"\n    println(s)")),
        "`-=` works on numbers"
    );
}

#[test]
fn the_error_is_reported_once_and_does_not_cascade() {
    // The result of the bad operation is `unknown`, so what is done with it
    // (`.len()`, an `int` binding) adds nothing.
    let src = main_with("    let c = [1] + [2]\n    let n: int = c.len()\n    println(n)");
    assert_eq!(codes(&src).1, vec!["TYPE-139"], "{:?}", messages(&src));
}

#[test]
fn a_compound_assignment_names_the_compound_operator() {
    let src = main_with("    let mut a = [1]\n    a += [2]\n    println(a.len())");
    let m = one(&src, "TYPE-139");
    assert!(m.contains("operator `+=`"), "{m}");
    let src = main_with("    let mut b = true\n    b &= false\n    println(b)");
    let m = one(&src, "TYPE-139");
    assert!(m.contains("operator `&=`") && m.contains("bool"), "{m}");
}

#[test]
fn a_compound_assignment_on_an_element_is_checked_too() {
    let src = main_with("    let mut xs = [true, false]\n    xs[0] |= true\n    println(xs.len())");
    one(&src, "TYPE-139");
}

#[test]
fn the_suggestion_says_what_the_operator_accepts() {
    assert_eq!(suggestion(&main_with("    println(true + false)")), "`+` works on numbers, and on a string with a string");
    assert_eq!(suggestion(&main_with("    println(\"a\" - \"b\")")), "`-` works on numbers");
    assert_eq!(suggestion(&main_with("    println(1.5 & 2.5)")), "`&` works on integers");
    assert_eq!(suggestion(&main_with("    println(true & false)")), "for booleans use `and` and `or`");
    assert_eq!(suggestion(&main_with("    println(true | false)")), "for booleans use `and` and `or`");
    assert_eq!(suggestion(&main_with("    println(true ^ false)")), "`^` works on integers");
}

// ── TYPE-139: unary operators ────────────────────────────────────────────────

#[test]
fn negating_a_value_that_has_no_negative() {
    for (body, ty) in [
        ("    println(-true)",  "bool"),
        ("    println(-\"a\")", "string"),
        ("    println(-[1])",   "List"),
    ] {
        let src = main_with(body);
        let m = one(&src, "TYPE-139");
        assert!(m.contains("operator `-`") && m.contains(ty), "{m}\n{src}");
    }
}

#[test]
fn negating_an_unsigned_integer() {
    let src = main_with("    let x: u8 = 5\n    println(-x)");
    let m = one(&src, "TYPE-139");
    assert!(m.contains("`u8`"), "{m}");
    assert_eq!(suggestion(&src), "an unsigned integer has no negative; use a signed type");
}

#[test]
fn every_unsigned_type_has_no_negation() {
    for ty in ["u8", "u16", "u32", "u64", "usize", "uint", "ulong"] {
        let src = format!("fn neg(a: {ty}) void {{\n    println(-a)\n}}\nfn main() void {{\n    println(1)\n}}\n");
        let m = one(&src, "TYPE-139");
        assert!(m.contains(&format!("`{ty}`")), "{m}\n{src}");
    }
}

#[test]
fn every_signed_type_has_a_negation() {
    for ty in ["i8", "i16", "i32", "i64", "isize", "int", "long", "double"] {
        let src = format!("fn neg(a: {ty}) void {{\n    println(-a)\n}}\nfn main() void {{\n    println(1)\n}}\n");
        assert!(matches!(check_source(&src).stage, Stage::Clean), "{ty}: {:?}", messages(&src));
    }
}

#[test]
fn the_sized_float_types_have_no_bitwise_operators() {
    for ty in ["f32", "f64", "float", "double"] {
        let src = format!("fn bits(a: {ty}) void {{\n    println(a & a)\n}}\nfn main() void {{\n    println(1)\n}}\n");
        let m = one(&src, "TYPE-139");
        assert!(m.contains(&format!("`{ty}`")), "{m}\n{src}");
    }
}

#[test]
fn negating_an_unsigned_literal() {
    // The old comment on the unary arm said sema accepted this and the
    // interpreter panicked ("cannot negate u8").
    let m = one(&main_with("    println(-5u8)"), "TYPE-139");
    assert!(m.contains("`u8`"), "{m}");
}

#[test]
fn bitwise_not_needs_an_integer() {
    let m = one(&main_with("    println(~1.5)"), "TYPE-139");
    assert!(m.contains("operator `~`") && m.contains("double"), "{m}");
    let m = one(&main_with("    println(~true)"), "TYPE-139");
    assert!(m.contains("operator `~`") && m.contains("bool"), "{m}");
}

#[test]
fn logical_not_needs_a_bool() {
    // `!x` is checked as a condition is: a plain type mismatch.
    for body in ["    println(!5)", "    println(!\"x\")"] {
        let m = one(&main_with(body), "TYPE-101");
        assert!(m.contains("expected `bool`"), "{m}");
    }
}

// ── What stays valid ─────────────────────────────────────────────────────────

#[test]
fn arithmetic_on_numbers_and_strings_runs() {
    let src = main_with("    let a = 7\n    let b = 2\n    println(a + b)\n    println(a - b)\n    println(a * b)\n    println(a / b)\n    println(a % b)\n    println(1.5 + 2.0)\n    println(\"a\" + \"b\")");
    assert_eq!(out(&src), "9\n5\n14\n3\n1\n3.5\nab\n");
}

#[test]
fn arithmetic_on_every_sized_number_runs() {
    let src = main_with("    let a: u8 = 6\n    let b: u8 = 3\n    let c: i64 = 6\n    let d: i64 = 3\n    let e: float = 1.5f\n    let f: float = 1.0f\n    println(a - b)\n    println(c * d)\n    println(e + f)");
    assert_eq!(out(&src), "3\n18\n2.5\n");
}

#[test]
fn bitwise_operators_on_integers_run() {
    let src = main_with("    let a: u8 = 6\n    let b: u8 = 3\n    println(a & b)\n    println(a | b)\n    println(a ^ b)\n    println(a << 1)\n    println(6 >> 1)\n    println(~5)\n    let c: i64 = 4\n    println(c << 2)");
    assert_eq!(out(&src), "2\n7\n5\n12\n3\n-6\n16\n");
}

#[test]
fn unary_operators_on_valid_operands_run() {
    let src = main_with("    let x = 5\n    let f = 2.5\n    let y: i8 = -128\n    println(-x)\n    println(-f)\n    println(!true)\n    println(!(x > 3))\n    println(y)");
    assert_eq!(out(&src), "-5\n-2.5\nfalse\nfalse\n-128\n");
}

#[test]
fn compound_assignment_on_valid_operands_runs() {
    let src = main_with("    let mut a = 1\n    a += 2\n    a *= 3\n    a -= 1\n    a /= 2\n    a %= 3\n    let mut s = \"x\"\n    s += \"y\"\n    let mut f = 1.5\n    f += 1.0\n    let mut b: u32 = 1\n    b <<= 2\n    b |= 1\n    println(a)\n    println(s)\n    println(f)\n    println(b)");
    assert_eq!(out(&src), "1\nxy\n2.5\n5\n");
}

#[test]
fn an_integer_literal_is_a_signed_number() {
    // An unconstrained literal is still an integer of some width.
    assert_eq!(out(&main_with("    println(1 + 2 * 3)\n    println(-4 + 1)\n    println(6 & 3)")), "7\n-3\n2\n");
}

#[test]
fn a_user_type_still_goes_through_its_operator_trait() {
    let src = r#"
struct V { n: int }
impl Add<V, V> for V { fn add(self, rhs: V) V { return V { n = self.n + rhs.n } } }
fn main() void {
    let a = V { n = 1 }
    let b = V { n = 2 }
    println((a + b).n)
}"#;
    assert_eq!(out(src), "3\n");
}

#[test]
fn a_user_type_without_the_impl_is_still_type_136() {
    // Not TYPE-139: a user type is asked for an impl, a built-in type is not.
    let src = "struct V { n: int }\nfn main() void {\n    let a = V { n = 1 }\n    let b = V { n = 2 }\n    println((a - b).n)\n}\n";
    one(src, "TYPE-136");
}

#[test]
fn a_bound_type_parameter_is_untouched() {
    let src = r#"
fn twice<T: Add<T, T>>(x: T) T { return x + x }
fn main() void {
    println(twice(4))
}"#;
    assert_eq!(out(src), "8\n");
}

#[test]
fn comparison_operators_are_not_part_of_this_check() {
    // They have their own rule (TYPE-118); `==` works on any two values of a type.
    assert_eq!(out(&main_with("    println(1 == 1)\n    println(\"a\" < \"b\")\n    println(true == false)")), "true\ntrue\nfalse\n");
}

// ── TYPE-140: calling something that is not a function ───────────────────────

#[test]
fn calling_a_number_is_an_error() {
    let m = one(&main_with("    let n = 5\n    n()"), "TYPE-140");
    assert!(m.contains("cannot call a value of type"), "{m}");
}

#[test]
fn calling_a_typed_value_names_its_type() {
    let m = one(&main_with("    let s = \"x\"\n    s()"), "TYPE-140");
    assert!(m.contains("`string`"), "{m}");
    let m = one(&main_with("    let l: List<int> = List.new()\n    l()"), "TYPE-140");
    assert!(m.contains("List"), "{m}");
    let m = one(&main_with("    let b = true\n    b(1)"), "TYPE-140");
    assert!(m.contains("`bool`"), "{m}");
}

#[test]
fn calling_a_struct_value_is_the_same_error() {
    // It was `TYPE-104`, "no method `<call>`"; a struct value is not a function either.
    let src = "struct P { x: int }\nfn main() void {\n    let p = P { x = 1 }\n    p()\n}\n";
    let m = one(src, "TYPE-140");
    assert!(m.contains("`P`"), "{m}");
}

#[test]
fn calling_a_typed_parameter_is_the_same_error() {
    let src = "fn f(x: int) int {\n    return x(1)\n}\nfn main() void {\n    println(f(3))\n}\n";
    let m = one(src, "TYPE-140");
    assert!(m.contains("`int`"), "{m}");
}

#[test]
fn calling_a_string_field_of_a_struct() {
    let src = "struct C { name: string }\nfn main() void {\n    let c = C { name = \"n\" }\n    let a = c.name(3)\n}\n";
    one(src, "TYPE-140");
}

#[test]
fn calling_a_literal_is_an_error() {
    one(&main_with("    5()"), "TYPE-140");
}

#[test]
fn the_arguments_of_a_bad_call_are_still_checked() {
    // The call is reported, and so is what is wrong inside it.
    let src = main_with("    let n = 5\n    n(1 + \"a\")");
    let (_, c) = codes(&src);
    assert!(c.contains(&"TYPE-140") && c.contains(&"TYPE-101"), "{c:?}");
}

#[test]
fn calling_functions_closures_and_methods_runs() {
    let src = r#"
struct Acc { n: int }
extend Acc { fn plus(self, x: int) int { return self.n + x } }
fn add(x: int, y: int) int { return x + y }
fn main() void {
    let f = fn(x: int) x + 1
    let g = add
    let a = Acc { n = 10 }
    println(f(1))
    println(add(1, 2))
    println(g(3, 4))
    println(a.plus(5))
}"#;
    assert_eq!(out(src), "2\n3\n7\n15\n");
}

// ── TYPE-141: a tuple index past the end ─────────────────────────────────────

#[test]
fn an_index_past_the_end_of_a_tuple_variable() {
    let m = one(&main_with("    let t = (1, 2)\n    println(t[5])"), "TYPE-141");
    assert!(m.contains("tuple index 5") && m.contains("2 element(s)"), "{m}");
    assert_eq!(suggestion(&main_with("    let t = (1, 2)\n    println(t[5])")), "the valid indexes are 0 to 1");
}

#[test]
fn the_suggestion_for_a_tuple_of_one_and_of_none() {
    let one_tuple = main_with("    let t = (1,)\n    println(t[1])");
    assert_eq!(suggestion(&one_tuple), "the valid indexes are 0 to 0");
    let empty = main_with("    let t = ()\n    println(t[0])");
    assert_eq!(suggestion(&empty), "an empty tuple has no elements");
}

#[test]
fn an_index_past_the_end_of_a_tuple_literal() {
    one(&main_with("    println((1, 2)[5])"), "TYPE-141");
}

#[test]
fn the_first_index_past_the_end_is_the_error() {
    one(&main_with("    let t = (1, 2, 3)\n    println(t[3])"), "TYPE-141");
    assert_eq!(out(&main_with("    let t = (1, 2, 3)\n    println(t[2])")), "3\n");
}

#[test]
fn a_negative_tuple_index_is_out_of_range() {
    // A tuple has no from-the-end indexing; the run time wrapped it into a huge index.
    let m = one(&main_with("    let t = (1, 2)\n    println(t[-1])"), "TYPE-141");
    assert!(m.contains("tuple index -1"), "{m}");
}

#[test]
fn every_tuple_element_is_reachable() {
    let src = main_with("    let t = (1, \"a\", 2.5)\n    println(t[0])\n    println(t[1])\n    println(t[2])");
    assert_eq!(out(&src), "1\na\n2.5\n");
}

#[test]
fn a_computed_tuple_index_is_not_checked_here() {
    // Only a literal index has a known value at compile time.
    let src = main_with("    let t = (1, 2)\n    let i = 1\n    println(t[i])");
    assert_eq!(out(&src), "2\n");
}

#[test]
fn a_list_index_is_untouched() {
    // A list has no fixed size; its index is checked when it runs.
    let src = main_with("    let l = [1, 2]\n    println(l[5])");
    assert!(run(&src).unwrap_err().contains("out of bounds"));
}

// ── TYPE-142: a struct literal through an alias of a non-struct ──────────────

#[test]
fn a_struct_literal_through_an_alias_of_int() {
    let src = "type Score = int\nfn main() void {\n    let s = Score { x = 1 }\n    println(1)\n}\n";
    let m = one(src, "TYPE-142");
    assert!(m.contains("`Score` is an alias for `int`"), "{m}");
}

#[test]
fn the_fields_of_the_bad_literal_are_still_checked() {
    let src = "type Score = int\nfn main() void {\n    let s = Score { x = 1 + \"a\" }\n    println(1)\n}\n";
    let (_, c) = codes(src);
    assert!(c.contains(&"TYPE-142") && c.contains(&"TYPE-101"), "{c:?}");
}

#[test]
fn a_struct_literal_through_an_alias_of_a_struct_runs() {
    let src = "struct Point { x: int, y: int }\ntype P = Point\nfn main() void {\n    let p = P { x = 1, y = 2 }\n    println(p.x + p.y)\n}\n";
    assert_eq!(out(src), "3\n");
}

#[test]
fn an_alias_of_a_plain_type_is_still_a_type() {
    let src = "type N = int\nfn main() void {\n    let n: N = 5\n    println(n + 1)\n}\n";
    assert_eq!(out(src), "6\n");
}
