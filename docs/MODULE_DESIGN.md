# Modules (`summon`): Design Record

> **Status: decisions made 2026-11-15 in a design session; nothing is built.**
> This document records what the compiler and interpreter do with `summon`, `package` and `pub`
> today (read from source and confirmed by running programs), the decisions taken, a verdict on
> each proposal in `docs/MODULE_SYSTEM_IDEAS.md`, the defaults that were set without a separate
> question, the details that are still open, and a delivery order.
> `docs/MODULE_SYSTEM_IDEAS.md` stays as the list of parked ideas. Async and synchronization are a
> separate record (`docs/ASYNC_SYNC_DESIGN.md`). Prior art (Rust, Go, Zig, Odin) was not
> re-researched for this record; the comparisons below come from general knowledge.

---

## 0. Decisions (2026-11-15)

Every row was a question with options and a recommendation. All six chosen options were the
recommended ones.

| Id | Question | Chosen |
|---|---|---|
| **M1** | How a module is named | From its file path. The `package` line is optional; when present it must match the path. |
| **M2** | Impls and trait methods across modules | Impls are program-wide, and one impl per trait and type pair holds for the whole program (S1, D3). The orphan rule is defined per package, as `TRAITS_DESIGN.md` already states: an impl is allowed only in the package that declares the trait or the type. With a single package it forbids nothing, so the S3d impls on built-in types stay legal. A trait must be in scope to call its methods with dot syntax: defined in the same module or summoned. The prelude traits are always in scope. |
| **M3** | Cycles between modules | Allowed between the modules of one package. A cycle among constants and statics through their initializers is an error. Cycles between packages are to be forbidden when packages exist. |
| **M4** | Fields and methods across modules | Private by default, with a per-item `pub`, as the grammar is written. |
| **M5** | A default tier for a whole module | An attribute on the package line, for example `@tier(low) package game.physics`. An explicit tier on a function still wins. |

---

## 1. Where things stand (probed 2026-11-15 on live master; imports, visibility and initialization updated when MS1 landed, 2026-11-16)

Each line was confirmed by running a program through `ubel check` or `ubel run`, or by reading the
source named.

**Imports and `package`**

- The grammar is `summon a.b.C [as X]` and `from a.b summon X` or `from a.b summon [X, Y]`
  (`docs/ubel.ebnf`). Imports sit at the top of a file, after an optional `package` line.
  `summon` inside a function body and `summon` between items are both `PARSE-001`.
- `summon std.*` is checked against a table of the names the compiler provides
  (`crates/core/src/builtins/std_modules.rs`, tested against the builtins registry). A member the
  module does not have, a module that does not exist, a module path with no member and a path
  outside `std` are `NAME-003`, once per path. The local name is still declared, so a use adds no
  `undefined name`. A path outside `std` cannot resolve before multi-file loading (MS2). Name
  resolution registers a `DefKind::Import` per name.
- A builtin renamed by an import works: after `summon std.collections.List as L`, `L.new()` and
  `L<int>` are the builtin `List`. Name resolution records a use of `L` as a use of the builtin,
  type inference maps a name written in a type through `SemaContext.builtin_aliases`, and the
  interpreter binds the new name from the program's imports. A rename may not take the name of
  another builtin, another import or an item in the file (`NAME-002`). The interpreter ignores
  imports in every other respect, and importing a builtin stays optional.
- `as` inside a list (`from std.collections summon [List as L, Dictionary]`) renames one name.
  `ImportItem` carries the name and the optional rename. A missing name after `as` is `PARSE-001`.
- `package a.b` is parsed and ignored. `@tier(low) package a.b` is `PARSE-001`.

**Visibility**

- `pub` is recorded on functions, structs, enums, traits, statics, constants and type aliases
  (the last two since MS1), and the AST has it on fields and methods. Nothing in sema or the
  interpreter reads visibility.

**Names and tables**

- Sema identifies a user type by `DefId` (`SemaType::Named { def, args }`), but the name table is
  one flat map for the whole program (`SemaContext.top_level: HashMap<String, DefId>`).
- The interpreter is keyed by name strings: `method_table`, `enum_table`, `struct_derives`,
  `struct_field_order`, `statics`, `type_aliases`, `trait_names` and `trait_method_table`, and
  top-level functions are defined into the environment by name. Two modules that each define a
  `struct Token` or a `fn helper` would collide there.
