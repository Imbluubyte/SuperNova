# Memory management

SuperNova memory is fully automatic. Strings, lists and (later) class instances live
in a garbage-collected heap; numbers, booleans and tuples are plain values with no
heap cost. You never allocate or free anything.

## The collector

A **non-moving, stop-the-world mark-and-sweep** collector (`runtime/sngc.h`, ~500 lines of C).

| Decision | Choice | Why |
|---|---|---|
| Roots | C stack and registers scanned **conservatively**, plus registered globals | The compiler emits C; precise stack maps or a shadow stack would add cost to every call and local. Conservative scanning needs no code in generated functions and no write barriers. |
| Heap objects | Scanned word by word, **except** "atomic" objects | Strings and `List[Int]` / `List[Float]` / `List[Bool]` storage can never hold pointers, so they are never scanned. A 5-million-element `Int` list costs nothing to mark. |
| Moving | No | Pointers stay valid for C interop (Android NDK, Nebula) and conservative roots cannot be updated anyway. |
| Interior pointers | Supported | A pointer into the middle of an object keeps it alive, so the C compiler's pointer arithmetic is safe. |
| Allocator | Size-class spans (40 classes, 16 B to 32 KiB, 256 KiB spans) with free lists and one state byte per object; larger objects are individual blocks | Cheap allocation, fast address-to-object lookup, little fragmentation. Empty spans are returned to the OS (one is kept per class). |
| Trigger | After `max(4 MiB, live bytes after the previous collection)` bytes have been allocated | Heap stays at roughly 2x live data; small programs rarely collect at all. |
| List growth | The old storage is freed immediately when a list outgrows it | Doubling does not leave garbage behind. |

Rejected: reference counting (cycles leak; atomic updates are costly on ARM), a copying
collector (moves objects, needs precise roots), generational collection (needs write
barriers in generated code; revisit when measurements show a need).

### Measured behaviour

Release build, x86-64 Linux (run `benchmarks/run.sh` and `snc doctor` to measure your device):

| Workload | Result |
|---|---|
| About 1 GB of short-lived lists (`tests/cases/slow_gc_churn.sn`) | 13 MB peak RSS, 266 collections, worst pause 0.02 ms |
| 20 rounds of `xs * 2 + 1` on a 5M-element list (`benchmarks/lists.sn`) | 270 MB peak RSS (was 1.6 GB without a collector), 16 collections, worst pause 0.01 ms |

Pauses are short because most data in these workloads is atomic. A heap with many
millions of small pointer-containing objects will pause longer (the whole live set
is traced on each collection). Incremental or generational collection is the planned
answer if that matters for games; see ROADMAP.md.

## Controlling the collector

```
gcCollect()       # run a collection now (e.g. while a level loads)
gcHeapBytes()     # bytes currently allocated, including garbage not yet collected;
                  # call gcCollect() first to see the live size
```

Environment variables (for debugging and tuning, no recompile needed):

* `SN_GC_STATS=1` prints collections, total and worst pause, and peak heap at exit.
* `SN_GC_STRESS=N` collects before every Nth allocation. `SN_GC_STRESS=1` is a brutal
  correctness test: any value the collector cannot see turns into corrupted output.
  The test suite runs every program this way.

## Other safety nets

* **Stack overflow**: unbounded recursion ends with `RuntimeError: stack overflow` and
  exit code 1 instead of a crash.
* **Out of memory**: when the OS refuses memory the collector runs once and retries,
  then reports `RuntimeError: out of memory` with the requested size.
* **Absurd sizes** (over 64 TiB) are rejected before reaching the allocator.
* Any other invalid memory access prints `FatalError ... a bug in SuperNova` rather
  than a bare segmentation fault.

## Rules and limits

* The collector is **single-threaded**: only the main thread may touch heap objects.
  Async tasks all run on the main thread (one at a time); file operations run on worker
  threads that never touch the heap and hand results back to the main thread. Each
  task has its own stack: suspended task stacks are scanned as roots, from their saved
  stack pointer to the top (see `runtime/snasync.h`).
* Conservative scanning can occasionally keep an object alive because an unrelated
  integer looks like its address (false retention). It never frees live data. Atomic
  storage is immune, so big numeric arrays are unaffected.
* POSIX only for now (Linux, Android/Bionic); Windows needs a port of the stack-bounds
  and signal code.
* Temporaries are not yet eliminated: `xs * 2 + 1` allocates two lists. The collector
  makes this safe; fusing such expressions is a planned optimisation.
