# CLAUDE.md — SuperNova

Read this first; it exists so you do not have to explore the repo. SuperNova is a statically
typed, Python-inspired, ahead-of-time language: a Rust compiler (`snc`) that emits C99, which
clang/gcc compiles. Android/Termux is the primary target. Repo: `Imbluubyte/SuperNova`, work
directly on `main`. The Nebula engine (Kotlin, 2D node-based) is a separate project that embeds
SuperNova as its scripting language.

## Working rules (save tokens)

* **Do not read whole big files.** `codegen.rs` is ~1800 lines. Use `Grep -n` for the function
  name, then `Read` with `offset`/`limit` ~60 lines. Use the map below to pick the file.
* **No Explore agents** for this repo; the map below is enough.
* Batch independent tool calls in one message. Chain `cargo build && run` in one Bash call.
* Make multi-spot edits with one Python script (`s.replace(old, new, 1)` with `assert old in s`)
  instead of many Edit calls. Rust build errors tell you exactly which `match` needs a new arm:
  add the AST/Ty variant first, build, and let the compiler list the places.
* Filter cargo noise: `cargo build 2>&1 | grep -E "^error" -A8 | head -30` (there is one
  harmless `unused mut` warning in `lexer.rs`). Use `cargo test -q 2>&1 | grep -E "test result|FAIL|panick|differs"`.
* Do not repeat verification the tests already do. Run the full suite once at the end of a change.
* Keep final replies short: what changed, any limitation, one next step. User is on a phone.
* Commit message trailers and PR text: follow the attribution reminder in the session.
  Push with `git push origin HEAD:main` (the "repository moved" notice is harmless).

## Commands

```
cd compiler && cargo build            # debug build: target/debug/snc
cargo build --release                 # needed for benchmarks (benchmarks/run.sh uses PATH snc, else release)
snc run f.sn | build f.sn [-o out] [--lib] | emit-c f.sn | check f.sn | doctor | version
snc ... -I folder                     # extra module search folder (also env SN_PATH)
cargo test -q                         # 16 e2e tests, ~15 s (compiles+runs every tests/ case, also under GC stress)
SN_GC_STRESS=1 ./prog                 # collect on every allocation (finds GC bugs)
```

Top-level statements form the program (no `main` function). Indent with **tabs** (spaces also
accepted, never mixed in one file). Keywords: `orif` (never `elif`), `and/or/not`, `has`, `repeat(n):`,
lists/tuples/strings are 1-based, `private`, `async/await`, `try/catch/finally`.

## Pipeline and where things live (`compiler/src/`)

