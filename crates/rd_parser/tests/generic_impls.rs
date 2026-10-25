//! Impls and `extend` blocks for generic types, slice S3e.
//!
//! `impl<T: Show> Show for Box<T>`, `impl Show for Box<int>`,
//! `extend<T> List<T> { .. }`. Each impl parameter occupies one argument of
//! the head; an argument written as a concrete type is a concrete position;
//! bounds written on the parameters make the impl conditional. The generic
//! built-in heads that take impls are `List`, `Dictionary`, `Queue`, `Stack`
//! and `InlineList`. Generics are erased at run time, so a trait has one impl
//! per head and a method name one inherent definition per head.
//!
//! The `.ubl` fixtures show whole programs accepted or rejected. These tests
//! pin the exact diagnostics and the run-time output.
//!
//! See docs/TRAITS_DESIGN.md, "What S3e settled".

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

fn positions(source: &str) -> Vec<(usize, usize)> {
    check_source(source)
        .diagnostics
        .iter()
        .map(|d| (d.primary_span.line as usize, d.primary_span.column as usize))
        .collect()
}

fn pos_of(source: &str, needle: &str) -> (usize, usize) {
    let at = source.find(needle).unwrap_or_else(|| panic!("`{needle}` not in the source"));
    let before = &source[..at];
    let line = before.matches('\n').count() + 1;
    let column = at - before.rfind('\n').map_or(0, |i| i + 1) + 1;
    (line, column)
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

/// A trait, two generic structs, a struct without `Show`, and `Show` for the
/// primitives the examples use.
const PRELUDE: &str = "trait Show { fn show(self) string }\n\
    struct Box<T> { v: T }\n\
    struct Pair<A, B> { a: A, b: B }\n\
    struct Plain { n: int }\n\
    impl Show for int { fn show(self) string { return $\"i{self}\" } }\n\
    impl Show for string { fn show(self) string { return self } }\n";

fn with(body: &str) -> String {
    format!("{PRELUDE}{body}")
}

const CLEAN: (Stage, Vec<&str>) = (Stage::Clean, Vec::new());

const IMPL_BOX: &str = "impl<T: Show> Show for Box<T> { fn show(self) string { return \"Box(\" + self.v.show() + \")\" } }\n";

// ── Parameters, scope and what they bind ─────────────────────────

#[test]
fn an_impl_for_a_generic_struct_binds_its_parameter_in_the_bodies() {
    let src = with(&format!("{IMPL_BOX}fn main() void {{\n    println(Box {{ v = 4 }}.show())\n    println(Box {{ v = \"s\" }}.show())\n}}\n"));
    assert_eq!(run(&src).unwrap(), "Box(i4)\nBox(s)\n");
}

#[test]
fn an_extend_block_takes_parameters_the_same_way() {
    let src = with("extend<T> Box<T> { fn get(self) T { return self.v } }\n\
                     fn main() void { println(Box { v = 5 }.get() + 1) }\n");
    assert_eq!(run(&src).unwrap(), "6\n");
}

#[test]
fn an_inherent_impl_block_takes_parameters_too() {
    let src = with("impl<T> Box<T> { fn get(self) T { return self.v } }\n\
                     fn main() void { println(Box { v = \"x\" }.get()) }\n");
    assert_eq!(run(&src).unwrap(), "x\n");
}

#[test]
fn a_parameter_is_numbered_by_the_place_it_occupies_not_the_order_it_is_declared() {
    // `X` is declared first but sits at the second argument of `Pair`.
    let src = with("impl<X: Show, Y: Show> Show for Pair<Y, X> {\n\
                         fn show(self) string { return self.a.show() + \",\" + self.b.show() }\n}\n\
                     fn main() void { println(Pair { a = 1, b = \"x\" }.show()) }\n");
    assert_eq!(run(&src).unwrap(), "i1,x\n");
}

#[test]
fn a_parameter_name_inside_the_body_means_the_place_it_occupies() {
    // `Y` is declared second but sits at the first argument of `Pair`, so a
    // local written `let first: Y = self.a` is the first field's type.
    let src = with("impl<X: Show, Y: Show> Show for Pair<Y, X> {\n    fn show(self) string {\n        let first: Y = self.a\n        let second: X = self.b\n        return first.show() + second.show()\n    }\n}\n\
                    fn main() void { println(Pair { a = 1, b = \"x\" }.show()) }\n");
    assert_eq!(run(&src).unwrap(), "i1x\n");
}

#[test]
fn an_enum_head_takes_a_generic_impl() {
    let src = with("enum Opt<T> { Some(T), None }\n\
                     impl<T: Show> Show for Opt<T> {\n    fn show(self) string {\n        match self {\n            Opt.Some(v) => { return \"Some \" + v.show() }\n            Opt.None => { return \"None\" }\n        }\n    }\n}\n\
                     fn main() void {\n    let n: Opt<int> = Opt.None\n    println(Opt.Some(3).show())\n    println(n.show())\n}\n");
    assert_eq!(run(&src).unwrap(), "Some i3\nNone\n");
}

#[test]
fn a_method_in_a_generic_block_keeps_its_own_generic_parameters() {
    let src = with("extend<A, B> Pair<A, B> {\n    fn first<U>(self, u: U) U { return u }\n    fn swap(self) Pair<B, A> { return Pair { a = self.b, b = self.a } }\n}\n\
                     fn main() void {\n    let p = Pair { a = 1, b = \"x\" }\n    println(p.first(9))\n    println(p.swap().b)\n}\n");
    assert_eq!(run(&src).unwrap(), "9\n1\n");
}

#[test]
fn an_associated_function_in_a_generic_block_is_called_on_the_type() {
    let src = with("extend<T> Box<T> { fn wrap(v: T) Box<T> { return Box { v = v } } }\n\
                     fn main() void { println(Box.wrap(8).v) }\n");
    assert_eq!(run(&src).unwrap(), "8\n");
}

#[test]
fn self_in_a_generic_block_is_the_head_applied_to_the_parameters() {
    let src = with("trait Same { fn same(self) Self }\n\
                     impl<T> Same for Box<T> { fn same(self) Self { return self } }\n\
                     fn main() void { println(Box { v = 2 }.same().v) }\n");
    assert_eq!(run(&src).unwrap(), "2\n");
}

#[test]
fn an_impl_parameter_is_named_in_diagnostics_inside_the_block() {
    let src = with("impl<T> Show for Box<T> { fn show(self) string { return self.v.show() } }\nfn main() void { }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-130"]));
    assert_eq!(messages(&src), vec![
        "cannot call `show` on a value of type parameter `T`: it has no trait bound",
    ]);
}

