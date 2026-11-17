//! Single-file groundwork for modules (docs/MODULE_DESIGN.md, MS1).
//!
//! The `.ubl` fixtures show whole programs accepted or rejected. These tests
//! pin what they cannot see: the shape of the parsed import (which name
//! carries which rename), the exact code and count each rule reports, that a
//! path that did not resolve is reported once and does not cascade into
//! `undefined name`, that a renamed builtin behaves as the builtin at run
//! time and in the type checker, and the stated limits of the
//! initialization-cycle check.

use ubel_stratum::ast::arena::AstArena;
use ubel_stratum::ast::common::Visibility;
use ubel_stratum::ast::root::{ImportItems, ImportKind, Item};
use ubel_stratum::interpreter::Interpreter;
use ubel_stratum_rd::{check_source, Stage};

/// The stage a program stops at and the codes it reports, in order.
fn codes(source: &str) -> (Stage, Vec<&'static str>) {
    let report = check_source(source);
    let codes = report.diagnostics.iter().map(|d| d.code).collect();
    (report.stage, codes)
}

fn messages(source: &str) -> Vec<String> {
    check_source(source).diagnostics.iter().map(|d| d.message.clone()).collect()
}

fn is_clean(source: &str) -> bool {
    matches!(check_source(source).stage, Stage::Clean)
}

