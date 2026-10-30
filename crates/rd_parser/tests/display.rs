//! `Display` and the order a struct prints in, slice S5a.
//!
//! `Display` is a prelude trait with one method, `fn to_string(self) string`.
//! A hand-written impl decides how `println`, `print`, `log`, `to_string`, an
//! interpolation hole `{x}` and the messages of `assert` and `panic` show a
//! value, at any depth inside a struct, an enum, a tuple or a collection. A
//! type with no impl still prints structurally, `{x:?}` never calls a user's
//! `Display`, and the scalar types satisfy `Display` themselves.
//!
//! A struct prints its fields in declaration order (an anonymous object in the
//! order written, an enum's struct variant sorted by name). Before this it
//! printed them in a different order from one run to the next.
//!
//! See docs/TRAITS_DESIGN.md, "What S5a settled".

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

fn assert_clean(source: &str) {
    assert_eq!(codes(source), (Stage::Clean, vec![]), "{source}");
}

/// The first diagnostic code and the stage, for programs that must be rejected.
fn first(source: &str) -> (Stage, &'static str) {
    let (stage, codes) = codes(source);
    (stage, codes.first().copied().unwrap_or("none"))
}

const V: &str = "struct V { x: int, y: int }\n\
                 impl Display for V { fn to_string(self) string { return $\"({self.x}, {self.y})\" } }\n";

// ── The order a struct prints in ─────────────────────────────────────────────

#[test]
fn a_struct_prints_its_fields_in_declaration_order_every_time() {
    // Each struct owns a hash map with its own seed, so thirty of them in one
    // run would show a wrong order if the printer used the map's order.
    let src = "struct R { a: int, b: int, c: int, d: int, e: int, f: int, g: int, h: int }\n\
               fn main() void {\n\
                   let i = 0\n\
                   while (i < 30) {\n\
                       let r = R { h = 8, g = 7, f = 6, e = 5, d = 4, c = 3, b = 2, a = 1 }\n\
                       assert($\"{r}\" == \"R {a: 1, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7, h: 8}\", \"display order\")\n\
                       assert($\"{r:?}\" == \"R {a: 1, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7, h: 8}\", \"debug order\")\n\
                       i = i + 1\n\
                   }\n\
                   println(\"stable\")\n\
               }\n";
    assert_eq!(out(src), "stable\n");
}

#[test]
fn the_declaration_order_is_not_the_alphabetical_order() {
    let src = "struct P { zeta: int, alpha: int, beta: int }\n\
               fn main() void { println(P { beta = 3, alpha = 2, zeta = 1 }) }\n";
    assert_eq!(out(src), "P {zeta: 1, alpha: 2, beta: 3}\n");
}

#[test]
fn debug_prints_fields_in_declaration_order_too() {
    // Declared out of alphabetical order, so sorting would be visible; thirty
    // values, each with its own hash map seed.
    let src = "struct P { zeta: int, alpha: int, beta: int }\n\
               fn main() void {\n\
                   let i = 0\n\
                   while (i < 30) {\n\
                       let p = P { beta = 3, alpha = 2, zeta = 1 }\n\
                       assert($\"{p:?}\" == \"P {zeta: 1, alpha: 2, beta: 3}\", \"debug order\")\n\
                       i = i + 1\n\
                   }\n\
                   println(\"stable\")\n\
               }\n";
    assert_eq!(out(src), "stable\n");
}

#[test]
fn a_nested_struct_prints_every_level_in_order() {
    let src = "struct In { b: int, a: int }\n\
               struct Out { z: In, y: In }\n\
               fn main() void {\n\
                   println(Out { y = In { a = 1, b = 2 }, z = In { a = 3, b = 4 } })\n\
               }\n";
    assert_eq!(out(src), "Out {z: In {b: 4, a: 3}, y: In {b: 2, a: 1}}\n");
}

#[test]
fn an_anonymous_object_prints_in_the_order_written() {
    // Thirty objects, each with its own hash map seed.
    let src = "fn main() void {\n\
                   let i = 0\n\
                   while (i < 30) {\n\
                       let o = { zed = 1, apple = 2, mid_ = 3, beta = 4 }\n\
                       assert($\"{o}\" == \"<anon> {zed: 1, apple: 2, mid_: 3, beta: 4}\", \"anonymous order\")\n\
                       i = i + 1\n\
                   }\n\
                   println(\"stable\")\n\
               }\n";
    assert_eq!(out(src), "stable\n");
}