// ── Bounds: conditional impls ────────────────────────────────────

#[test]
fn a_conditional_impl_is_used_through_a_bound_and_a_qualified_call() {
    let src = with(&format!("{IMPL_BOX}fn via<X: Show>(x: X) string {{ return x.show() }}\n\
        fn main() void {{\n    println(via(Box {{ v = 1 }}))\n    println(Show.show(Box {{ v = Box {{ v = 2 }} }}))\n}}\n"));
    assert_eq!(run(&src).unwrap(), "Box(i1)\nBox(Box(i2))\n");
}

#[test]
fn a_conditional_impl_does_not_apply_when_the_argument_lacks_the_bound() {
    let src = with(&format!("{IMPL_BOX}fn via<X: Show>(x: X) string {{ return x.show() }}\n\
        fn main() void {{ println(via(Box {{ v = Plain {{ n = 1 }} }})) }}\n"));
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Show` is not implemented for `Box<Plain>`"]);
}

#[test]
fn a_method_call_on_a_conditional_impl_reports_the_argument_that_lacks_the_bound() {
    let src = with(&format!("{IMPL_BOX}fn main() void {{ println(Box {{ v = Plain {{ n = 1 }} }}.show()) }}\n"));
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Show` is not implemented for `Plain`"]);
    // Reported at the call, on the line the call is on.
    assert_eq!(positions(&src).iter().map(|p| p.0).collect::<Vec<_>>(), vec![pos_of(&src, "Box { v = Plain { n = 1 } }.show").0]);
}

#[test]
fn a_structs_own_bound_must_be_restated_on_the_impl_parameter() {
    // The body uses what the struct promises (`self.v.show()`), which holds
    // inside the block, so the missing bound is the only mistake reported.
    let src = with("struct Need<T: Show> { v: T }\n\
                    impl<U> Show for Need<U> { fn show(self) string { return self.v.show() } }\nfn main() void { }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Show` is not implemented for `U`"]);
    assert_eq!(suggestions(&src), vec![Some("declare the bound where `U` is declared, as in `<U: Show>`".to_string())]);
}

#[test]
fn a_structs_own_bound_restated_on_the_impl_parameter_is_accepted() {
    let src = with("struct Need<T: Show> { v: T }\n\
                    impl<U: Show> Show for Need<U> { fn show(self) string { return self.v.show() } }\n\
                    fn main() void { println(Need { v = 3 }.show()) }\n");
    assert_eq!(run(&src).unwrap(), "i3\n");
}

#[test]
fn a_structs_own_bound_is_checked_on_a_concrete_argument() {
    let src = with("struct Need<T: Show> { v: T }\n\
                    impl Show for Need<Plain> { fn show(self) string { return \"n\" } }\nfn main() void { }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Show` is not implemented for `Plain`"]);
}

#[test]
fn a_bound_on_an_impl_parameter_may_have_arguments_and_name_other_parameters() {
    let src = with("trait Convert<T> { fn convert(self) T }\n\
                    impl Convert<int> for string { fn convert(self) int { return 7 } }\n\
                    impl<X: Convert<int>> Show for Box<X> { fn show(self) string { return $\"c{self.v.convert()}\" } }\n\
                    fn main() void { println(Box { v = \"s\" }.show()) }\n");
    assert_eq!(run(&src).unwrap(), "c7\n");
}

#[test]
fn a_dictionary_key_parameter_must_be_hash_on_the_impl() {
    let src = with("trait Size { fn size(self) int }\n\
                    impl<K, V> Size for Dictionary<K, V> { fn size(self) int { return 1 } }\nfn main() void { }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Hash` is not implemented for `K`"]);
}