- Several sema tables are keyed by `Span` (`expr_types`, `int_literal_types`, `trait_call_sites`,
  `resolutions`, `binding_types`). A `Span` is `(start, end, line, column)` with no file. The
  prelude already solves the same problem for its own source: its spans are shifted to start at
  2^31 (`PRELUDE_SPAN_START`), which fits a 32-bit `usize` for the wasm playground and keeps a
  prelude node from colliding with a user node (`crates/core/src/lexer/token.rs`).

**Initialization**

- `const A: int = B + 1` with `const B: int = A + 1` is `NAME-009`, for constants and statics
  alike. The check also follows top-level functions called in initializers (2.5). A cycle through
  a method, a function value or a trait object is not followed and still stops the program at
  startup.
- **Gap, not fixed in MS1.** A `const` initializer that calls a function which reads another
  `const` stops at startup with `undefined name`, with no cycle involved. A function's closure is
  a snapshot of the module scope taken before any constant is defined, and the second snapshot,
  which includes the constants, is taken after every initializer has run. A `static` initializer
  is not affected, because statics live in their own table. Passing the constant to the function
  as an argument works.

**Tooling**

- `ubel check`, `ubel run`, the LSP and the wasm playground all take one source text
  (`check_source`).
- The self-hosted compiler is one file, `self-hosted/tokenizer/tokenizer.ubl` (136 lines, no
  imports), with a parser and a type checker planned as further files. It is the first real user
  of a module system.

---

## 2. The model

### 2.1 Modules and names (M1)

- Every `.ubl` file is a module. The root is the directory of the entry file. Module `a.b` is the
  file `<root>/a/b.ubl`. A directory is not a module; `a/b.ubl` and `a/b/c.ubl` can coexist.
- The module name comes from the path. A `package` line is optional. When present, it must equal
  the path-derived name, which catches a file that was moved or renamed without its line.
- A path segment cannot be a reserved word (`low`, `mid`, `high`, `with`, `using`, `Task`, and the
  other keywords).
- `std.*` is built into the compiler for now (2.6).

### 2.2 Imports

| Form | Meaning |
|---|---|
| `summon a.b` | Names the module `a.b`; its items are used as `b.item`. |
| `summon a.b as m` | Same, under the name `m`. |
| `summon a.b.C` | Imports item `C` of module `a.b`. |
| `from a.b summon C` | Imports item `C` of module `a.b`. |
| `from a.b summon [C, D as E]` | Imports several items, with an optional rename each (`as` inside the list landed in MS1). |
| `pub summon ...` | Any form above, re-exported to the importers of this module. |

- **Path resolution.** For `a.b.C` the longest prefix that names a module wins and the rest names
  an item. If both a module `a.b.C` and an item `C` in module `a.b` exist, the path is ambiguous
  and is an error.
- An import is private to the importing module unless it is `pub summon`.
- No glob imports in v1.
- An import whose name equals an item declared in the same module, or another import, is an error.
- The reversed form in the ideas document, `summon [A, B] from X`, is not adopted. The shipped
  `from X summon [A, B]` stays.

### 2.3 Visibility (M4)

- Everything is private to its module unless marked `pub`. `pub` applies to functions, structs,
  enums, traits, statics, constants and type aliases (`pub const` and `pub type` start to take
  effect), and to struct fields and methods individually.
- A struct literal for a struct with a private field is an error outside the defining module. The
  same holds for field access and for a pattern that names a private field.
- Enum variants have the visibility of their enum. Trait methods have the visibility of their
  trait. A method in an `impl Trait for Type` block has the visibility of the trait method.
- A `@derive` expands inside the defining module, so it can reach private fields.
- A private item used from another module is a new diagnostic (code to be allocated).

### 2.4 Impls and trait methods (M2)

- Impls are not importable items. Once a module is part of the program, its impls apply
  everywhere. One impl per trait and type pair holds for the whole program, and an overlap is an
  error that names both modules.
- The orphan rule is per package. In v1 a program is one package, so any module may hold any
  impl. When packages exist, an impl is allowed only in the package that declares the trait or
  the type.