#[test]
fn an_enum_struct_variant_prints_its_fields_sorted_by_name() {
    // The runtime keeps no declaration order for a variant's fields, so they are
    // sorted; thirty values, each with its own hash map seed, in both forms.
    let src = "enum Shape { Rect { w: int, h: int, d: int, a: int } }\n\
               fn main() void {\n\
                   let i = 0\n\
                   while (i < 30) {\n\
                       let r = Shape.Rect { w = 2, h = 3, d = 4, a = 5 }\n\
                       assert($\"{r}\" == \"Shape.Rect {a: 5, d: 4, h: 3, w: 2}\", \"display\")\n\
                       assert($\"{r:?}\" == \"Shape.Rect {a: 5, d: 4, h: 3, w: 2}\", \"debug\")\n\
                       i = i + 1\n\
                   }\n\
                   println(\"stable\")\n\
               }\n";
    assert_eq!(out(src), "stable\n");
}

// ── A hand-written Display is used wherever a value is shown ─────────────────

#[test]
fn println_and_interpolation_use_the_impl() {
    let src = format!(
        "{V}fn main() void {{\n\
             let v = V {{ x = 1, y = 2 }}\n\
             println(v)\n\
             println($\"v is {{v}}\")\n\
         }}\n"
    );
    assert_eq!(out(&src), "(1, 2)\nv is (1, 2)\n");
}

#[test]
fn width_and_alignment_apply_to_the_text() {
    let src = format!(
        "{V}fn main() void {{\n\
             let v = V {{ x = 1, y = 2 }}\n\
             println($\"[{{v:>10}}]\")\n\
             println($\"[{{v:<10}}]\")\n\
             println($\"[{{v:^10}}]\")\n\
         }}\n"
    );
    assert_eq!(out(&src), "[    (1, 2)]\n[(1, 2)    ]\n[  (1, 2)  ]\n");
}

#[test]
fn debug_stays_structural() {
    let src = format!(
        "{V}fn main() void {{\n\
             let v = V {{ x = 1, y = 2 }}\n\
             println($\"{{v:?}}\")\n\
         }}\n"
    );
    assert_eq!(out(&src), "V {x: 1, y: 2}\n");
}

#[test]
fn to_string_as_a_method_and_as_a_function_use_the_impl() {
    let src = format!(
        "{V}fn main() void {{\n\
             let v = V {{ x = 1, y = 2 }}\n\
             println(v.to_string())\n\
             println(to_string(v))\n\
             let s = v.to_string() + \"!\"\n\
             println(s)\n\
         }}\n"
    );
    assert_eq!(out(&src), "(1, 2)\n(1, 2)\n(1, 2)!\n");
}

#[test]
fn several_arguments_are_each_shown() {
    let src = format!(
        "{V}fn main() void {{\n\
             println(1, V {{ x = 3, y = 4 }}, \"end\")\n\
         }}\n"
    );
    assert_eq!(out(&src), "1 (3, 4) end\n");
}

#[test]
fn a_struct_with_no_impl_shows_a_field_that_has_one() {
    let src = format!(
        "{V}struct Body {{ name: string, pos: V, mass: int }}\n\
         fn main() void {{\n\
             println(Body {{ name = \"rock\", pos = V {{ x = 1, y = 2 }}, mass = 5 }})\n\
         }}\n"
    );
    assert_eq!(out(&src), "Body {name: rock, pos: (1, 2), mass: 5}\n");
}

#[test]
fn an_impl_on_one_type_leaves_every_other_type_alone() {
    let src = format!(
        "{V}struct Plain {{ a: int, b: int }}\n\
         fn main() void {{\n\
             println(Plain {{ a = 1, b = 2 }})\n\
             println(5)\n\
             println(\"s\")\n\
         }}\n"
    );
    assert_eq!(out(&src), "Plain {a: 1, b: 2}\n5\ns\n");
}

#[test]
fn an_impl_can_use_another_types_display() {
    let src = format!(
        "{V}struct Seg {{ a: V, b: V }}\n\
         impl Display for Seg {{ fn to_string(self) string {{ return $\"{{self.a}} -> {{self.b}}\" }} }}\n\
         fn main() void {{\n\
             println(Seg {{ a = V {{ x = 0, y = 0 }}, b = V {{ x = 1, y = 1 }} }})\n\
         }}\n"
    );
    assert_eq!(out(&src), "(0, 0) -> (1, 1)\n");
}