#[test]
fn a_dictionary_key_parameter_with_hash_is_accepted() {
    let src = with("trait Size { fn size(self) int }\n\
                    impl<K: Hash, V> Size for Dictionary<K, V> { fn size(self) int { return self.len() * 2 } }\n\
                    fn main() void {\n    let mut d = Dictionary.new()\n    d.set(\"a\", 1)\n    d.set(\"b\", 2)\n    println(d.size())\n}\n");
    assert_eq!(run(&src).unwrap(), "4\n");
}

#[test]
fn a_qualified_call_checks_the_impls_conditions_on_the_receiver() {
    let src = with(&format!("{IMPL_BOX}fn main() void {{ println(Show.show(Box {{ v = Plain {{ n = 1 }} }})) }}\n"));
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Show` is not implemented for `Box<Plain>`"]);
}

#[test]
fn an_associated_function_in_a_conditional_block_checks_its_bounds() {
    let src = with("extend<T: Show> Box<T> { fn wrap(v: T) Box<T> { return Box { v = v } } }\n\
                    fn main() void { println(Box.wrap(Plain { n = 1 }).v.n) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Show` is not implemented for `Plain`"]);
}

// ── Supertraits and prelude traits on generic impls ──────────────

#[test]
fn a_supertrait_impl_that_asks_no_more_than_the_impl_does_is_found() {
    let src = with(&format!("{IMPL_BOX}trait Loud: Show {{ fn loud(self) string }}\n\
        impl<T: Show> Loud for Box<T> {{ fn loud(self) string {{ return self.show() + \"!\" }} }}\n\
        fn main() void {{ println(Box {{ v = 4 }}.loud()) }}\n"));
    assert_eq!(run(&src).unwrap(), "Box(i4)!\n");
}

#[test]
fn a_supertrait_impl_that_asks_more_than_the_impl_does_is_missing() {
    let src = with(&format!("{IMPL_BOX}trait Loud: Show {{ fn loud(self) string }}\n\
        impl<T> Loud for Box<T> {{ fn loud(self) string {{ return \"l\" }} }}\nfn main() void {{ }}\n"));
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-132"]));
    assert_eq!(messages(&src), vec![
        "`impl Loud for Box<T>` needs `Show`, which `Box<T>` does not implement",
    ]);
}

#[test]
fn a_prelude_trait_written_by_hand_for_a_generic_struct_drives_the_operator() {
    let src = with("impl<T: PartialEq> PartialEq for Box<T> { fn eq(self, o: Box<T>) bool { return self.v == o.v } }\n\
                    fn same<X: PartialEq>(a: X, b: X) bool { return a == b }\n\
                    fn main() void {\n    println(Box { v = 1 } == Box { v = 1 })\n    println(Box { v = 1 } == Box { v = 2 })\n    println(same(Box { v = 3 }, Box { v = 3 }))\n}\n");
    assert_eq!(run(&src).unwrap(), "true\nfalse\ntrue\n");
}

#[test]
fn a_conditional_prelude_impl_is_not_there_for_an_argument_without_the_bound() {
    let src = with("impl<T: PartialEq> PartialEq for Box<T> { fn eq(self, o: Box<T>) bool { return self.v == o.v } }\n\
                    fn same<X: PartialEq>(a: X, b: X) bool { return a == b }\n\
                    fn main() void { println(same(Box { v = Plain { n = 1 } }, Box { v = Plain { n = 1 } })) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `PartialEq` is not implemented for `Box<Plain>`"]);
}

#[test]
fn a_conditional_prelude_impl_waits_for_an_argument_that_is_not_known_yet() {
    // The argument of `b` is only decided by the last line, so whether `Box<T>`
    // is `PartialEq` is not answered when `same(b, b)` is checked.
    let src = with("impl<T: PartialEq> PartialEq for Box<T> { fn eq(self, o: Box<T>) bool { return self.v == o.v } }\n\
                    fn make<T>() T { return make() }\n\
                    fn same<X: PartialEq>(a: X, b: X) bool { return a == b }\n\
                    fn main() void {\n    let b = Box { v = make() }\n    println(same(b, b))\n    let z: int = b.v\n}\n");
    assert_eq!(codes(&src), CLEAN);
}

#[test]
fn a_hand_written_partial_ord_for_a_generic_struct_makes_it_orderable() {
    // `PartialOrd` builds on `PartialEq`, which the struct derives.
    let src = "@derive(PartialEq)\nstruct W<T> { v: T }\n\
               impl<T: PartialOrd> PartialOrd for W<T> {\n    fn partial_cmp(self, o: W<T>) Ordering? {\n        if self.v < o.v { return Ordering.Less }\n        if o.v < self.v { return Ordering.Greater }\n        return Ordering.Equal\n    }\n}\n\
               fn main() void {\n    println(W { v = 1 } < W { v = 2 })\n    println(W { v = 3 } < W { v = 2 })\n}\n";
    assert_eq!(run(src).unwrap(), "true\nfalse\n");
}

#[test]
fn a_conditional_partial_ord_is_not_there_for_an_argument_that_cannot_be_ordered() {
    let src = "@derive(PartialEq)\nstruct W<T> { v: T }\nstruct Plain { n: int }\n\
               impl<T: PartialOrd> PartialOrd for W<T> {\n    fn partial_cmp(self, o: W<T>) Ordering? { return Ordering.Equal }\n}\n\
               fn main() void { println(W { v = Plain { n = 1 } } < W { v = Plain { n = 2 } }) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-118"]));
}