/// Run a program that passes `check_source` and return what it printed.
fn run(source: &str) -> Result<String, String> {
    assert!(is_clean(source), "program should check clean: {:?}", codes(source).1);
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

/// Parse a program and hand its AST to `f`.
fn with_program<R>(source: &str, f: impl FnOnce(&ubel_stratum::ast::root::Program<'_>) -> R) -> R {
    let tokens = ubel_stratum::lexer::tokenize(source).expect("source should lex");
    let arena = AstArena::new();
    let program = ubel_stratum_rd::parse(&arena, &tokens, source.to_string())
        .unwrap_or_else(|_| panic!("source should parse"));
    f(&program)
}

// ── `as` inside an import list ───────────────────────────────────

#[test]
fn each_name_in_an_import_list_keeps_its_own_rename() {
    let src = "from std.collections summon [List as L, Dictionary, Set as S]\nfn main() void { }\n";
    with_program(src, |p| {
        let ImportKind::FromSummon { items: ImportItems::List(items), .. } = &p.imports[0].kind
            else { panic!("expected a from-summon list") };
        let seen: Vec<(&str, Option<&str>)> = items.iter().map(|i| (i.name, i.alias)).collect();
        assert_eq!(seen, vec![("List", Some("L")), ("Dictionary", None), ("Set", Some("S"))]);
        let locals: Vec<&str> = items.iter().map(|i| i.local_name()).collect();
        assert_eq!(locals, vec!["L", "Dictionary", "S"]);
    });
}

#[test]
fn a_single_name_can_be_renamed_too() {
    let src = "from std.io summon println as say\nfn main() void { }\n";
    with_program(src, |p| {
        let ImportKind::FromSummon { items: ImportItems::Single(item), .. } = &p.imports[0].kind
            else { panic!("expected a from-summon single") };
        assert_eq!((item.name, item.alias), ("println", Some("say")));
    });
}

#[test]
fn the_name_after_as_is_not_imported_as_a_second_name() {
    // Before the fix `[a as b]` brought in `a` and `b` as two separate names.
    let src = "from std.collections summon [List as L]\nfn main() void { let xs = List.new() }\n";
    with_program(src, |p| {
        let ImportKind::FromSummon { items: ImportItems::List(items), .. } = &p.imports[0].kind
            else { panic!("expected a from-summon list") };
        assert_eq!(items.len(), 1, "one entry, not two");
    });
}

#[test]
fn a_missing_name_after_as_is_reported_in_every_import_form() {
    for src in [
        "from std.collections summon [List as ]\nfn main() void { }\n",
        "from std.io summon println as\nfn main() void { }\n",
        "summon std.math.sqrt as\nfn main() void { }\n",
    ] {
        let report = check_source(src);
        assert!(matches!(report.stage, Stage::Parse), "stops at parse: {src}");
        assert!(report.diagnostics.iter().any(|d| d.code == "PARSE-001"
            && d.message.contains("identifier after 'as'")), "reported: {src}");
    }
}

// ── `pub const` and `pub type` ───────────────────────────────────

#[test]
fn pub_on_a_const_or_a_type_alias_is_recorded() {
    let src = "pub const A = 1\nconst B = 2\npub type T = int\ntype U = int\nfn main() void { }\n";
    with_program(src, |p| {
        let mut consts = Vec::new();
        let mut types  = Vec::new();
        for item in p.items {
            match item {
                Item::Const(c)     if !c.span.is_prelude() => consts.push((c.name, c.visibility)),
                Item::TypeAlias(a) if !a.span.is_prelude() => types.push((a.name, a.visibility)),
                _ => {}
            }
        }
        assert_eq!(consts, vec![("A", Visibility::Public), ("B", Visibility::Private)]);
        assert_eq!(types,  vec![("T", Visibility::Public), ("U", Visibility::Private)]);
    });
}

#[test]
fn a_pub_const_and_type_still_check_and_run() {
    let src = "pub const MAX: int = 10\npub type Score = int\n\
               fn main() void {\n    let s: Score = MAX\n    println(s)\n}\n";
    assert_eq!(run(src).unwrap(), "10\n");
}

// ── `summon std.*` is checked against what the compiler provides ──

#[test]
fn every_valid_import_form_checks_clean() {
    for src in [
        "summon std.collections.List\nfn main() void { }\n",
        "summon std.io.println\nfn main() void { println(1) }\n",
        "from std.collections summon [List, Dictionary, Queue, Stack]\nfn main() void { }\n",
        "from std.math summon sqrt\nfn main() void { println(sqrt(4)) }\n",
        "from std.memory summon [Unique, Shared]\nfn main() void { }\n",
        "from std.convert summon [to_string, to_int]\nfn main() void { }\n",
        "from std.debug summon [assert, panic, typeof]\nfn main() void { }\n",
    ] {
        assert!(is_clean(src), "{src} -> {:?}", codes(src).1);
    }
}

#[test]
fn a_name_the_module_does_not_have_is_name_003_once() {
    for src in [
        "summon std.collections.Frobnicate\nfn main() void { }\n",
        "from std.collections summon Missing\nfn main() void { }\n",
        "summon std.collections.println\nfn main() void { }\n",
    ] {
        assert_eq!(codes(src), (Stage::Sema, vec!["NAME-003"]), "{src}");
    }
}

#[test]
fn a_module_that_does_not_exist_is_name_003_once_even_with_many_names() {
    let src = "from std.nope summon [A, B, C]\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["NAME-003"]));
    assert_eq!(messages(src), vec!["cannot resolve import path `std.nope`".to_string()]);
}

#[test]
fn a_module_path_alone_is_not_a_member() {
    let src = "summon std.collections\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["NAME-003"]));
}

#[test]
fn a_path_outside_std_cannot_resolve_before_multi_file_loading() {
    for src in [
        "from game.util summon Helper\nfn main() void { }\n",
        "summon engine.physics.Body\nfn main() void { }\n",
    ] {
        assert_eq!(codes(src), (Stage::Sema, vec!["NAME-003"]), "{src}");
    }
}

#[test]
fn each_bad_name_in_one_list_is_reported_separately() {
    let src = "from std.collections summon [List, Nope, Dictionary, Gone]\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["NAME-003", "NAME-003"]));
    assert_eq!(messages(src), vec![
        "cannot resolve import path `std.collections.Nope`".to_string(),
        "cannot resolve import path `std.collections.Gone`".to_string(),
    ]);
}

#[test]
fn an_unresolved_import_does_not_cascade_into_undefined_name() {
    let src = "from game.util summon Helper\nfn main() void {\n    let h = Helper.make()\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["NAME-003"]));
}

#[test]
fn importing_a_builtin_is_optional() {
    let with    = "summon std.collections.List\nfn main() void { let mut xs = List.new()\n xs.push(1)\n println(xs.len()) }\n";
    let without = "fn main() void { let mut xs = List.new()\n xs.push(1)\n println(xs.len()) }\n";
    assert_eq!(run(with).unwrap(), run(without).unwrap());
}

// ── A renamed builtin is the builtin ─────────────────────────────

#[test]
fn a_renamed_collection_builds_and_runs_like_the_original() {
    let src = "summon std.collections.List as L\n\
               from std.collections summon [Dictionary as D, Queue as Q]\n\
               fn main() void {\n\
                   let mut xs: L<int> = L.new()\n    xs.push(4)\n    xs.push(5)\n\
                   let mut d: D<string, int> = D.new()\n    d.set(\"a\", 7)\n\
                   let mut q: Q<int> = Q.new()\n    q.enqueue(9)\n\
                   println(xs.len())\n    println(d.get(\"a\"))\n    println(q.len())\n}\n";
    assert_eq!(run(src).unwrap(), "2\n7\n1\n");
}

#[test]
fn a_renamed_function_builtin_is_callable_under_the_new_name() {
    let src = "from std.io summon println as say\nsummon std.math.sqrt as root\n\
               fn main() void {\n    say(root(16))\n}\n";
    assert_eq!(run(src).unwrap(), "4\n");
}

#[test]
fn a_renamed_math_namespace_still_reaches_its_members() {
    let src = "summon std.math.Math as M\nfn main() void { println(M.sqrt(25)) }\n";
    assert_eq!(run(src).unwrap(), "5\n");
}

#[test]
fn a_renamed_pool_builds_inside_a_pool_block() {
    // `Pool.new()` is special-cased at run time (it takes its capacity from the
    // enclosing `with pool<T>(n)` block), so it has its own path through the rename.
    let src = "summon std.memory.Pool as P\n@tier(mid)\nfn run() void {\n    with pool<int>(2) {\n\
               let p = P.new()\n        let a = p.acquire(1)\n\
               assert(p.get(a) == 1, \"pool through a rename\")\n    }\n}\nfn main() void { run() }\n";
    assert_eq!(run(src).unwrap(), "");
}

#[test]
fn the_original_name_keeps_working_beside_its_rename() {
    let src = "summon std.collections.List as L\n\
               fn main() void {\n    let a = L.new()\n    let b = List.new()\n    println(a.len() + b.len())\n}\n";
    assert_eq!(run(src).unwrap(), "0\n");
}

#[test]
fn the_type_checker_treats_the_rename_as_the_builtin_type() {
    // Were `L<int>` left unresolved, the first line would not be a mismatch.
    let src = "summon std.collections.List as L\nfn main() void { let n: L<int> = 5 }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-101"]));
    assert_eq!(messages(src), vec!["type mismatch: expected `List<int>`, found `int`".to_string()]);
}

