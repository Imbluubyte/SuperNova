# Roadmap

- [x] 1. Architecture document
- [x] 2. Lexer and parser (full V1 surface syntax)
- [x] 3. Type checker: variables, expressions, inference
- [x] 4. C emitter + runtime: control flow, functions, strings
- [ ] 5. Classes and inheritance
- [x] 6a. Lists, tuples, mixed lists, filter, elementwise arithmetic
- [ ] 6b. Dictionaries
- [ ] 7. Modules and `private`
- [x] 8a. Garbage collector, stack-overflow and out-of-memory handling (docs/MEMORY.md)
- [ ] 8b. Errors (try/catch/finally)
- [ ] 9. async/await
- [ ] 10. Standard library
- [ ] 11. Termux build and on-device testing
- [ ] 12. Optimisation passes and benchmarks
- [ ] 13. Nebula integration interface

## Known limitations
* The collector is stop-the-world and non-incremental; the whole live pointer-containing heap is traced each cycle (docs/MEMORY.md).
* Only tested on x86-64 so far; the Android/ARM64 run is the next verification step (docs/TERMUX.md, `snc doctor`).
* Expression temporaries are not fused (`xs * 2 + 1` allocates two lists).
* A list filtered from a mixed list is still a mixed list (no type narrowing yet).
* `for` over a range ending at the maximum `Int` never terminates.
* Generic (unannotated) functions are only type-checked when they are called.