#[test]
fn a_derive_and_a_hand_written_impl_for_a_generic_struct_overlap() {
    let src = "trait Show { fn show(self) string }\n@derive(PartialEq)\nstruct Box<T> { v: T }\n\
               impl<T: PartialEq> PartialEq for Box<T> { fn eq(self, o: Box<T>) bool { return true } }\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-128"]));
    assert_eq!(messages(src), vec!["`PartialEq` is implemented for `Box<T>` more than once"]);
}

// ── Concrete arguments ───────────────────────────────────────────

#[test]
fn an_impl_for_one_instance_applies_to_that_instance_only() {
    let src = with("impl Show for Box<int> { fn show(self) string { return $\"intbox {self.v + 1}\" } }\n\
                    fn via<X: Show>(x: X) string { return x.show() }\n\
                    fn main() void {\n    println(Box { v = 1 }.show())\n    println(via(Box { v = 5 }))\n}\n");
    assert_eq!(run(&src).unwrap(), "intbox 2\nintbox 6\n");
}

#[test]
fn a_method_of_an_impl_for_one_instance_is_not_a_method_of_another() {
    let src = with("impl Show for Box<int> { fn show(self) string { return \"i\" } }\n\
                    fn main() void { println(Box { v = \"hi\" }.show()) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-104"]));
    assert_eq!(messages(&src), vec!["type `Box<string>` has no method `show`"]);
}

#[test]
fn another_instance_does_not_satisfy_the_bound() {
    let src = with("impl Show for Box<int> { fn show(self) string { return \"i\" } }\n\
                    fn via<X: Show>(x: X) string { return x.show() }\n\
                    fn main() void { println(via(Box { v = \"hi\" })) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Show` is not implemented for `Box<string>`"]);
}

#[test]
fn an_extend_for_one_instance_adds_methods_and_functions_to_it() {
    let src = with("extend Box<string> { fn shout(self) string { return self.v + \"!\" } }\n\
                    extend Box<int> { fn make() Box<int> { return Box { v = 10 } } fn double(self) int { return self.v * 2 } }\n\
                    fn main() void {\n    println(Box { v = \"hi\" }.shout())\n    println(Box.make().double())\n}\n");
    assert_eq!(run(&src).unwrap(), "hi!\n20\n");
}

#[test]
fn an_extend_for_one_instance_is_not_there_for_another() {
    let src = with("extend Box<int> { fn double(self) int { return self.v * 2 } }\n\
                    fn main() void { println(Box { v = \"hi\" }.double()) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-104"]));
}

#[test]
fn a_concrete_position_mixes_with_a_parameter() {
    let src = with("impl<B: Show> Show for Pair<int, B> { fn show(self) string { return self.a.show() + \"/\" + self.b.show() } }\n\
                    fn main() void { println(Pair { a = 1, b = \"z\" }.show()) }\n");
    assert_eq!(run(&src).unwrap(), "i1/z\n");
}

// ── Generic trait arguments naming impl parameters ───────────────

#[test]
fn a_trait_argument_may_name_an_impl_parameter() {
    let src = with("trait Convert<T> { fn convert(self) T }\n\
                    impl<T> Convert<T> for Box<T> { fn convert(self) T { return self.v } }\n\
                    fn conv<X: Convert<int>>(x: X) int { return x.convert() }\n\
                    fn main() void {\n    println(conv(Box { v = 4 }))\n    println(Box { v = \"s\" }.convert())\n}\n");
    assert_eq!(run(&src).unwrap(), "4\ns\n");
}

#[test]
fn a_bound_with_arguments_is_met_only_by_the_arguments_of_the_receiver() {
    let src = with("trait Convert<T> { fn convert(self) T }\n\
                    impl<T> Convert<T> for Box<T> { fn convert(self) T { return self.v } }\n\
                    fn conv<X: Convert<int>>(x: X) int { return x.convert() }\n\
                    fn main() void { println(conv(Box { v = \"s\" })) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Convert<int>` is not implemented for `Box<string>`"]);
}

#[test]
fn a_method_signature_is_checked_against_the_trait_in_the_blocks_numbering() {
    let src = with("trait Convert<T> { fn convert(self) T }\n\
                    impl<T> Convert<T> for Box<T> { fn convert(self) int { return 1 } }\nfn main() void { }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-125"]));
}

#[test]
fn a_missing_trait_method_names_the_target_as_written() {
    let src = with("impl<T> Show for Box<T> { }\nfn main() void { }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-123"]));
    assert_eq!(messages(&src), vec!["`Box<T>` does not implement `show`, which trait `Show` requires"]);
}

#[test]
fn an_alias_of_one_instance_is_that_instance_as_a_target() {
    let src = with("type IntBox = Box<int>\n\
                    impl Show for IntBox { fn show(self) string { return \"ib\" } }\n\
                    fn main() void { println(Box { v = \"s\" }.show()) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-104"]));
    assert_eq!(messages(&src), vec!["type `Box<string>` has no method `show`"]);
    let ok = with("type IntBox = Box<int>\n\
                   impl Show for IntBox { fn show(self) string { return \"ib\" } }\n\
                   fn main() void { println(Box { v = 1 }.show()) }\n");
    assert_eq!(run(&ok).unwrap(), "ib\n");
}