- A method of a trait is callable with dot syntax only when the trait is in scope. The prelude
  traits (`Eq`, `Ord`, `Hash`, `Clone`, `PartialEq`, `PartialOrd`, `Display`, `Iterator` and the
  operator traits) are always in scope.

### 2.5 Cycles and initialization (M3)

- A cycle between modules is allowed. The compiler already pre-declares every top-level name
  before it checks any body, so two files that refer to each other behave like two functions of
  one file.
- Constants and statics initialize once, before `main`, in dependency order across the whole
  program. A cycle among them, through initializers, is a compile-time error (`NAME-009`) that
  names the items on the cycle. MS1 reports it within one file. The modules are named once files
  are loaded (MS2).
- The check builds a graph with one node per constant, static and top-level function, and an edge
  to every constant, static or function a body or initializer names. A constant or static that
  can reach itself is on a cycle. Each separate cycle is one report. Recursion among functions
  alone is not a cycle.
- Limits, stated so the check is not read as exact. It is conservative: a function that names the
  constant on a branch that never runs during initialization is still an edge, so
  `const A = f(false)` with `fn f(b: bool) int { if b { return A } return 0 }` is rejected. It
  does not follow a method called with dot syntax, a call through a function value held in a
  variable, or a call through a trait object, and a cycle through one of those still stops the
  program when it starts. A closure written inside a function body counts as part of that
  function.

### 2.6 The standard library and builtins

- The builtins (`List`, `println`, the collections, the prelude traits) stay global with no
  `summon`, as today.
- `summon std.*` is checked against a table of the names the compiler provides, grouped as
  `std.collections` (`List`, `Dictionary`, `Set`, `Queue`, `Stack`, `InlineList`), `std.memory`
  (`Pool`, `Unique`, `Shared`, `SyncShared`), `std.task` (`Task`), `std.io` (`println`, `print`, `log`), `std.debug`
  (`assert`, `panic`, `typeof`), `std.math` (`Math`, `sqrt`, `abs`, `min`, `max`, `floor`, `ceil`,
  `range`) and `std.convert` (`to_string`, `to_int`, `to_float`, `to_double`). An unknown path is
  `NAME-003`. The checked form stays optional for builtins. A test fails when a builtin is added
  to the registry and not placed in the table.
- A standard library written in Ubel source is a later step and needs the module system first.

### 2.7 A default tier for a module (M5)

```ubel
@tier(low) package game.physics
fn step(dt: double) void { ... }              // LOW without writing @tier
@tier(high) fn debug_dump() void { ... }      // an explicit tier wins
```

The attribute list before `package` applies to the whole file. `@tier` sets the default tier of
every function and method of the file that has no tier of its own. The precedence, strongest
first: an attribute on the function or method, a tier block, a struct's `@tier` for its methods
(S4), the module default, then HIGH. `@cfg(...)` on the package line could later gate a whole
file. This needs a small parser change.

### 2.8 Implementation approach (a plan, not a decision)

- **A source map.** Each file gets its own range of offsets, so every `Span`-keyed table stays
  valid with no change to `Span`. The prelude is the precedent. The total size of user sources
  must stay below 2^31 bytes, the start of the prelude range.
- **A loader with a source provider.** The provider reads from disk, or from memory for the wasm
  playground, the LSP buffers and tests. It builds the module graph from the entry file.
- **Per-module scopes in name resolution,** with `top_level` becoming per module.
- **Canonical names.** Sema assigns each item a unique canonical name (for example
  `game.physics::Body`) and the interpreter and the LLVM emitter look types, methods, statics and
  trait tables up by it, through facts that sema records by `Span`, the same way
  `trait_call_sites` works today. The alternatives, a flattening rewrite of the AST or an
  interpreter keyed by `DefId`, are larger. This is the main cost of the whole feature.
- **Diagnostics** name the file and show module-qualified item names.

---

## 3. Cost

- The canonical-name work in the interpreter and the LLVM emitter (2.8) is the bulk of the effort.
  Sema already identifies types by `DefId`.
- Per-item `pub` on fields makes data-heavy structs, such as ECS components, noisier to write.
- A whole-program check of every module in a cycle, which the compiler already does for a program.
- Parser changes: `as` inside an import list, attributes on the package line, `pub` kept on
  `const` and `type`.

---

