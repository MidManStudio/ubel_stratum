// crates/core/src/builtins/std_modules.rs
//! The names a `summon std...` import can bring in.
//!
//! The builtins are global: `List`, `println` and `Math` need no `summon`,
//! and importing one is optional (docs/MODULE_DESIGN.md, 2.6). The import
//! forms are still checked, so a path that names nothing is `NAME-003`
//! instead of passing `ubel check` and failing when the program runs, and
//! `as` can rename a builtin (`summon std.collections.List as L`).
//!
//! `STD_MODULES` groups the names the compiler provides under module paths.
//! Every entry is a name that `GLOBAL_BUILTINS` or `BUILTIN_NAMESPACES`
//! already provides, or a builtin collection type; the tests at the bottom
//! fail when a builtin is added to one of those lists and not placed here,
//! so the table cannot drift from the registry.

/// The root every standard-library path starts with.
pub const STD_ROOT: &str = "std";

/// `(module, members)`: the module is the second path segment
/// (`std.collections`), a member is the third (`std.collections.List`).
pub static STD_MODULES: &[(&str, &[&str])] = &[
    ("collections", &["List", "Dictionary", "Set", "Queue", "Stack", "InlineList"]),
    ("memory",      &["Pool", "Unique", "Shared", "SyncShared"]),
    ("task",        &["Task"]),
    ("io",          &["println", "print", "log"]),
    ("debug",       &["assert", "panic", "typeof"]),
    ("math",        &["Math", "sqrt", "abs", "min", "max", "floor", "ceil", "range"]),
    ("convert",     &["to_string", "to_int", "to_float", "to_double"]),
];

/// The members of `std.<module>`, or `None` when `path` is not exactly
/// `std.<module>` for a module in the table.
pub fn std_module_members(path: &[&str]) -> Option<&'static [&'static str]> {
    match path {
        [root, module] if *root == STD_ROOT =>
            STD_MODULES.iter().find(|(m, _)| m == module).map(|(_, members)| *members),
        _ => None,
    }
}

/// The builtin name behind `std.<module>.<member>`, or `None` when the path
/// names nothing in the table.
pub fn resolve_std_member(path: &[&str]) -> Option<&'static str> {
    let [root, module, member] = path else { return None; };
    if *root != STD_ROOT { return None; }
    STD_MODULES.iter()
        .find(|(m, _)| m == module)
        .and_then(|(_, members)| members.iter().find(|n| *n == member).copied())
}

/// The builtin `member` of the module at `module_path`, if there is one.
pub fn resolve_std_item(module_path: &[&str], member: &str) -> Option<&'static str> {
    std_module_members(module_path)?.iter().find(|n| **n == member).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_members() -> Vec<&'static str> {
        STD_MODULES.iter().flat_map(|(_, members)| members.iter().copied()).collect()
    }

    #[test]
    fn every_global_builtin_is_in_exactly_one_module() {
        let members = all_members();
        for sig in crate::builtins::global::GLOBAL_BUILTINS {
            let n = members.iter().filter(|m| **m == sig.name).count();
            assert_eq!(n, 1, "builtin function `{}` must be listed in exactly one std module", sig.name);
        }
    }

    #[test]
    fn every_builtin_namespace_is_in_exactly_one_module() {
        let members = all_members();
        for ns in crate::builtins::BUILTIN_NAMESPACES {
            let n = members.iter().filter(|m| **m == *ns).count();
            assert_eq!(n, 1, "builtin namespace `{}` must be listed in exactly one std module", ns);
        }
    }

    #[test]
    fn no_member_name_is_listed_twice() {
        let mut members = all_members();
        members.sort_unstable();
        let total = members.len();
        members.dedup();
        assert_eq!(members.len(), total, "a name appears under two std modules");
    }

    #[test]
    fn resolves_a_full_path_and_rejects_the_near_misses() {
        assert_eq!(resolve_std_member(&["std", "collections", "List"]), Some("List"));
        assert_eq!(resolve_std_member(&["std", "io", "println"]), Some("println"));
        assert_eq!(resolve_std_member(&["std", "collections", "println"]), None, "right name, wrong module");
        assert_eq!(resolve_std_member(&["std", "collections"]), None, "a module is not a member");
        assert_eq!(resolve_std_member(&["game", "collections", "List"]), None, "not under std");
        assert_eq!(resolve_std_member(&["std", "collections", "List", "x"]), None, "too long");
    }

    #[test]
    fn task_is_the_only_member_of_std_task() {
        assert_eq!(resolve_std_member(&["std", "task", "Task"]), Some("Task"));
        assert_eq!(resolve_std_member(&["std", "memory", "Task"]), None, "right name, wrong module");
        assert_eq!(std_module_members(&["std", "task"]), Some(&["Task"][..]));
    }

    #[test]
    fn module_paths_are_exact() {
        assert!(std_module_members(&["std", "math"]).is_some());
        assert!(std_module_members(&["std"]).is_none());
        assert!(std_module_members(&["std", "nope"]).is_none());
        assert!(std_module_members(&["std", "math", "Math"]).is_none());
        assert_eq!(resolve_std_item(&["std", "math"], "sqrt"), Some("sqrt"));
        assert_eq!(resolve_std_item(&["std", "math"], "println"), None);
    }
}
