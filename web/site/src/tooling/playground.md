# The Playground

The [playground](/playground/) runs the real Ubel Stratum compiler and
interpreter in your browser, compiled to WebAssembly. There is nothing to
install and your code never leaves the page.

## Using it

1. Type Ubel source in the left pane, or pick a program from the **fixture
   picker**. The picker lists the project's own test programs: `Passing
   (ok_)` ones compile and run, `Errors (err_)` ones fail on purpose, which
   makes them a fast way to see what a given error looks like.
2. Press **Run** (or **Ctrl/Cmd+Enter**).
3. The right pane shows the pipeline stage reached
   (`tokenize` → `parse` → `sema` → `interpret`), any diagnostics, and the
   program's output.

A program is run through `fn main()`. If the file has no `main`, the lex,
parse and type-check stages still run and the pane tells you there was
nothing to execute.

## Diagnostics link to the docs

Every diagnostic starts with `error[CODE]`. In the playground the code is a
link to its entry in [the error reference](../errors/index.md): what it means,
a failing example, and the fix. Try loading `err_duplicate_def` from the
fixture picker and click `NAME-002`.

The links go to `/docs/latest/`, which is the newest release once one is
published and the development docs until then. See
[Versions and releases](../versions.md).

## Changing the syntax colours

The **Colours** button in the header (and the palette icon in the docs top
bar) opens a small panel:

- **Preset**: Ubel (the default), Monokai style, Solarized dark and light,
  GitHub light, and High contrast.
- **Per-token colours**: background, plain text, keywords, types, tier words
  (`mid`, `arena`, ...), strings, numbers, comments, attributes (`@tier`),
  functions and constants. Pick with the colour control or type a hex value.
- A **live preview** of a real program, and a *low contrast* warning beside
  any colour that is hard to read on the chosen background.
- **Reset to default**, and **Share or import a palette**: copy your palette
  as JSON, or paste one to apply it.

The choice is saved in your browser (local storage), so it carries across
the playground, the docs and the home page, and across tabs. Nothing is
sent anywhere. If your browser blocks storage the colours still change for
the current page; they just are not remembered.

## Limits

- Programs that read input or touch the file system are not possible:
  there is no standard library yet.
- `with arena` blocks are validated by the tier checker but do not yet
  allocate real arena memory in the interpreter; see
  [The Tier Model](../tier-model.md#what-is-enforced-today).
- Very large or deeply nested programs can hit the browser's stack limit
  earlier than the command line, which runs on a 256 MB stack.

If the page says *"Something went wrong inside the playground itself"*,
that is a compiler bug rather than a mistake in your program; please report
it with the source that triggered it.