#[test]
fn a_renamed_collection_keeps_its_type_argument_count() {
    let src = "summon std.collections.List as L\nfn main() void { let w: L<int, int> = L.new() }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-108"]));
}

#[test]
fn tier_rules_apply_through_a_rename() {
    let src = "summon std.memory.Unique as U\nfn main() void { let u = U.new(5) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TIER-014"]));
}

#[test]
fn a_rename_cannot_take_the_name_of_another_builtin() {
    let src = "summon std.collections.List as Dictionary\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["NAME-002"]));
    // A builtin has no place in the source: no "first defined here" note.
    assert!(check_source(src).diagnostics[0].secondary.is_empty());
}

#[test]
fn two_imports_cannot_share_a_local_name() {
    let src = "summon std.collections.Queue as Q\nsummon std.collections.Stack as Q\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["NAME-002"]));
}

#[test]
fn a_rename_cannot_take_the_name_of_a_local_item() {
    let src = "summon std.collections.Set as helper\nfn helper() void { }\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["NAME-002"]));
}

// ── Initialization cycles ────────────────────────────────────────

#[test]
fn constants_that_read_each_other_are_one_error() {
    let src = "const A = B + 1\nconst B = A + 1\nfn main() void { println(A) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["NAME-009"]));
    assert_eq!(messages(src), vec!["initialization cycle among `A`, `B`".to_string()]);
}

