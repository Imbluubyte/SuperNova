# Building and running SuperNova on Android (Termux)

SuperNova's compiler is written in Rust with no dependencies and emits C, which it
compiles with `clang`. Both are available as Termux packages, so everything
(compiling the compiler, compiling your programs, running them) happens on the device.

## 1. Install Termux

Install Termux from **F-Droid** or the **GitHub releases page** of the Termux project,
not from an old Google Play build (those are outdated and can no longer install packages).
Work inside Termux's own home directory (`~`). Do not build or run programs from
`/sdcard` or other shared storage: Android does not allow executing files there.

## 2. Quick install

```
pkg update && pkg upgrade
pkg install git
git clone https://github.com/Imbluubyte/SuperNova
cd SuperNova
./scripts/termux-install.sh
```

The script installs `rust` and `clang`, builds the compiler (a few minutes the first
time), copies `snc` to your PATH and runs `snc doctor`.

## 3. Manual install

```
pkg install rust clang git
git clone https://github.com/Imbluubyte/SuperNova
cd SuperNova/compiler
cargo build --release
cp target/release/snc $PREFIX/bin/
snc doctor
```

## 4. Check that it works

```
snc doctor
```

compiles and runs real programs on your device: normally, with the garbage collector
forced to run on every allocation, a gigabyte of throw-away garbage, and infinite
recursion. It ends with `All checks passed.` or lists what failed. **If anything fails,
copy the whole output** (it includes your CPU architecture, clang version and timings).

## 5. Use it

```
cat > hello.sn <<'EOT'
var name = "Android"
print(f"Hello from {name}!")
print([1, 2, 3, 4, 5].filter(> 2) * 10)
EOT

snc run hello.sn                 # compile and run
snc build hello.sn -o hello      # produce a native executable
./hello
snc emit-c hello.sn              # look at the generated C
```

Runtime errors name the source line: `RuntimeError: list index 4 is out of range ... at hello.sn:3:7`.

## 6. Measure performance on your device

```
cd benchmarks
./run.sh                 # SuperNova vs Python (and Lua if installed: pkg install lua54)
SN_GC_STATS=1 ./some_program   # GC pause and heap statistics for any compiled program
```

`pkg install python lua54` lets the script compare against them.

## Troubleshooting

| Symptom | Fix |
|---|---|
| `no C compiler found` | `pkg install clang` |
| `cannot create a temporary directory` | `export TMPDIR=$PREFIX/tmp; mkdir -p $TMPDIR` |
| `Permission denied` when running a built program | Build in `~`, not `/sdcard` |
| Build runs out of memory or Termux is killed | Close other apps; `cargo build --release -j1`; or edit `compiler/Cargo.toml` and set `lto = false` |
| `snc doctor` fails only on the GC checks | Please report; include the full output |
| Want a different C compiler | `CC=gcc snc run hello.sn` (needs a single program name, no flags) |

## What is verified

The compiler and runtime are tested on x86-64 Linux with clang and gcc at several
optimisation levels. The ARM64/Android path uses only standard POSIX calls plus
`__builtin_unwind_init`, which was checked to generate the expected register spills for
`aarch64-linux-android`, but the full test suite has not yet run on a physical device:
`snc doctor` is how that gets verified.