## 4. Verdicts on `docs/MODULE_SYSTEM_IDEAS.md`

| Proposal | Verdict | Reason |
|---|---|---|
| `summon` keyword, dotted paths | Keep | Shipped. |
| `summon [A, B] from X` | Change | The shipped `from X summon [A, B]` stays, and `as` is added inside the list. |
| Every file a module, private by default, `pub` exports | Keep | The grammar already has it (2.1, 2.3). |
| Scope-local `summon` | Parked | Imports are file-level in the grammar, and the gain is small. |
| Tier ceiling on a `summon` | Parked | It needs a transitive tier check. The module default tier (M5) is the cheap useful piece. |
| `with [..]` features, `with (..)` parameters | Parked | `with` is the allocator block, `@cfg(feature = ..)` exists, and parametric modules are a type-system feature. |
| `grimoire.ubl` manifest in Ubel syntax | Later | It belongs to the package design. The rejection of `.mdix` for the manifest stands. |
| `grim.ubl` directory entry point | Changed | A facade is an ordinary file with `pub summon`. |
| Content-addressed caching | Parked | A build subsystem of its own. |

---

## 5. Defaults taken without a separate question

None of these has been confirmed. Each is settled by the slice that needs it, as options with a
recommendation, if there is an objection.

1. The import forms in 2.2, including `as` inside a list, `pub summon`, the ambiguity error and no
   globs.
2. Builtins stay global and `summon std.*` is validated (2.6).
3. The root is the entry file's directory and a directory is not a module (2.1).
4. `pub const` and `pub type` take effect (2.3).
5. A name conflict between an import and a local item is an error (2.2).
6. The precedence of the module default tier (2.7).

---

## 6. Open details

1. **Shadowing a builtin.** Whether a module item or an import may reuse the name of a builtin
   (`List`, `println`). Recommended: a module item may shadow a builtin within its module and an
   import may not, because a builtin never needs summoning. MS1 enforces the import half: a
   rename may not take the name of another builtin (`NAME-002`).
2. **Diagnostic codes.** A private item used across modules, a `package` line that does not match
   the path, an ambiguous import path, an initialization cycle, an overlapping impl across
   modules. Allocated in `DIAGNOSTICS_RULES.md` at the slice that introduces each.
3. **The table behind `summon std.*`.** Built in MS1 from the builtins registry (2.6). The grouping
   is a first cut, not a decision: only `std.collections.List` and `std.io.println` were fixed by
   an existing fixture. A name can move between modules while no program depends on it.
4. **Initialization order between independent modules.** Dependency order is fixed by the
   initializers; the order among unrelated modules should be deterministic (recommended: by
   module name).
5. **Tooling.** The `--json` output gains a file field. The LSP needs a workspace notion and open
   buffers as sources. The playground gains a multi-file source provider later.
6. **Packages.** The manifest, the dependency graph, the name of a distributable unit, and the
   package-level orphan rule are a separate design.

---

## 7. Delivery order

Design first, then slices, each with fixtures, mutation checks and a fresh-clone verification, as
for traits.

| Slice | Content | Depends on |
|---|---|---|
| **MS0** | This document and the public status page. | none |
| **MS1** | **Done 2026-11-16.** Single-file groundwork that fixed bugs which existed: `as` inside an import list, `summon std.*` validated (`NAME-003`) and a builtin rename that works, a compile-time initialization-cycle error for constants and statics (`NAME-009`), `pub const` and `pub type` recorded. Attributes on the package line moved to MS5, because parsing `@tier(low) package a.b` without applying it would add a new accept-and-ignore gap. | MS0 |
| **MS2** | The source map, the loader with a source provider, the module graph, per-module name resolution, visibility checks, import conflicts. `ubel check` on several files. | MS1 |
| **MS3** | Canonical names in sema, the interpreter and the diagnostics, so a multi-module program runs. | MS2 |
| **MS4** | Impls and trait scope across modules (M2), initialization order across modules. | MS3 |
| **MS5** | The module default tier (M5) and attributes on the package line (`@tier`, `@cfg`). | MS1 |
| **MS6** | CLI, LSP and playground with several files, and the self-hosted compiler split into modules as the first user. | MS3 |
| **MS7** | Packages: the manifest, dependencies, the orphan rule between packages. A separate design. | MS6 |
