# Using SuperNova as Nebula's scripting language

Nebula (Kotlin, node-based, 2D first) embeds SuperNova scripts the way Godot embeds GDScript:
each node may have a script, written as a class that extends a node class, with `onReady` and
`onUpdate(dt)` methods the engine calls. This document describes how the pieces fit; the
mechanics are in [EMBEDDING.md](EMBEDDING.md) and a concrete starting point lives in
`examples/nebula/`.

## How it fits together

```
 game scripts (.sn)  ──snc build --lib──►  libscripts.so + libscripts.h
        │  use nebula                                  ▲
        ▼                                              │ JNA / JNI
 nebula.sn  (API module, shipped with the engine)      │
   extern fn nb_set_position(id, x, y) ... ◄───────────┘  Kotlin implements the externs
```

1. **`nebula.sn` is the scripting API.** It declares the engine functions as `extern fn`
   (node ids, positions, input, sound, ...) and wraps them in friendly base classes
   (`Node`, `Node2D`, `Signal`). It is ordinary SuperNova, so adding an engine feature means
   adding an `extern fn` on the Kotlin side and a wrapper in `nebula.sn`.
   `examples/nebula/nebula.sn` is a proposal, not a fixed contract.
2. **Game scripts extend those classes.** `export class Player extends nebula.Node2D` makes the
   class visible to the host.
3. **The engine keeps one integer handle per scripted node**: it creates the node, calls
   `sn_new_Player(nodeId)`, stores the handle, and later calls `sn_Player_onReady(h)`,
   `sn_Player_onUpdate(h, dt)` each frame, and `sn_release(h)` when the node is freed.
   Missing methods simply do not exist in the library, so the engine can look them up once
   (`dlsym` / JNA `getFunction`) and skip the ones a script did not define.
4. **Per frame**: call `onUpdate` for scripted nodes, then `sn_pump()` so `async` code
   (timers, `await sleep(...)`) advances.

## Build

```
snc build main.sn --lib -I path/to/nebula-api -o libscripts.so
```

`-I` (or the `SN_PATH` environment variable) adds the folder holding `nebula.sn`.
On Android build with the NDK compiler: `CC=aarch64-linux-android24-clang snc build ...`.
A Nebula editor could run this command when a script is saved and reload the library (a new
`libscripts.so` can be loaded in place of the old one).

## Language features that matter for an engine

* classes with single inheritance, `onCreation`, `super`, `private`
* lambdas and function values: `signal.connect(fn() => ...)`
* enums for states and input actions
* `T?` optionals for "maybe there is a node here"
* `async`/`await` for timers and sequences
* try/catch: a script error never crashes the engine (the call returns 0 and
  `sn_last_error()` describes it with `file:line:col`)

## What is not there yet

* Only Int, Float, Bool and String cross the boundary, so vectors are passed as two Floats and
  node references as ids. Methods taking objects (`addChild(child: Node)`) stay script-only.
* Property editing in the editor (exported variables with types and ranges) is a design
  question for the editor: the header already lists public fields with their types, which
  could drive it.
* Scripts are native code: a reload means building and loading a new `.so`; there is no
  interpreter mode.
* One thread runs script code at a time.