#[test]
fn an_enum_can_have_a_display_and_its_payload_values_are_shown() {
    let src = format!(
        "{V}enum Shape {{ Dot(V), Named {{ at: V }} }}\n\
         enum Mark {{ Pin(V) }}\n\
         impl Display for Mark {{ fn to_string(self) string {{ return \"mark\" }} }}\n\
         fn main() void {{\n\
             let v = V {{ x = 1, y = 2 }}\n\
             println(Shape.Dot(v))\n\
             println(Shape.Named {{ at = v }})\n\
             println(Mark.Pin(v))\n\
         }}\n"
    );
    assert_eq!(out(&src), "Shape.Dot((1, 2))\nShape.Named {at: (1, 2)}\nmark\n");
}

#[test]
fn values_inside_collections_tuples_and_wrappers_are_shown() {
    let src = format!(
        "{V}fn main() void {{\n\
             let v = V {{ x = 1, y = 2 }}\n\
             let l = List.new()\n\
             l.push(v)\n\
             l.push(V {{ x = 3, y = 4 }})\n\
             println(l)\n\
             let t = (v, 7)\n\
             println(t)\n\
             let d = Dictionary.new()\n\
             d.set(\"k\", v)\n\
             println(d)\n\
             let q = Queue.new()\n\
             q.enqueue(v)\n\
             println(q)\n\
             let s = Stack.new()\n\
             s.push(v)\n\
             println(s)\n\
         }}\n"
    );
    assert_eq!(out(&src), "[(1, 2), (3, 4)]\n((1, 2), 7)\n{k: (1, 2)}\nQueue[(1, 2)]\nStack[(1, 2)]\n");
}

#[test]
fn the_shapes_of_collections_are_unchanged_by_the_substitution() {
    // The same containers holding a type with a Display and holding the text
    // that Display produces must print identically.
    fn program(item: &str, with_impl: bool) -> String {
        let decl = if with_impl {
            "struct V { n: int }\n\
             impl Display for V { fn to_string(self) string { return \"<v>\" } }\n"
        } else {
            ""
        };
        format!(
            "{decl}fn main() void {{\n\
                 let item = {item}\n\
                 let l = List.new()\n\
                 l.push(item)\n\
                 l.push(item)\n\
                 println(l)\n\
                 let t = (item, 1, item)\n\
                 println(t)\n\
                 let d = Dictionary.new()\n\
                 d.set(\"a\", item)\n\
                 println(d)\n\
                 let q = Queue.new()\n\
                 q.enqueue(item)\n\
                 println(q)\n\
                 let st = Stack.new()\n\
                 st.push(item)\n\
                 println(st)\n\
                 let il = InlineList.new(3)\n\
                 il.push(item)\n\
                 println(il)\n\
             }}\n"
        )
    }
    assert_eq!(out(&program("V { n = 1 }", true)), out(&program("\"<v>\"", false)));
}

#[test]
fn assert_and_panic_messages_use_the_impl() {
    let src = format!(
        "{V}fn main() void {{\n\
             assert(false, V {{ x = 9, y = 8 }})\n\
         }}\n"
    );
    let err = run(&src).unwrap_err();
    assert!(err.contains("(9, 8)"), "{err}");
}

#[test]
fn a_program_with_no_impl_prints_as_before() {
    let src = "struct P { a: int, b: string }\n\
               fn main() void {\n\
                   println(P { a = 1, b = \"x\" })\n\
                   println(List.new())\n\
                   let t = (1, \"two\")\n\
                   println(t)\n\
               }\n";
    assert_eq!(out(src), "P {a: 1, b: x}\n[]\n(1, two)\n");
}

// ── The scalars satisfy Display themselves ───────────────────────────────────

#[test]
fn a_bounded_function_accepts_every_scalar_and_a_type_with_an_impl() {
    let src = format!(
        "{V}fn show<T: Display>(x: T) string {{ return x.to_string() }}\n\
         fn main() void {{\n\
             println(show(5))\n\
             println(show(2.5))\n\
             println(show(true))\n\
             println(show('c'))\n\
             println(show(\"s\"))\n\
             println(show(V {{ x = 1, y = 2 }}))\n\
         }}\n"
    );
    assert_eq!(out(&src), "5\n2.5\ntrue\nc\ns\n(1, 2)\n");
}

