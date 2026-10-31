# SuperNova

A fast, statically typed, Python-inspired language that compiles ahead-of-time to
native code (via C and clang) and is designed to build and run on Android/Termux.

**Status: early development.** SuperNova programs using variables, numbers,
strings, f-strings, lists (1-based, elementwise arithmetic, `filter`), tuples, control flow (`if/orif/else`, `while`, `for` over ranges,
`match`), and functions (generic, recursive, default and named arguments) compile
to native executables today. Errors (try/catch) and async are
parsed or planned but not implemented yet. Memory is managed by a built-in garbage collector ([docs/MEMORY.md](docs/MEMORY.md)); see [ROADMAP.md](ROADMAP.md).

* [Language reference](docs/LANGUAGE.md) (implemented subset)
* [Architecture and design decisions](docs/ARCHITECTURE.md)
* [Memory management and the garbage collector](docs/MEMORY.md)
* [**Android / Termux guide**](docs/TERMUX.md)

```
var name = "Player"
fn fib(n: Int) -> Int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)
print(f"Hello {name}! fib(20) = {fib(20)}")
```

## Building the compiler

```
cd compiler
cargo build --release
cargo test
./target/release/snc run   ../examples/hello.sn     # compile and run
./target/release/snc build ../examples/hello.sn -o hello
./target/release/snc emit-c ../examples/hello.sn    # inspect generated C
```

`snc` needs a C compiler at run time (`$CC`, else `clang`, `cc`, `gcc`).

Termux: see [docs/TERMUX.md](docs/TERMUX.md) (or run `scripts/termux-install.sh`). No crates are
downloaded; the compiler has zero dependencies. After installing, `snc doctor` checks that
everything works on your machine.

## Language decisions

* Ranges: `1..5` is inclusive (1,2,3,4,5), `1..<5` excludes the end. Inclusive
  is the default because indexing is 1-based, so `1..list.length()` covers every
  element.
* `orif` replaces `elif`; semicolons are a syntax error; tabs in indentation are
  rejected.
