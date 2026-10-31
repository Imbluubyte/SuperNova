# SuperNova Architecture

This document records the major architectural decisions and the reasoning behind
them. It is a living document: when a decision changes, the change and the reason
are recorded here.

## 1. Pipeline overview

```
 .sn source
    │
    ▼
 Lexer  (indentation → INDENT/DEDENT tokens, f-string parts)
    │
    ▼
 Parser (hand-written recursive descent + Pratt expressions) → AST
    │
    ▼
 Resolver / Type checker (inference, monomorphisation of untyped params)
    │
    ▼
 Typed IR  (optimisations: const folding, DCE, inlining, list specialisation)
    │
    ▼
 C99 emitter  ──►  clang (Termux: `pkg install clang`)  ──►  native ARM64 binary
                          ▲
                          └── links libsnrt (the SuperNova runtime, plain C)
```

## 2. Key decisions

### 2.1 Compiler implementation language: Rust, zero crate dependencies

* Termux ships `rust` and `cargo` as packages, and `aarch64-linux-android` is a
  supported Rust target, so the compiler builds and runs on-device.
* No external crates: `cargo build --release` works offline and nothing has to
  be fetched or compiled from C sources on a phone.
* Strong enums/pattern matching suit AST and IR work; memory safety keeps the
  compiler maintainable.
* Alternatives considered: Python (easy for the author, but slow to compile big
  programs and a poor fit for a self-hosting future), C (portable, but
  error-prone for a type checker), Go (fine, but larger binaries and less
  natural enum modelling).

### 2.2 Backend: emit C99, compile with clang/cc

* **Why not LLVM directly?** LLVM libraries are enormous, slow to build, and
  painful to link on Android. Termux already ships `clang`, which *is* the LLVM
  backend, so emitting C gives us LLVM's optimiser (inlining, vectorisation,
  ARM64 codegen) for free with a single, very stable dependency.
* **Why not Cranelift/QBE?** Cranelift needs crates and a large Rust build;
  QBE is not packaged for Termux and has no ARM64 Android story we can rely on.
* **Why not a bytecode VM?** The brief prefers AOT native code and Performance
  is a first-class goal. A VM would also cost 5–50x on tight loops.
* **Why not a custom ARM64 backend?** Far too much work for little gain over
  clang -O2, and it would hurt PC portability.
* Generated C is portable, so PC support is automatic: any host with a C
  compiler works. The compiler discovers `$CC`, then `clang`, then `cc`, `gcc`.
* Cost: compile time includes a C compile. Mitigated by emitting compact C and
  caching the (precompiled) runtime object so only user code is compiled.

### 2.3 Typing model: static, inferred, monomorphised

* Every expression has a static type known at compile time.
* Untyped function parameters (`fn calculateDamage(attack, defence)`) are
  treated as **implicitly generic**: the function is specialised ("monomorphised")
  for each distinct tuple of argument types it is called with. This keeps
  the syntax from the brief, gives fully typed native code with no boxing, and
  gives compile-time errors when a specialisation is invalid.
* Annotated parameters are checked against the annotation and are not generic.
* Functions used as first-class values must have a single concrete signature
  (annotated, or inferable from the single use site); otherwise the compiler
  reports an error asking for annotations.
* Mixed-type collections use a tagged `Value` (see 2.5).

### 2.4 Memory management: conservative-root, non-moving mark-sweep

Implemented in `runtime/sngc.h`; full description in [MEMORY.md](MEMORY.md).

* Non-moving stop-the-world mark & sweep with size-class spans; the C stack and
  registers are scanned conservatively and heap objects word by word, except
  "atomic" objects (strings, numeric list storage), which are never scanned.
* Why conservative roots: the compiler emits C, and a precise scheme (shadow stack
  or stack maps) would tax every call and local variable. This keeps generated code
  free of GC bookkeeping.
* Value-typed data (Int, Float, Bool, tuples of primitives) never touches the GC.
* Rejected: reference counting (cycle leaks, atomic cost on ARM), copying GC
  (moves objects, complicates FFI and needs precise roots).
* Later options if pauses matter for games: incremental marking, generational
  collection with write barriers.

### 2.5 Value representation

* Statically known `Int`/`Float`/`Bool` are raw `int64_t`/`double`/`bool`.
* `String` is a pointer to an immutable, length-prefixed UTF-8 heap object.
* `null` is a distinct type; optional types are written `T?`.
* Dynamic `Value` (for mixed lists, dict values, `Any`): 16-byte tagged union
  (`tag`, payload). Chosen over NaN-boxing because Int is a full 64-bit and ARM64
  handles 16-byte pairs in two registers efficiently.

### 2.6 Lists

* Lists are typed at compile time where possible: `List[Int]` → contiguous
  `int64_t[]`, `List[Float]` → contiguous `double[]`, otherwise `List[Value]`.
* Representation is chosen by the type checker from literal / inferred element
  types, never by runtime guessing, so behaviour is predictable.
* A list that starts numeric but receives a non-numeric element via `add` is a
  *compile-time* type error unless declared `List[Any]`.
* Elementwise arithmetic on numeric lists compiles to simple loops clang can
  auto-vectorise to NEON.

### 2.7 Async

* Cooperative, single-threaded event loop with **stackless coroutines**:
  each `async fn` is lowered to a state machine (like Rust/C#).
  Awaiting suspends only that task; the loop keeps running other tasks.
* Real background work (file IO, networking, heavy compute) is dispatched to a
  small thread pool; completion re-enters the loop on the main thread. So
  `await` never blocks the main thread, and game code has no data races.
* Deferred to a later stage; see ROADMAP.

### 2.8 Modules

* One file = one module. `use a.b` resolves `a/b.sn` relative to the project
  root, then the standard library path. Whole program is compiled together
  (whole-program optimisation, dead-code elimination of unused functions).
* `private` is enforced at compile time at module (and later class) level.

### 2.9 Nebula separation

* The core compiler knows nothing about Nebula. `manages Node` is parsed as a
  generic *host binding declaration*: it declares a module-level global of an
  opaque host type that the embedder provides. `onFrame`, `onTimer`,
  `physicsProcess` are ordinary functions; the compiler can export their
  symbols with a stable naming scheme (`sn_<module>_<fn>`) for an embedder.
* Nebula APIs live in `nebula.*` modules provided by the engine side, not in
  the compiler.

## 3. Requirement notes / conflicts to flag

* **`onTimer(1.5)` is not valid function-declaration syntax** in a typed
  language: `1.5` is a literal in a parameter position. Proposal: the core
  language accepts `fn onTimer(interval: Float = 1.5)`-style declarations, and
  Nebula additionally reads a *decorator-like attribute*. Final decision is
  deferred to the Nebula integration stage and will be raised with the author
  before implementation; the core language will not special-case it.
* `filter(< 4)` introduces a *predicate expression* (operator + operand) that is
  valid only as a call argument. It is parsed as a dedicated `PredicateExpr`
  node and lowered to an inlined comparison loop. No lambdas are involved.
* 1-based indexing is applied uniformly to lists, tuples and strings.

## 4. Repository layout

```
compiler/      Rust compiler (lexer, parser, checker, IR, C emitter, driver)
runtime/       C runtime (strings, lists, GC, async loop)
stdlib/        SuperNova standard library modules (.sn)
examples/      Example programs
tests/         End-to-end tests (.sn + expected output)
benchmarks/    Benchmarks and comparison scripts
docs/          Documentation
```
