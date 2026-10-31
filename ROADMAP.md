# Roadmap

- [x] 1. Architecture document
- [x] 2. Lexer and parser (full V1 surface syntax)
- [x] 3. Type checker: variables, expressions, inference
- [x] 4. C emitter + runtime: control flow, functions, strings
- [ ] 5. Classes and inheritance
- [x] 6a. Lists, tuples, mixed lists, filter, elementwise arithmetic
- [ ] 6b. Dictionaries
- [ ] 7. Modules and `private`
- [ ] **8a. Garbage collector (URGENT: nothing is freed yet; see Known limitations)**
- [ ] 8b. Errors (try/catch/finally)
- [ ] 9. async/await
- [ ] 10. Standard library
- [ ] 11. Termux build and on-device testing
- [ ] 12. Optimisation passes and benchmarks
- [ ] 13. Nebula integration interface

## Known limitations
* **No memory is ever freed.** Every list, string and temporary is leaked until the program exits. A benchmark that builds 40 MB temporary lists in a loop reaches 1.6 GB peak memory. This is the top priority before anything long-running (games) or Android use is realistic.
* A list filtered from a mixed list is still a mixed list (no type narrowing yet).
* `for` over a range ending at the maximum `Int` never terminates.
* Generic (unannotated) functions are only type-checked when they are called.