#[test]
fn an_undeclared_name_as_an_argument_is_an_unconstrained_position_not_a_bound_one() {
    // `extend Box<T>` without `<T>` is the earlier form: the name is not bound
    // and the block covers every `Box`, and `T` inside is still the struct's
    // own unbounded parameter.
    let good = with("extend Box<T> { fn one(self) int { return 1 } }\n\
                     fn main() void {\n    println(Box { v = 1 }.one())\n    println(Box { v = \"s\" }.one())\n}\n");
    assert_eq!(run(&good).unwrap(), "1\n1\n");
    let bad = with("extend Box<T> { fn bad(self) string { return self.v.show() } }\nfn main() void { }\n");
    assert_eq!(codes(&bad), (Stage::Sema, vec!["TYPE-130"]));
}

#[test]
fn self_in_a_generic_block_is_known_to_the_checker() {
    let src = with("extend<T> Box<T> { fn same(self) Self { return self } }\n\
                    fn main() void { let n: int = Box { v = \"s\" }.same().v }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-101"]));
}

#[test]
fn a_bound_on_an_impl_parameter_brings_the_traits_it_builds_on() {
    // `T: Loud` gives `show` because `Loud: Show`.
    let src = with("trait Loud: Show { fn loud(self) string }\n\
                    trait Echo { fn echo(self) string }\n\
                    impl Loud for int { fn loud(self) string { return \"L\" } }\n\
                    impl<T: Loud> Echo for Box<T> { fn echo(self) string { return self.v.show() } }\n\
                    fn main() void { println(Box { v = 3 }.echo()) }\n");
    assert_eq!(run(&src).unwrap(), "i3\n");
}

#[test]
fn a_bound_may_name_a_parameter_declared_after_it() {
    // `B` is declared after the bound that names it and sits at the second
    // argument of `Pair`, so the bound is `Convert<string>` for `Pair<int, string>`.
    let src = with("trait Convert<T> { fn convert(self) T }\n\
                    impl Convert<string> for int { fn convert(self) string { return \"c\" } }\n\
                    impl<A: Convert<B>, B: Show> Show for Pair<A, B> {\n    fn show(self) string { return self.a.convert().show() + self.b.show() }\n}\n\
                    fn main() void { println(Pair { a = 1, b = \"s\" }.show()) }\n");
    assert_eq!(run(&src).unwrap(), "cs\n");
}

#[test]
fn a_qualified_call_of_a_generic_trait_gets_the_receivers_arguments() {
    let src = with("trait Convert<T> { fn convert(self) T }\n\
                    impl<T> Convert<T> for Box<T> { fn convert(self) T { return self.v } }\n\
                    fn main() void { println(Convert.convert(Box { v = 4 }) + 1) }\n");
    assert_eq!(run(&src).unwrap(), "5\n");
}

#[test]
fn a_default_method_of_an_impl_for_one_instance_is_not_there_for_another() {
    let src = with("trait Greet { fn name(self) string\n    fn greet(self) string { return \"hi \" + self.name() } }\n\
                    impl Greet for Box<int> { fn name(self) string { return \"int\" } }\n\
                    fn main() void {\n    println(Box { v = 1 }.greet())\n    println(Box { v = \"s\" }.greet())\n}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-104"]));
    assert_eq!(messages(&src), vec!["type `Box<string>` has no method `greet`"]);
    let ok = with("trait Greet { fn name(self) string\n    fn greet(self) string { return \"hi \" + self.name() } }\n\
                   impl Greet for Box<int> { fn name(self) string { return \"int\" } }\n\
                   fn main() void { println(Box { v = 1 }.greet()) }\n");
    assert_eq!(run(&ok).unwrap(), "hi int\n");
}

#[test]
fn a_reported_trait_argument_shows_what_its_variables_were_solved_to() {
    let src = with("trait Convert<T> { fn convert(self) T }\n\
                    impl Convert<int> for string { fn convert(self) int { return 7 } }\n\
                    fn conv<T, X: Convert<Box<T>>>(t: T, x: X) int { return 1 }\n\
                    fn main() void { println(conv(\"a\", \"s\")) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Convert<Box<string>>` is not implemented for `string`"]);
}

// ── The built-in generic heads ───────────────────────────────────

#[test]
fn a_list_takes_a_conditional_impl_and_an_extend() {
    let src = with("impl<T: Show> Show for List<T> {\n    fn show(self) string {\n        let mut out = \"[\"\n        for x in self { out = out + x.show() + \";\" }\n        return out + \"]\"\n    }\n}\n\
                    extend<T> List<T> { fn second(self) T { return self[1] } }\n\
                    extend List<int> { fn total(self) int {\n    let mut t = 0\n    for x in self { t = t + x }\n    return t\n} }\n\
                    fn via<X: Show>(x: X) string { return x.show() }\n\
                    fn main() void {\n    let l = [1, 2, 3]\n    println(l.show())\n    println(via(l))\n    println(Show.show(l))\n    println(l.second())\n    println(l.total())\n}\n");
    assert_eq!(run(&src).unwrap(), "[i1;i2;i3;]\n[i1;i2;i3;]\n[i1;i2;i3;]\n2\n6\n");
}