#[test]
fn a_constant_that_reads_itself_is_an_error() {
    let src = "const Z = Z * 2\nfn main() void { println(Z) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["NAME-009"]));
    assert_eq!(messages(src), vec!["`Z` depends on itself while it is being initialized".to_string()]);
}

#[test]
fn statics_that_read_each_other_are_one_error() {
    let src = "static P: int = Q + 1\nstatic Q: int = P + 1\nfn main() void { println(P) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["NAME-009"]));
}

#[test]
fn two_separate_cycles_are_two_errors() {
    let src = "const A = B\nconst B = A\nconst C = D\nconst D = C\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["NAME-009", "NAME-009"]));
}

#[test]
fn a_cycle_through_functions_called_in_the_initializers_is_found() {
    let src = "const A = f()\nfn f() int { return B }\nconst B = g()\nfn g() int { return A }\n\
               fn main() void { println(A) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["NAME-009"]));
    assert_eq!(messages(src), vec!["initialization cycle among `A`, `f`, `B`, `g`".to_string()]);
}

#[test]
fn a_static_that_reads_itself_through_its_own_initializer_function_is_found() {
    let src = "static N: int = next()\nfn next() int { N += 1\n return N }\nfn main() void { println(N) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["NAME-009"]));
}

#[test]
fn dependencies_in_any_order_without_a_cycle_are_fine() {
    let src = "const A = double(B)\nconst B = 21\nfn double(x: int) int { return x * 2 }\n\
               fn fact(n: int) int { if n <= 1 { return 1 } return n * fact(n - 1) }\n\
               const C = fact(5)\nstatic S: int = A + C\n\
               fn main() void {\n    println(A)\n    println(C)\n    println(S)\n}\n";
    assert_eq!(run(src).unwrap(), "42\n120\n162\n");
}

#[test]
fn recursion_among_functions_alone_is_not_a_cycle() {
    let src = "fn even(n: int) bool { if n == 0 { return true } return odd(n - 1) }\n\
               fn odd(n: int) bool { if n == 0 { return false } return even(n - 1) }\n\
               const E = even(4)\nfn main() void { println(E) }\n";
    assert!(is_clean(src), "{:?}", codes(src).1);
}

#[test]
fn a_local_with_the_constants_name_is_not_a_use_of_it() {
    let src = "const A = f()\nfn f() int { let A = 3\n return A }\nfn main() void { println(A) }\n";
    assert!(is_clean(src), "{:?}", codes(src).1);
}

#[test]
fn a_function_nothing_initializes_with_may_read_the_constant() {
    let src = "const A = 5\nfn show() void { println(A) }\nfn main() void { show() }\n";
    assert!(is_clean(src), "{:?}", codes(src).1);
}

#[test]
fn a_function_that_is_not_on_the_cycle_is_not_named() {
    // `helper` is reached from the cycle but does not lead back to it.
    let src = "const A = B\nconst B = A + helper()\nfn helper() int { return 1 }\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["NAME-009"]));
    assert_eq!(messages(src), vec!["initialization cycle among `A`, `B`".to_string()]);
}

#[test]
fn the_check_is_conservative_about_a_branch_that_never_runs() {
    // Documented limit (docs/MODULE_DESIGN.md): the function names `A`, so it
    // is an edge, even though `f(false)` never reads it during initialization.
    let src = "const A = f(false)\nfn f(flag: bool) int { if flag { return A } return 0 }\n\
               fn main() void { println(A) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["NAME-009"]));
}