#[test]
fn to_string_is_a_method_of_every_scalar() {
    let src = "fn main() void {\n\
                   let n = 7\n\
                   let f = 2.5\n\
                   let b = true\n\
                   let c = 'x'\n\
                   let s = \"text\"\n\
                   let u: u8 = 200\n\
                   println(n.to_string())\n\
                   println(f.to_string())\n\
                   println(b.to_string())\n\
                   println(c.to_string())\n\
                   println(s.to_string())\n\
                   println(u.to_string())\n\
               }\n";
    assert_eq!(out(src), "7\n2.5\ntrue\nx\ntext\n200\n");
}

#[test]
fn a_type_with_no_impl_does_not_satisfy_the_bound() {
    assert_eq!(
        first("struct P { n: int }\n\
               fn show<T: Display>(x: T) string { return x.to_string() }\n\
               fn main() void { println(show(P { n = 1 })) }\n"),
        (Stage::Sema, "TYPE-126"),
    );
}

#[test]
fn a_collection_a_tuple_and_an_optional_do_not_satisfy_the_bound() {
    for value in ["List.new()", "(1, 2)"] {
        let src = format!(
            "fn show<T: Display>(x: T) string {{ return x.to_string() }}\n\
             fn main() void {{\n let v = {value}\n println(show(v))\n}}\n"
        );
        assert_eq!(first(&src).0, Stage::Sema, "{value}");
    }
}

#[test]
fn a_type_with_no_impl_has_no_to_string() {
    assert_eq!(
        first("struct P { n: int }\n\
               fn main() void { let p = P { n = 1 }\n println(p.to_string()) }\n"),
        (Stage::Sema, "TYPE-104"),
    );
}

#[test]
fn to_string_is_a_method_of_every_sized_scalar() {
    let src = "fn main() void {\n\
                   let a: i8 = 1\n\
                   let b: i16 = 2\n\
                   let c: i32 = 3\n\
                   let d: u16 = 4\n\
                   let e: u32 = 5\n\
                   let f: u64 = 6\n\
                   let g = 1.5 as f32\n\
                   let h = 2.5 as f64\n\
                   println(a.to_string() + b.to_string() + c.to_string() + d.to_string())\n\
                   println(e.to_string() + f.to_string() + g.to_string() + h.to_string())\n\
               }\n";
    assert_eq!(out(src), "1234\n561.52.5\n");
}

#[test]
fn a_qualified_call_shows_a_scalar_and_a_struct() {
    let src = format!(
        "{V}fn main() void {{\n\
             let n = 5\n\
             let s = \"str\"\n\
             let f = 2.5\n\
             println(Display.to_string(n))\n\
             println(Display.to_string(s))\n\
             println(Display.to_string(f))\n\
             println(Display.to_string(V {{ x = 1, y = 2 }}))\n\
         }}\n"
    );
    assert_eq!(out(&src), "5\nstr\n2.5\n(1, 2)\n");
}

#[test]
fn print_and_panic_use_the_impl() {
    let src = format!(
        "{V}fn main() void {{\n\
             print(V {{ x = 1, y = 2 }})\n\
             print(V {{ x = 3, y = 4 }})\n\
             println(\"\")\n\
         }}\n"
    );
    assert_eq!(out(&src), "(1, 2)(3, 4)\n");
    let panics = format!("{V}fn main() void {{ panic(V {{ x = 7, y = 8 }}) }}\n");
    let err = run(&panics).unwrap_err();
    assert!(err.contains("(7, 8)"), "{err}");
}

#[test]
fn a_dictionary_key_is_shown_by_the_impl() {
    let src = "@derive(PartialEq, Eq, Hash)\n\
               struct K { id: int }\n\
               impl Display for K { fn to_string(self) string { return $\"k{self.id}\" } }\n\
               fn main() void {\n\
                   let d = Dictionary.new()\n\
                   d.set(K { id = 1 }, 10)\n\
                   println(d)\n\
               }\n";
    assert_eq!(out(src), "{k1: 10}\n");
}

#[test]
fn the_ownership_wrappers_show_what_they_hold() {
    // The same wrappers around a value with a Display and around the text it
    // produces must print identically.
    fn program(item: &str, with_impl: bool) -> String {
        let decl = if with_impl {
            "struct V { x: int }\n\
             impl Display for V { fn to_string(self) string { return \"<v>\" } }\n"
        } else {
            ""
        };
        format!(
            "{decl}@tier(low) fn go() void {{\n\
                 let u = Unique.new({item})\n\
                 println(u)\n\
                 let s = Shared.new({item})\n\
                 println(s)\n\
                 let y = SyncShared.new({item})\n\
                 println(y)\n\
             }}\n\
             fn main() void {{ go() }}\n"
        )
    }
    let shown = out(&program("V { x = 1 }", true));
    assert_eq!(shown, out(&program("\"<v>\"", false)));
    assert_eq!(shown, "Unique(<v>)\nShared(<v>)\nSyncShared(<v>)\n");
}

