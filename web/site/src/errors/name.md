# Name resolution (NAME)

Name resolution connects every name to its definition: variables, functions, types, imports, `self`, constants and statics.

Every example below was compiled with the real `ubel` binary, and the output shown is exactly what it printed. Entries marked *no verified example yet* come from the compiler's diagnostics registry (`docs/DIAGNOSTICS_RULES.md`) but could not be reproduced from ordinary source when this page was written; the registry text is given as is.

## Name resolution

### NAME-001

**UndefinedName** — `` undefined name `x` ``

A name is used that no `let`, parameter, function, type or import in scope defines. When a close match exists, the diagnostic adds `did you mean ...?`.

<!-- ubel-test: error NAME-001 -->
```ubel
fn main() {
    let total = count + 1
}
```

```text
>>> error[NAME-001]: undefined name `count`
  --> 2:17
    |
  2 |     let total = count + 1
    |                 ^^^^^
<<<
```

**Fix:** Define the name before use, import it, or fix the spelling.

### NAME-002

**DuplicateDefinition** — `` `x` is already defined in this scope ``

A second definition of the same name in one scope. A secondary note points at the first definition.

<!-- ubel-test: error NAME-002 -->
```ubel
fn main() int {
    let x = 1
    let x = 2    // ERROR: `x` is already defined in this scope
    return x
}
```

```text
>>> error[NAME-002]: `x` is already defined in this scope
  --> 3:5
    |
  3 |     let x = 2    // ERROR: `x` is already defined in this scope
    |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  ~> note: first defined here
    --> 2:5
      |
    2 |     let x = 1
      |     ^^^^^^^^^
  <~
<<<
```

**Fix:** Rename one of them. A `let` in an inner block may shadow an outer name; two in the same block may not.

### NAME-003

**UnresolvedImport** — `` cannot resolve import path `x` ``

A `summon` path could not be resolved.

*No verified example yet.*

**Fix:** Check the path.

### NAME-004

**UnresolvedPathSegment** — `` no member `seg` in `resolved-so-far` (while resolving `full-path`) ``

A dotted path resolved up to a point, and the next segment is not a member of what it resolved to.

*No verified example yet.*

**Fix:** Check the segment's spelling and the path before it.

### NAME-005

**SelfOutsideMethod** — `` `self` can only be used inside a method body ``

`self` appears in a function that is not a method.

<!-- ubel-test: error NAME-005 -->
```ubel
fn broken() int {
    return self.value    // ERROR: `self` is not valid here
}
```

```text
>>> error[NAME-005]: `self` can only be used inside a method body
  --> 2:12
    |
  2 |     return self.value    // ERROR: `self` is not valid here
    |            ^^^^
  ~> help: move this code into a method that takes `self` as a parameter
  <~
<<<
```

**Fix:** Move the code into a method whose first parameter is `self`.

### NAME-006

**UnresolvedTypeParam** — `` unknown type parameter `x` ``

A type parameter name is used that the enclosing declaration does not declare.

*No verified example yet.*

**Fix:** Declare it in the `<...>` list, or fix the name.

### NAME-007

**AssignToConst** — `` cannot assign to constant `x` ``

A `const` is initialised once and cannot be assigned, with `=` or a compound operator. A local `let` that shadows the constant's name stays assignable.

<!-- ubel-test: error NAME-007 -->
```ubel
const LIMIT: int = 5

fn main() void {
    LIMIT = 6
    println($"{LIMIT}")
}
```

```text
>>> error[NAME-007]: cannot assign to constant `LIMIT`
  --> 4:5
    |
  4 |     LIMIT = 6
    |     ^^^^^
  ~> help: constants are initialized once; use a `let` binding if the value needs to change
  <~
<<<
```

**Fix:** Use a `let mut` binding if the value must change.

### NAME-008

**StaticInConst** — `` a constant cannot read the static `x` ``

A `const` is evaluated once at startup, so it cannot depend on a mutable global's current value. A `static` initialiser reading a const or another static is fine.

<!-- ubel-test: error NAME-008 -->
```ubel
static BASE: int = 4
const DOUBLE: int = BASE * 2

fn main() void {
    println($"{DOUBLE}")
}
```

```text
>>> error[NAME-008]: a constant cannot read the static `BASE`
  --> 2:21
    |
  2 | const DOUBLE: int = BASE * 2
    |                     ^^^^
  ~> help: make the other item a `const` too, or initialize this value from a `static`
  <~
<<<
```

**Fix:** Make the other item a `const` too, or initialise this value from a `static`.
