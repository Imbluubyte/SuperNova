# SuperNova

A fast, statically typed, Python-inspired language that compiles ahead-of-time to
native code (via C and clang) and is designed to build and run on Android/Termux.

**Status: early development.** Implemented so far: lexer (indentation, f-strings)
and parser for the full V1 surface syntax. Type checker, code generation and
runtime are in progress. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for design
decisions and the roadmap for what is next.

## Building the compiler

```
cd compiler
cargo build --release
cargo test
./target/release/snc check ../examples/hello.sn
```

Termux: `pkg install rust clang git`, then the same commands. No crates are
downloaded; the compiler has zero dependencies.

## Language decisions

* Ranges: `1..5` is inclusive (1,2,3,4,5), `1..<5` excludes the end. Inclusive
  is the default because indexing is 1-based, so `1..list.length()` covers every
  element.
* `orif` replaces `elif`; semicolons are a syntax error; tabs in indentation are
  rejected.