// ── Impl rules ───────────────────────────────────────────────────────────────

#[test]
fn an_impl_for_a_scalar_is_a_duplicate_of_the_built_in_one() {
    assert_eq!(
        first("impl Display for int { fn to_string(self) string { return \"i\" } }\n\
               fn main() void { println(1) }\n"),
        (Stage::Sema, "TYPE-128"),
    );
}

#[test]
fn an_impl_for_a_collection_is_reported_with_a_display_message() {
    let src = "impl Display for List<int> { fn to_string(self) string { return \"l\" } }\n\
               fn main() void { println(1) }\n";
    assert_eq!(first(src), (Stage::Sema, "TYPE-129"));
    let msg = messages(src).join("\n");
    assert!(msg.contains("printed element by element"), "{msg}");
}

#[test]
fn an_impl_for_an_array_is_reported_and_lists_print_as_before() {
    let src = "impl Display for [3]int { fn to_string(self) string { return \"arr\" } }\n\
               fn main() void { let l = [1, 2, 3]\n println(l) }\n";
    assert_eq!(first(src), (Stage::Sema, "TYPE-129"));
    let msg = messages(src).join("\n");
    assert!(msg.contains("only a struct or an enum"), "{msg}");
    // The rejected impl changes nothing about how a list prints.
    assert_eq!(out("fn main() void { let l = [1, 2, 3]\n println(l) }\n"), "[1, 2, 3]\n");
}

#[test]
fn the_method_has_to_match_the_trait() {
    assert_eq!(
        first("struct V { x: int }\n\
               impl Display for V { fn to_string(self) int { return 1 } }\n\
               fn main() void { println(1) }\n"),
        (Stage::Sema, "TYPE-125"),
    );
    assert_eq!(
        first("struct V { x: int }\n\
               impl Display for V { }\n\
               fn main() void { println(1) }\n"),
        (Stage::Sema, "TYPE-123"),
    );
}

#[test]
fn display_is_not_derivable() {
    assert_eq!(
        first("@derive(Display)\n\
               struct V { x: int }\n\
               fn main() void { println(1) }\n").0,
        Stage::Sema,
    );
}

#[test]
fn a_programs_own_trait_named_display_takes_the_name() {
    // The prelude's `Display` is dropped when a program declares its own, and
    // printing stays structural.
    let src = "trait Display { fn show(self) string }\n\
               struct V { x: int }\n\
               impl Display for V { fn show(self) string { return \"shown\" } }\n\
               fn main() void {\n\
                   println(V { x = 1 })\n\
                   println(V { x = 1 }.show())\n\
               }\n";
    assert_eq!(out(src), "V {x: 1}\nshown\n");
}

// ── Tiers: Display is a LOW trait like the rest of the prelude ───────────────

#[test]
fn an_impl_that_writes_no_tier_is_low() {
    assert_clean(
        "struct V { x: int }\n\
         impl Display for V { fn to_string(self) string { return \"v\" } }\n\
         @tier(low) fn show(v: V) string { return v.to_string() }\n\
         fn main() void { }\n",
    );
}

#[test]
fn a_written_tier_above_low_is_a_mismatch() {
    assert_eq!(
        first("struct V { x: int }\n\
               impl Display for V { @tier(high) fn to_string(self) string { return \"v\" } }\n\
               fn main() void { }\n"),
        (Stage::Sema, "TIER-016"),
    );
}

#[test]
fn the_body_of_an_impl_is_checked_at_low() {
    assert_eq!(
        first("fn noisy() string { return \"x\" }\n\
               struct V { x: int }\n\
               impl Display for V { fn to_string(self) string { return noisy() } }\n\
               fn main() void { }\n"),
        (Stage::Sema, "TIER-005"),
    );
}

#[test]
fn code_at_any_tier_can_call_to_string_through_the_bound() {
    for tier in ["low", "mid", "high"] {
        let src = format!(
            "@tier({tier}) fn show<T: Display>(x: T) string {{ return x.to_string() }}\n\
             fn main() void {{ }}\n"
        );
        assert_clean(&src);
    }
}
