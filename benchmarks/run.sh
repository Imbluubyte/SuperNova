#!/usr/bin/env bash
# Times each benchmark in SuperNova and, where installed, Python and Lua.
# Usage: benchmarks/run.sh [name ...]     (default: all)
# Needs `snc` on PATH, or a release build at compiler/target/release/snc.
cd "$(dirname "$0")" || exit 1
SNC=$(command -v snc || echo ../compiler/target/release/snc)
[ -x "$SNC" ] || { echo "snc not found; build it first (cargo build --release in compiler/)"; exit 1; }
names=("$@"); [ ${#names[@]} -eq 0 ] && names=(fib loop lists strings)
secs() { local TIMEFORMAT=%R; { time "$@" >/dev/null 2>&1; } 2>&1; }
printf "%-10s %12s %12s %12s\n" benchmark "SuperNova" "Python" "Lua"
for n in "${names[@]}"; do
  tmp=$(mktemp -d); "$SNC" build "$n.sn" -o "$tmp/$n" || { echo "$n: compile failed"; continue; }
  sn=$(secs "$tmp/$n"); rm -rf "$tmp"
  py="-"; command -v python3 >/dev/null && [ -f "$n.py" ] && py=$(secs python3 "$n.py")
  lu="-"; for l in luajit lua; do command -v $l >/dev/null && [ -f "$n.lua" ] && { lu=$(secs $l "$n.lua"); break; }; done
  printf "%-10s %12s %12s %12s\n" "$n" "${sn}s" "${py/%[0-9]/&s}" "${lu/%[0-9]/&s}"
done
echo "(compile time excluded; times are wall-clock seconds, lower is better)"