#[test]
fn an_impl_for_one_list_instance_does_not_satisfy_a_bound_for_another() {
    let src = with("impl Show for List<int> { fn show(self) string { return \"ints\" } }\n\
                    fn via<X: Show>(x: X) string { return x.show() }\n\
                    fn main() void { println(via([\"a\"])) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Show` is not implemented for `List<string>`"]);
    let qualified = with("impl Show for List<int> { fn show(self) string { return \"ints\" } }\n\
                          fn main() void { println(Show.show([\"a\"])) }\n");
    assert_eq!(codes(&qualified), (Stage::Sema, vec!["TYPE-126"]));
}

#[test]
fn a_method_of_a_list_block_gets_the_lists_element_type() {
    let src = with("extend<T> List<T> { fn second(self) T { return self[1] } }\n\
                    fn main() void { println([10, 20].second() + 1) }\n");
    assert_eq!(run(&src).unwrap(), "21\n");
}

#[test]
fn a_dictionary_takes_an_impl_with_both_arguments() {
    let src = with("trait Size { fn size(self) int }\n\
                    impl<K: Hash, V: Show> Size for Dictionary<K, V> { fn size(self) int { return self.len() } }\n\
                    impl Show for Dictionary<string, int> { fn show(self) string { return \"d\" } }\n\
                    fn main() void {\n    let mut d = Dictionary.new()\n    d.set(\"a\", 1)\n    println(d.size())\n    println(d.show())\n}\n");
    assert_eq!(run(&src).unwrap(), "1\nd\n");
}

#[test]
fn a_queue_a_stack_and_an_inline_list_take_impls() {
    let src = with("impl Show for Queue<int> { fn show(self) string { return \"queue\" } }\n\
                    impl<T> Show for Stack<T> { fn show(self) string { return \"stack\" } }\n\
                    impl Show for InlineList<int> { fn show(self) string { return \"inline\" } }\n\
                    fn main() void {\n    let q = Queue.new()\n    q.enqueue(1)\n    let s = Stack.new()\n    s.push(1)\n    let v = InlineList.new(3)\n    v.push(1)\n    println(q.show())\n    println(s.show())\n    println(v.show())\n}\n");
    assert_eq!(run(&src).unwrap(), "queue\nstack\ninline\n");
}

#[test]
fn an_alias_of_a_collection_is_the_collection_as_a_target() {
    let src = with("type Ints = List<int>\n\
                    impl Show for Ints { fn show(self) string { return \"ints\" } }\n\
                    fn main() void { println([1, 2].show()) }\n");
    assert_eq!(run(&src).unwrap(), "ints\n");
}

#[test]
fn an_impl_for_one_list_instance_is_not_there_for_another() {
    let src = with("impl Show for List<int> { fn show(self) string { return \"ints\" } }\n\
                    fn main() void { println([\"a\"].show()) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-104"]));
    assert_eq!(messages(&src), vec!["type `List<string>` has no method `show`"]);
}

#[test]
fn a_native_method_wins_over_a_method_of_the_same_name_in_an_extend() {
    let src = with("extend<T> List<T> { fn len(self) int { return 99 } }\n\
                    fn main() void { println([1, 2, 3].len()) }\n");
    assert_eq!(run(&src).unwrap(), "3\n");
}

#[test]
fn extend_on_a_bare_collection_name_means_every_instance() {
    let src = with("extend List { fn twice(self) int { return 2 } }\n\
                    fn main() void { println([1].twice())\n println([\"a\"].twice()) }\n");
    assert_eq!(run(&src).unwrap(), "2\n2\n");
}

#[test]
fn extend_on_a_bare_generic_struct_name_keeps_its_old_meaning() {
    let src = with("extend Box { fn one(self) int { return 1 } }\n\
                    fn main() void { println(Box { v = \"a\" }.one()) }\n");
    assert_eq!(run(&src).unwrap(), "1\n");
}

#[test]
fn two_heads_of_one_trait_dispatch_independently() {
    let src = with(&format!("{IMPL_BOX}impl<A: Show, B: Show> Show for Pair<A, B> {{ fn show(self) string {{ return \"Pair\" }} }}\n\
        impl<T> Show for List<T> {{ fn show(self) string {{ return \"List\" }} }}\n\
        fn main() void {{\n    println(Box {{ v = 1 }}.show())\n    println(Pair {{ a = 1, b = 2 }}.show())\n    println([1].show())\n}}\n"));
    assert_eq!(run(&src).unwrap(), "Box(i1)\nPair\nList\n");
}

// ── Overlap: one impl per head ───────────────────────────────────

#[test]
fn a_second_impl_for_the_same_head_overlaps_whatever_the_arguments() {
    let src = with("impl Show for Box<int> { fn show(self) string { return \"a\" } }\n\
                    impl Show for Box<string> { fn show(self) string { return \"b\" } }\nfn main() void { }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-128"]));
    assert_eq!(messages(&src), vec![
        "`Show` is implemented for `Box<string>` more than once (generics are erased at run time, so one impl covers every instance of the type)",
    ]);
}

