# Modules and imports

> **Status: syntax only.** Import statements are parsed, but nothing is loaded
> or resolved: there is no module resolution, no standard library to import
> from, and no multi-file builds. The design was decided on 2026-11-15 and is
> summarised at the end; none of it is built. This page describes what you can
> write today and what that actually does.

## The forms

A file may start with a `package` declaration, followed by imports. Imports use
the keyword `summon`, with dots between the parts of a path:

<!-- ubel-test: run -->
```ubel
package app.main

summon std.collections.List                // one name
summon std.io.println as say               // under another name
from std.collections summon [List, Dictionary]   // several names from one place
from std.core summon Option                // or a single one

fn main() {
    println("imports parse")
}
```

```text
imports parse
```

| Form | Means |
|------|-------|
| `package a.b` | names this file's package |
| `summon a.b.C` | bring `C` from `a.b` into scope |
| `summon a.b.C as D` | the same, under the name `D` |
| `from a.b summon C` | the same, written the other way round |
| `from a.b summon [C, D]` | several names from one place |

All imports use `summon`; there is no `import` or `use`.

## What happens today

The names the language provides, such as `println`, `List`, `Dictionary` and
`range`, are available **without** any import (see
[Built-in functions](../reference/builtins.md)). Everything else about imports
is accepted and then ignored:

- **A `summon` of a path that does not exist passes the check.** The
  diagnostics for this exist ([NAME-003](../errors/name.md#name-003),
  [NAME-004](../errors/name.md#name-004)) but are never reported.

<!-- ubel-test: ok -->
```ubel
summon nothing.here.Thing

fn main() {
}
```

- **The interpreter ignores imports.** Renaming a built-in through `as` passes
  the check and then fails when the program runs:

<!-- ubel-test: ok -->
```ubel
summon std.collections.List as L

fn main() {
    let xs = L.new()
}
```

```text
✗ runtime error: panic: undefined name 'L'
```

- **`from m summon [a as b]` is read wrongly.** It is accepted, but it imports
  both `a` and `b` as separate names, because the list parser skips the `as`.
  A known bug.
- **Imports go at the top of the file.** A `summon` inside a function, or between
  items, is a parse error:

<!-- ubel-test: error PARSE-001 -->
```ubel
fn main() {
    summon std.io.println
}
```

- **`package` is parsed and ignored.** A tier attribute in front of it is a
  parse error ([PARSE-001](../errors/parse.md#parse-001)).

Do not rely on an import to keep a name out of scope or to check a path.

## Visibility

`pub` marks an item public. It is recorded on functions, structs, enums, traits
and statics, but nothing reads it yet: today everything in a file is visible to
everything else in it. A `const` or `type` is always private; `pub const` and
`pub type` are accepted and ignored.

## The decided design (not built)

The module system was designed in a session on 2026-11-15, and the record is
`docs/MODULE_DESIGN.md` in the repository. The decisions, in brief:

| Question | Decision |
|----------|----------|
| How a module is named | From its file path: `a.b` is the file `a/b.ubl` under the entry file's directory. The `package` line becomes optional, and when present must match the path. |
| Impls across modules | Program-wide: one impl of a trait for a type holds for the whole program. |
| Cycles between modules | Allowed within one package. A cycle among constants and statics through their initialisers is an error. |
| Visibility | Private by default, with a per-item `pub`. |
| A default tier for a module | An attribute on the package line, for example `@tier(low) package game.physics`. A tier written on a function still wins. |

Also decided: `summon a.b` names the module itself (its items are used as
`b.item`), `as` inside a `from` list gets a real meaning, and `pub summon`
re-exports. Single-file fixes (imports that are checked, initialisation
cycles) come before multi-file checking. Until then, everything above under
"What happens today" is the behaviour.
