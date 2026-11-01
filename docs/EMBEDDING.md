# Embedding SuperNova in a host program (Kotlin, C, ...)

SuperNova compiles to native code, so a host app uses a script by loading it as a shared
library. The script declares what it needs from the host (`extern fn`) and what the host may
call (`export fn`); `snc build --lib` then produces `libname.so` plus `libname.h`.

```
snc build game.sn --lib -o libgame.so      # also writes libgame.h
```

## The script side

```
extern fn hostLog(msg: String)             # the host provides this
extern fn hostTime() -> Float

var score = 0                              # script state lives in top-level variables

export fn start(name: String):             # the host calls these
	hostLog(f"hello {name} at {hostTime()}")

export fn addScore(n: Int) -> Int:
	score += n
	return score

print("loaded")                            # top-level code runs inside sn_init()
```

Only `Int` (`int64_t`), `Float` (`double`), `Bool` (`int32_t`), `String` (UTF-8 `const char *`)
and no value at all may cross the boundary, as parameters or results. Enums, lists, objects and
lambdas stay inside the script (expose them through exported functions).

## The C interface (in `libgame.h`)

| Function | Meaning |
| --- | --- |
| `int sn_init(void)` | Run the script's top-level code once. Returns 1 on success. Call it first, after registering externs. |
| `sn_<name>(...)` | One per `export fn`. Returns 0 / NULL if the script raised an error. |
| `const char *sn_last_error(void)` | Description (with `file:line:col`) of the last failed call, or "" if it succeeded. |
| `void sn_set_extern_<name>(void *fn)` | Register the host's implementation of `extern fn <name>`. |

A string result is valid until the next call into the library on the same thread, so copy it
if you need to keep it. An error raised in the script (bad index, `throw`, `x!` on null, ...)
does not crash the host: the call returns 0 and `sn_last_error()` says why. Calling an extern
the host did not register is such an error too.

## From Kotlin with JNA

(This is the intended usage; the library side is tested with a C host, the Kotlin snippet is
not run in the test suite.)

```kotlin
import com.sun.jna.*

interface GameLib : Library {
    fun sn_init(): Int
    fun sn_last_error(): String
    fun sn_start(name: String)
    fun sn_addScore(n: Long): Long
    fun sn_set_extern_hostLog(cb: Callback)
    fun sn_set_extern_hostTime(cb: Callback)
}
interface LogCallback : Callback { fun invoke(msg: String) }
interface TimeCallback : Callback { fun invoke(): Double }

val lib = Native.load("game", GameLib::class.java)
// keep these in fields: JNA frees a callback that is garbage collected
val log = object : LogCallback { override fun invoke(msg: String) { Log.d("script", msg) } }
val time = object : TimeCallback { override fun invoke() = System.nanoTime() / 1e9 }
lib.sn_set_extern_hostLog(log)
lib.sn_set_extern_hostTime(time)
check(lib.sn_init() == 1) { lib.sn_last_error() }
lib.sn_start("hero")
val total = lib.sn_addScore(5)
```

On Android put `libgame.so` in `app/src/main/jniLibs/arm64-v8a/`. Build it for the right target
with the NDK's clang, for example
`CC=aarch64-linux-android24-clang snc build game.sn --lib -o libgame.so` (`snc` uses `$CC`
when it is set). `snc` itself runs on your development machine or in Termux; the app only
needs the finished `.so`. Plain JNI works the same way: declare the functions from the header
in a small C file and call them from your `external fun`s.

## Rules and limits

* Calls are serialised: one thread runs script code at a time (others wait). Calls from a host
  callback back into the library on the same thread are fine.
* The script's memory is managed by its own garbage collector; nothing needs freeing by the host.
* `async`/`await` is not available in library builds yet (compile error).
* Fatal conditions (out of memory, stack overflow) still end the process; ordinary script
  errors do not.
* `print` writes to stdout, which Android does not show; route logging through an `extern fn`.
* Scripts are compiled ahead of time, so changing a script means rebuilding the `.so`
  (there is no on-device hot reload).