#[test]
fn a_generic_impl_and_a_concrete_impl_of_one_trait_overlap() {
    let src = with(&format!("{IMPL_BOX}impl Show for Box<int> {{ fn show(self) string {{ return \"b\" }} }}\nfn main() void {{ }}\n"));
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-128"]));
}

#[test]
fn two_collection_impls_of_one_trait_overlap_whatever_their_arguments() {
    let src = with("impl Show for List<int> { fn show(self) string { return \"a\" } }\n\
                    impl Show for List<string> { fn show(self) string { return \"b\" } }\nfn main() void { }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-128"]));
    assert_eq!(messages(&src), vec![
        "`Show` is implemented for `List<string>` more than once (generics are erased at run time, so one impl covers every instance of the type)",
    ]);
}

#[test]
fn an_array_and_a_list_are_one_kind_of_value_so_their_impls_overlap() {
    let src = with("impl Show for [3]int { fn show(self) string { return \"a\" } }\n\
                    impl<T> Show for List<T> { fn show(self) string { return \"b\" } }\nfn main() void { }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-128"]));
    assert_eq!(messages(&src), vec![
        "`Show` is implemented for `List<T>` more than once (`[3]int` and `List` are one kind of value at run time, and a call cannot tell arrays and lists of every length and element type apart)",
    ]);
}

#[test]
fn one_method_name_has_one_inherent_definition_per_generic_head() {
    let src = with("extend<T> Box<T> { fn f(self) int { return 1 } }\n\
                    extend Box<int> { fn f(self) int { return 2 } }\nfn main() void { }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-129"]));
    assert_eq!(positions(&src), vec![pos_of(&src, "fn f(self) int { return 2 }")]);
}

#[test]
fn a_method_of_the_struct_itself_counts_as_an_inherent_definition_of_its_head() {
    let src = "struct Box<T> { v: T\n    fn f(self) int { return 1 } }\n\
               extend<T> Box<T> { fn f(self) int { return 2 } }\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-129"]));
}

#[test]
fn different_method_names_on_different_instances_are_fine() {
    let src = with("extend Box<int> { fn a(self) int { return 1 } }\nextend Box<string> { fn b(self) int { return 2 } }\n\
                    fn main() void { println(Box { v = 1 }.a() + Box { v = \"s\" }.b()) }\n");
    assert_eq!(run(&src).unwrap(), "3\n");
}

#[test]
fn two_primitives_of_one_run_time_kind_cannot_both_define_a_method_name() {
    // `int` and `long` are one kind of value at run time, so `tag` would be
    // whichever block registered last.
    let src = "extend int { fn tag(self) string { return \"i\" } }\nextend long { fn tag(self) string { return \"l\" } }\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-129"]));
    assert_eq!(positions(src), vec![pos_of(src, "fn tag(self) string { return \"l\" }")]);
}

#[test]
fn an_array_and_a_list_cannot_both_define_a_method_name() {
    let src = with("extend [3]int { fn f(self) int { return 1 } }\nextend<T> List<T> { fn f(self) int { return 2 } }\nfn main() void { }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-129"]));
}

#[test]
fn one_block_may_extend_a_primitive_twice_over_distinct_names() {
    let src = "extend int { fn a(self) int { return 1 } }\nextend long { fn b(self) int { return 2 } }\n\
               fn main() void {\n    let x: int = 1\n    let y: long = 2\n    println(x.a() + y.b())\n}\n";
    assert_eq!(run(src).unwrap(), "3\n");
}

// ── Targets that cannot take an impl yet (TYPE-129 and TYPE-108) ──

const UNSUPPORTED: &str = " is not supported yet";

#[test]
fn a_repeated_or_nested_parameter_is_reported() {
    let repeated = with("impl<T> Show for Pair<T, T> { fn show(self) string { return \"p\" } }\nfn main() void { }\n");
    let nested   = with("impl<T> Show for Box<List<T>> { fn show(self) string { return \"p\" } }\nfn main() void { }\n");
    for src in [&repeated, &nested] {
        assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-129"]));
        assert_eq!(messages(src), vec![format!(
            "an impl target that repeats a type parameter or nests one inside another type (`Pair<T, T>`, `Box<List<T>>`){UNSUPPORTED}")]);
    }
}

#[test]
fn a_parameter_the_target_does_not_use_is_reported() {
    let src = with("impl<T> Show for Box<int> { fn show(self) string { return \"p\" } }\nfn main() void { }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-129"]));
    assert_eq!(messages(&src), vec![format!("an impl type parameter that the target type does not use{UNSUPPORTED}")]);
    let prim = with("impl<T> Show for bool { fn show(self) string { return \"p\" } }\nfn main() void { }\n");
    assert_eq!(messages(&prim), vec![format!("an impl type parameter that the target type does not use{UNSUPPORTED}")]);
}

#[test]
fn a_blanket_impl_is_reported() {
    let src = with("impl<T> Show for T { fn show(self) string { return \"p\" } }\nfn main() void { }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-129"]));
    assert_eq!(messages(&src), vec![format!("a blanket impl (a type parameter as the impl target){UNSUPPORTED}")]);
}

