# Modules and imports

> **Status: single-file imports.** A `summon std...` path is checked against the
> names the compiler provides, and `as` renames a built-in. Nothing is loaded
> from another file: there is no module resolution beyond `std` and no
> multi-file builds. The design was decided on 2026-11-15 and is summarised at
> the end; the multi-file part is not built. This page describes what you can
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
from std.math summon sqrt                  // or a single one

fn main() {
    println("imports resolve")
}
```

```text
imports resolve
```

| Form | Means |
|------|-------|
| `package a.b` | names this file's package |
| `summon a.b.C` | bring `C` from `a.b` into scope |
| `summon a.b.C as D` | the same, under the name `D` |
| `from a.b summon C` | the same, written the other way round |
| `from a.b summon [C, D as E]` | several names from one place, each optionally renamed |

All imports use `summon`; there is no `import` or `use`.

## What happens today

The names the language provides, such as `println`, `List`, `Dictionary` and
`range`, are available **without** any import (see
[Built-in functions](../reference/builtins.md)), so an import is optional. This
is what an import does today:

- **A path under `std` is checked.** `summon std.collections.List` and
  `from std.io summon [print, println]` name something the compiler provides. A
  member the module does not have, a module that does not exist, a module path
  with no member, and any path outside `std` are
  [NAME-003](../errors/name.md#name-003). A path outside `std` cannot resolve
  yet, because nothing is loaded from another file:

<!-- ubel-test: error NAME-003 -->
```ubel
summon nothing.here.Thing

fn main() {
}
```

- **`as` renames a built-in.** The new name works wherever the built-in does: in
  a type, in a constructor call and as a function. The original name keeps
  working beside it:

<!-- ubel-test: run -->
```ubel
summon std.collections.List as L
from std.io summon [println as say]

fn main() {
    let mut xs: L<int> = L.new()
    xs.push(3)
    say(xs.len())
}
```

```text
1
```

  A new name may not be the name of another built-in, of another import or of an
  item in the file ([NAME-002](../errors/name.md#name-002)).

- **`as` works inside a `from` list.**
  `from std.collections summon [List as L, Dictionary]` renames `List` and
  leaves `Dictionary` alone. A missing name after `as` is a parse error
  ([PARSE-001](../errors/parse.md#parse-001)).
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

`pub` marks an item public. It is recorded on functions, structs, enums, traits,
statics, constants and type aliases, but nothing reads it yet: today everything
in a file is visible to everything else in it.

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
re-exports. The single-file fixes (imports that are checked, a rename of a
built-in, initialisation cycles) have landed. Multi-file checking is next. Until
it lands, everything above under "What happens today" is the behaviour.