lexer.rs (tokens, indentation) → parser.rs (AST in ast.rs) → modules.rs (load files, link,
rename with `m{idx}_{stem}__` prefix) → codegen.rs (+ codegen/*.rs: type check and emit C) →
`main.rs` writes the runtime headers to a temp dir and runs `$CC`/clang/cc/gcc (`-O2`).

| To change / add… | Files |
| --- | --- |
| a token or keyword | `lexer.rs` (`keyword` table, `Tok` enum + `Display`), then parser |
| syntax (expression, statement, declaration) | `ast.rs` (variant), `parser.rs` (`statement`, `primary`, `cmp_expr`…), **`modules.rs` `Rn::expr`/`stmt`** (renamer must walk new nodes, incl. scoping of new binders), `codegen.rs` `expr()`/`stmt()` |
| a new type | `Ty` enum in `codegen.rs` (+ `name`, `has_ptrs`), `resolve_type`, `cty`/`type_name` and `buf_append` in `codegen/collections.rs`, `coerce`, `binary()` |
| operators / comparisons | `binary()` in `codegen.rs`; list ops `list_binary` (collections.rs); optional equality `opt_equality` (optional.rs); `any_binary` |
| classes, inheritance, `self`, access | `codegen/classes.rs` (layout, ctor, `call_method`, `member_expr`) |
| list/tuple/index/for-each/methods | `codegen/collections.rs` (`method_call` is the dispatcher for `x.name(...)`) |
| dict | `codegen/dicts.rs` + `SN_DEFINE_DICT` in `runtime/snrt.h` |
| strings and stdlib intrinsics `__name` | `codegen/stdlib.rs` (`intrinsic` table, `str_method`) + helpers in `runtime/snrt.h`; Sn-source modules in `stdlib/*.sn` (embedded via `STD` in `modules.rs`) |
| builtins (`print`, `str`, `int`, `sleep`, `getInput`…) | `builtin()` in `codegen.rs`, name list `BUILTINS` + `did_you_mean` list in `call_inner` |
| optionals / null safety / narrowing | `codegen/optional.rs` (`if_stmt`, `while_stmt`, `cond_narrow`, `coalesce`, `force_expr`), narrowing state in `Ctx` |
| lambdas / function values | `codegen/lambda.rs` (closures = `{fnptr, env}` struct + thunks), `codegen/funcs.rs` (`call_value`, `map/each/reduce/any/all`) |
| enums, `.type()`, type names as values | `codegen/enums.rs` |
| try/catch/finally/throw | `codegen/errors.rs` + try machinery in `runtime/snrt.h` |
| async/await | `codegen/async_.rs` + `runtime/snasync.h` (stackful coroutines, ARM64 asm untested on device) |
| host embedding (`extern fn`, `export fn`, `export class`, `--lib`, handles, `sn_pump`) | `codegen/embed.rs`, `generate()` tail in `codegen.rs`, `build_with` in `main.rs`; docs `docs/EMBEDDING.md`, `docs/NEBULA.md` |
| error messages / "did you mean" | `did_you_mean` at the bottom of `codegen.rs`; per-site `err(...)` |
| GC / memory | `runtime/sngc.h` (conservative mark & sweep; roots via `sn_gc_root`, `sn_gc_extra` hook) |
| CLI, doctor, C compiler flags | `compiler/src/main.rs` |

Key codegen types: `E{c, ty, pure, fused}` (an emitted C expression), `Var`, `Ctx` (per-function
state: scopes, narrowing, try stack), `Gen` (global state: `fns`, `classes`, `insts` = monomorphised
function instances, `protos`/`bodies`/`type_decls`). Functions are monomorphised per argument
types in `instance()`. Expressions compile to C statement-expressions `({ ... })`.

## Tests (`tests/`)

* `cases/NAME.sn` + `NAME.out`: program output must match exactly (also run with GC stress).
  Regenerate an expected file with `snc run` only after checking the output is correct.
* `errors/NAME.sn` + `NAME.err`: compile must fail and stderr must contain the `.err` text.
* `runtime_errors/`: runtime failure with location. `projects/`: multi-file module tests.
* `embed/`, `embed2/`, `embed3/`: `--lib` builds driven by a C `host.c` (`expected.out`);
  run through `run_embed` in `compiler/tests/e2e.rs`. `stdin_demo.sn` is used by the stdin test.
* New feature checklist: a `cases/` test, one or two `errors/` tests, `docs/LANGUAGE.md` section,
  tick `ROADMAP.md`, adjust `docs/CHEATSHEET.md` / `README.md` status lines if they mention it.
* Top-level variable names in one test file must be unique (redeclaration is an error).

## Gotchas learned the hard way

* Top-level variables are C globals (`g_name`, `shared: true`), so narrowing of globals is dropped
  after any non-builtin call. Class fields are never narrowed.
* Class fields need initialisers (`var x = 0`); inside methods fields are used bare (no `self.`
  needed); `onCreation` is the constructor; classes cannot be nested in functions.
* `/` always yields Float, `//` Int. `Int` does not convert to `Float` in lambda return positions.
* Function values are `{f, env}` structs: always call through `.f(.env, ...)`; `has_ptrs(Func)` is true.
* Anything that can hold heap pointers must be GC-visible: non-atomic allocs, roots registered in `main`/`sn_init`.
* Locals assigned inside `try` bodies are `volatile` (`assigned_in_try`); keep that when adding statements.
* Exported/extern names crossing the host boundary use the *bare* name; module-mangled names
  (`m1_x__name`) are internal. `bare_name()`/`unmangle()` undo mangling for messages.
* Only Int/Float/Bool/String cross the host ABI. `async` works in `--lib` builds only via `sn_pump()`.
* `cargo test` e2e tests need a C compiler on PATH (clang or gcc) and `dlopen` (`-ldl`).
* There is no `/usr/bin/time` here; measure with `time` or Python `resource`/`subprocess`.
* Do not edit `.out` files by hand to make a test pass; fix the cause.

## Docs map (read only what the task needs)

`docs/LANGUAGE.md` language reference (add new feature sections before "Standard library"),
`docs/CHEATSHEET.md` one-page syntax, `docs/ARCHITECTURE.md` design decisions,
`docs/MEMORY.md` GC, `docs/EMBEDDING.md` host API, `docs/NEBULA.md` engine integration,
`docs/TERMUX.md` install/update on Android, `ROADMAP.md` checklist.

## Status

Implemented: variables/types, f-strings, control flow, functions (generic, default/named args),
lists/tuples/dicts, classes (single inheritance), modules + `private`, try/catch/finally,
async/await, strings + small stdlib (`std.math/random/time/sys/fs`), lambdas, enums, optionals +
null safety, `has`, `repeat`, `getInput()`, `.type()`, host embedding for Nebula, GC.
Ideas not done: `snc fmt`, generic classes, interfaces/traits, struct-like value types,
object handles passed *as arguments* across the host boundary, test framework (`snc test`),
on-device Android check of a Termux-built `.so`.
