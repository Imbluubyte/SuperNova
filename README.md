# SuperNova

A fast, statically typed, Python-inspired language that compiles ahead-of-time to
native code (via C and clang) and is designed to build and run on Android/Termux.

**Status: early development.** SuperNova programs using variables, numbers,
strings, f-strings, control flow (`if/orif/else`, `while`, `for` over ranges,
`match`), and functions (generic, recursive, default and named arguments) compile
to native executables today. Classes, collections, errors, async and modules are
parsed but not implemented yet; see [ROADMAP.md](ROADMAP.md).

* [Language reference](docs/LANGUAGE.md) (implemented subset)
* [Architecture and design decisions](docs/ARCHITECTURE.md)

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

Termux: `pkg install rust clang git`, then the same commands. No crates are
downloaded; the compiler has zero dependencies.

## Language decisions

* Ranges: `1..5` is inclusive (1,2,3,4,5), `1..<5` excludes the end. Inclusive
  is the default because indexing is 1-based, so `1..list.length()` covers every
  element.
* `orif` replaces `elif`; semicolons are a syntax error; tabs in indentation are
  rejected.