#[test]
fn an_optional_a_fallible_and_a_task_are_transparent_wrappers() {
    let wrapper = format!("implementing a trait for an optional, a fallible or a task type, which are transparent wrappers (a method call on one resolves on the value inside it){UNSUPPORTED}");
    for target in ["int?", "int!", "Task<int>"] {
        let src = with(&format!("impl Show for {target} {{ fn show(self) string {{ return \"p\" }} }}\nfn main() void {{ }}\n"));
        assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-129"]), "{target}");
        assert_eq!(messages(&src), vec![wrapper.clone()], "{target}");
    }
    let ext = with("extend int? { fn f(self) int { return 1 } }\nfn main() void { }\n");
    assert_eq!(messages(&ext), vec![format!("extending an optional, a fallible or a task type, which are transparent wrappers (a method call on one resolves on the value inside it){UNSUPPORTED}")]);
}

#[test]
fn a_set_and_a_slice_are_reported() {
    for target in ["Set<int>", "[]int"] {
        let src = with(&format!("impl Show for {target} {{ fn show(self) string {{ return \"p\" }} }}\nfn main() void {{ }}\n"));
        assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-129"]), "{target}");
    }
}

#[test]
fn a_pool_and_a_handle_are_reported() {
    for target in ["Pool<int>", "Handle<int>"] {
        let src = with(&format!("impl Show for {target} {{ fn show(self) string {{ return \"p\" }} }}\nfn main() void {{ }}\n"));
        assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-129"]), "{target}");
        assert_eq!(messages(&src), vec![format!("implementing a trait for a Pool or a Handle (a written `Pool<T>` or `Handle<T>` is not a type of its own yet){UNSUPPORTED}")], "{target}");
    }
}

#[test]
fn a_prelude_trait_by_hand_for_a_collection_is_reported() {
    let src = "impl PartialEq for List<int> { fn eq(self, o: List<int>) bool { return true } }\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-129"]));
    assert_eq!(messages(src), vec![format!("implementing a prelude trait by hand for a collection (a collection is compared, hashed and cloned by identity at run time){UNSUPPORTED}")]);
}

#[test]
fn a_generic_head_without_its_arguments_in_a_trait_impl_is_a_count_mistake() {
    let user = with("impl Show for Box { fn show(self) string { return \"p\" } }\nfn main() void { }\n");
    assert_eq!(codes(&user), (Stage::Sema, vec!["TYPE-108"]));
    assert_eq!(messages(&user), vec!["`Box` expects 1 type argument(s), found 0"]);
    let builtin = with("impl Show for List { fn show(self) string { return \"p\" } }\nfn main() void { }\n");
    assert_eq!(codes(&builtin), (Stage::Sema, vec!["TYPE-108"]));
    assert_eq!(messages(&builtin), vec!["`List` expects 1 type argument(s), found 0"]);
}

#[test]
fn a_struct_without_parameters_given_arguments_is_a_count_mistake() {
    let src = with("impl Show for Plain<int> { fn show(self) string { return \"p\" } }\nfn main() void { }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-108"]));
    assert_eq!(messages(&src), vec!["`Plain` expects 0 type argument(s), found 1"]);
}

#[test]
fn the_wrong_number_of_arguments_is_a_count_mistake() {
    let src = with("impl Show for Box<int, int> { fn show(self) string { return \"p\" } }\nfn main() void { }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-108"]));
    assert_eq!(messages(&src), vec!["`Box` expects 1 type argument(s), found 2"]);
}

#[test]
fn a_reported_block_is_not_read_so_nothing_follows_from_it() {
    let src = with("impl<T> Show for Pair<T, T> { fn show(self) string { return self.a.show() + self.nothing } }\nfn main() void { }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-129"]));
}

#[test]
fn an_unreported_target_position_is_the_impl_for_a_trait_and_the_type_for_extend() {
    let imp = with("impl<T> Show for T? { fn show(self) string { return \"p\" } }\nfn main() void { }\n");
    assert_eq!(positions(&imp), vec![pos_of(&imp, "impl<T> Show for T?")]);
    let ext = with("extend Set<int> { fn f(self) int { return 1 } }\nfn main() void { }\n");
    assert_eq!(positions(&ext), vec![pos_of(&ext, "Set<int>")]);
}

// ── Blocks that keep the older paths ─────────────────────────────

#[test]
fn a_non_generic_struct_and_the_primitives_are_unchanged() {
    let src = with("impl Show for Plain { fn show(self) string { return \"plain\" } }\n\
                    extend int { fn twice(self) int { return self * 2 } }\n\
                    fn via<X: Show>(x: X) string { return x.show() }\n\
                    fn main() void {\n    println(via(Plain { n = 1 }))\n    let n: int = 4\n    println(n.twice())\n}\n");
    assert_eq!(run(&src).unwrap(), "plain\n8\n");
}

#[test]
fn a_clean_program_with_every_form_is_clean() {
    let src = with(&format!("{IMPL_BOX}impl Show for List<int> {{ fn show(self) string {{ return \"l\" }} }}\n\
        extend<T> Box<T> {{ fn get(self) T {{ return self.v }} }}\nfn main() void {{ }}\n"));
    assert_eq!(codes(&src), CLEAN);
}
