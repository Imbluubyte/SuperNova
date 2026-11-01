# SuperNova language reference (implemented subset)

Everything here is implemented and covered by tests in `tests/`. Syntax that
parses but is not yet implemented (optional types) is rejected with a "not implemented yet"
compile error rather than silently misbehaving.

## Basics
* Indentation-based blocks (use one tab per level; spaces also work, but a file must not mix the two), `#` comments, no semicolons.
* `var` declares a mutable variable, `const` an immutable one. Both require an initialiser.
* Types: `Int` (64-bit), `Float` (64-bit), `Bool`, `String`, `null`.
  Types are inferred; annotations are optional: `var x: Float = 5`.
* Implicit conversion: `Int` to `Float` only. `Float` to `Int` needs `int(x)`.
* Top-level statements run in order as the program entry point.

## Operators
| Kind | Operators | Notes |
|---|---|---|
| Arithmetic | `+ - * / // % **` | `/` always yields `Float`; `//` and `%` use floor semantics (`-7 // 2 == -4`, `-7 % 3 == 2`). `Int ** Int` is `Int` (negative exponent is a runtime error). |
| Comparison | `== != < <= > >=` | No chaining. Strings compare bytewise (UTF-8). |
| Logic | `and or not` | Operands must be `Bool` (no truthiness). Short-circuit. |
| Assignment | `= += -= *= /= %=` | `x /= 2` on an `Int` variable is an error; use `//=`-style `x = x // 2`. |

* `Int` `+ - *` wrap on overflow (defined behaviour, not UB).
* Division or modulo by zero is a runtime error (also for `Float`).
* String `+` concatenates. f-strings: `f"Hello {name}!"`; `{{` and `}}` are literal braces.

## Control flow
```
if a:
	...
orif b:
	...
else:
	...

while cond:
	...

for i in 1..10:      # inclusive: 1,2,...,10
	...
for i in 1..<10:     # exclusive end: 1,2,...,9
	...

match value:
	case "x":
		...
	default:
		...
```
* Conditions must be `Bool`. `break` and `continue` work in `while` and `for`.
* `for` currently iterates ranges only. The loop variable is a constant.
* `match` cases must be literal values (Int, Float, String, Bool) of the subject's type.

## Functions
```
fn add(a, b):                 # unannotated parameters are generic
	return a + b

fn greet(who: String, punct: String = "!") -> String:
	return "Hi " + who + punct

greet("Ana", punct="?")       # named arguments
```
* Declared at module level only (nested functions are an error in V1); may be used before their declaration.
* Unannotated parameters are specialised per call: `add(1, 2)` and `add(1.5, 2)` compile to two typed native functions. Mixed Int/Float arguments to one call just follow the usual numeric rules inside the body.
* Return types are inferred; a recursive function needs `-> Type`. All `return` statements must agree (Int is not silently merged with Float).
* A function with a non-nothing return type must return on every path.
* Default arguments are evaluated at the call site in a context that cannot see caller locals. Arguments are evaluated left to right, in the order written.
* Parameters are mutable locals.
* Functions that are fully annotated are type-checked even if never called; generic ones are checked when first called.

## Lists
```
var numbers = [10, 20, 30]
print(numbers[1], numbers[3])      # 1-based: 10 30
numbers[2] = 25
var empty: List[Int] = []          # empty lists need an annotation
for n in numbers:
	print(n)
```
* Indexing is **1-based**. Index 0, indexes past the end and negative indexes are runtime errors that name the index and the length. There is no negative indexing.
* The element type is fixed at compile time. `[1, 2, 3]` is a `List[Int]` stored as a contiguous `int64` array; `[1, 2.5]` is a `List[Float]` (Int widens); a list mixing Int/Float/Bool/String/null is a mixed list (`Any` elements, 16-byte tagged values). `add` of a value that does not fit is a compile error, so representation never changes behind your back.
* Values taken from a mixed list can only be compared with `==`/`!=`; filter by type first (`items.filter(int)`). *(The filtered result is still a mixed list in V1; type narrowing is on the roadmap.)*
* Lists have **reference semantics**: `var b = a` makes `b` another name for the same list. Use `a.copy()` for an independent list.
* Printing shows strings quoted inside lists (`["Hi", "Hello"]`) and bare at top level.

**Elementwise arithmetic** (`+ - * / // % **` and unary `-`) works list-with-number, number-with-list and list-with-list, producing a new list. `Int` lists stay `Int` except `/`, which gives `Float`. Lists of **different lengths are a runtime error** (no broadcasting, no silent truncation).

**Methods:** `length()`, `add(x)`, `remove(x)` (first match, returns whether anything was removed), `contains(x)`, `merge(other)`, `copy()`, `clear()`, `reverse()`, `sort()` (numbers or strings), `sum()`, `min()`, `max()`, `mean()` (`mean` always returns `Float`; `min`/`max`/`mean` of an empty list are runtime errors), `filter(...)`.

**filter** returns a *new* list and never modifies the original:
```
items.filter(str)            # by type: str, int, float, bool, null
numbers.filter(< 4)          # comparison predicate: < <= > >= == !=
numbers.filter(isEven)       # function taking the element, returning Bool
numbers.filter(greaterThan, 9)   # extra arguments follow the function
```
A comparison predicate such as `< 4` is a dedicated syntactic form, valid only as a call argument. It compiles to an inlined comparison in the loop (no function call, no lambda).

## Tuples
```
fn getPosition():
	return 10, 20
var position = getPosition()
print(position[1], position[2])     # 1-based
```
* Tuples are immutable, fixed-size and may hold different types; they are plain structs passed by value (no allocation).
* The index must be an integer **literal** because each element can have a different type, so the compiler needs to know which one you mean. Out-of-range literal indexes are compile errors.
* Type syntax: `Tuple[Int, String]`. Multiple return values are just tuples; you are never forced to unpack them.

## Builtins
`print(a, b, ...)` (space separated, newline terminated), `str(x)`, `int(x)`, `float(x)`.
`int("12")`/`float("1.5")` parse strings and raise a runtime error on bad input.
`gcCollect()` runs the garbage collector now; `gcHeapBytes()` returns the bytes currently
allocated (see [MEMORY.md](MEMORY.md)). Memory is otherwise fully automatic.

## Runtime errors
Printed as `RuntimeError: <message>` with `at file.sn:line:col`, exit code 1.

## Classes

```
class Entity:
	var hp = 10
	private var secret = 1
	fn onCreation(hp: Int = 10):
		self.hp = hp
	fn describe():
		return f"entity {hp}"

class Goblin extends Entity:
	fn describe():                      # override
		return "goblin " + super.describe()

var g = Goblin(5)
print(g.describe())
```

- Single inheritance; fields and methods are inherited; `private` members are only visible inside the declaring class.
- `onCreation` is the constructor. A parent `onCreation` that takes no required arguments runs automatically first; otherwise call `super.onCreation(...)` explicitly.
- Bare field/method names work inside methods; `self.x` disambiguates. `super.m()` calls the parent version.
- Overrides must keep parameter count, visibility and return type. Calls dispatch on the real object type.
- Objects compare by identity (`==`, `!=`); a subclass can be used wherever its parent is expected; `filter(ClassName)` selects instances of a class from a list.
- `toString()` (no params, returns String) customises `print` and f-strings.
- Objects are garbage collected. Classes must be declared at module level.

## Dictionaries

```
var ages = {"ada": 36, "bob": 41}
ages["cy"] = 29              # insert or overwrite
ages["bob"] += 1             # the key must already exist
print(ages["ada"])           # a missing key is a runtime error
var counts: Dict[String, Int] = {}     # empty dictionaries need an annotation
for name in ages:            # iterates keys, in insertion order
	print(name, ages[name])
```

- Keys are `Int`, `String` or `Bool`; values can be any type (mixed values become `Any`, like mixed lists).
- Insertion-ordered; reference semantics (like lists); printed as `{"a": 1}`.
- Methods: `length()`, `keys()`, `values()`, `contains(key)`, `get(key, default)`, `remove(key)` (no-op if absent), `merge(other)`, `clear()`.
- Iterating takes a snapshot of the keys, so the loop body may add or remove entries.
- Dictionaries cannot be compared with `==`.

## Modules

```
# utils/mathx.sn
const PI = 3.14159
private var calls = 0          # not visible to importers
fn square(x):
	calls += 1
	return x * x

# main.sn
use utils.mathx                # loads utils/mathx.sn; namespace is `mathx`
use game.shapes as sh          # `as` picks another namespace
print(mathx.square(3), mathx.PI)
var c = sh.Circle(1.0)         # classes, functions and variables are all reachable
var s: sh.Shape = c            # ...including in type annotations and `extends sh.Shape`
```

- `use a.b` loads `a/b.sn` relative to the folder of the file you compile, then each folder in `$SN_PATH` (colon separated).
- A file's top-level code runs once, before the code of any file that imports it. Files are loaded once, however often they are imported.
- Everything is reached through the namespace (`mathx.square`); there is no `from ... import`. Variables of another module can be read and assigned (`mathx.counter = 0`).
- `private` on a top-level `var`, `const`, `fn` or `class` hides it from importing files (compile error). Class-level `private` is separate and works as described under Classes.
- Circular imports, unknown modules/members and a module name that clashes with a top-level name are compile errors. Errors point at the right file and line.
- A local variable or parameter named like a module shadows it.
- `use` must be at the top level of a file.

## Errors: try / catch / finally / throw

```
fn withdraw(balance: Int, n: Int) -> Int:
	if n > balance:
		throw f"insufficient funds: {balance} < {n}"
	return balance - n

try:
	print(withdraw(10, 40))
catch e:                       # e is a String
	print("failed:", e)
finally:
	print("always runs")       # also on return, break and continue
```

- `throw` takes a `String` message (use `str(x)` for other values). Runtime errors (bad index, missing dictionary key, division by zero, failed `int("abc")`, ...) are caught the same way; `e` is the message without the source location.
- `try` needs a `catch`, a `finally`, or both. Errors thrown inside `catch` or `finally` propagate outward; `finally` runs first.
- An uncaught error prints `Error: message` (or `RuntimeError: ...`) with its location and exits with status 1.
- Not catchable: out of memory and stack overflow.
- Implementation: `setjmp`/`longjmp` handler frames, so a `try` that never throws costs a few instructions. Locals assigned inside a `try` are declared `volatile` so their values are correct after an error.

## Async / await

```
async fn fetchSquare(x: Int) -> Int:
	await sleep(0.1)                   # does not block other tasks
	return x * x

async fn main2():
	var a = fetchSquare(3)             # starts a task, returns a Task[Int]
	var b = fetchSquare(4)             # both run concurrently
	print(await a + await b)           # 25 after ~0.1 s, not 0.2 s

await main2()                          # `await` is allowed at the top level of a file
var text = await readFile("notes.txt")     # file I/O runs in the background
```

- Calling an `async fn` returns immediately with a `Task[T]` (`T` = the declared or inferred return type; `Task[Void]`-style tasks give no value). The body runs when the program waits: at an `await`, or before the program exits.
- `await task` suspends only the current task and yields its value. `await` works inside `async fn` bodies and at the top level of a file, but not in ordinary functions.
- Errors thrown in a task are rethrown by `await`, so `try`/`catch` around `await` works. An error in a task that is never awaited is reported when the program ends and makes it exit with status 1.
- Built-in awaitables: `sleep(seconds)`, `readFile(path)` (-> String), `writeFile(path, text)`. A failed file operation throws a message such as `cannot read 'x': No such file or directory`.
- Methods may be `async` (not `onCreation`). Tasks are collected like any object; `Task[T]` can be used as a type, e.g. `List[Task[Int]]`.
- Only one task runs at a time, so tasks never race on your data. A task that loops forever without awaiting blocks all others.
- Each task has a 256 KiB stack (override with env `SN_TASK_STACK_KB`), so very deep recursion inside a task reports "stack overflow" sooner than on the main stack.
- Supported on x86-64 and ARM64.

## Strings

Strings are immutable byte strings: `length` and indexes count bytes (so they are
exact for ASCII), `upper`/`lower` change ASCII letters only. Indexes start at 1.

```
var s = "  Hello, World  "
var t = s.trim()                 # "Hello, World"
print(t.length(), t.upper(), t[1], t.slice(1, 5))     # 12 HELLO, WORLD H Hello
print(t.contains("World"), t.startsWith("He"), t.endsWith("x"))
print(t.find("o"), t.find("o", 6), t.find("zzz"))      # 5 9 0   (0 = not found)
print(t.replace("l", "L"), "ab".repeat(3))
print("a,b,c".split(","), "abc".chars())               # lists of Strings
print(["x", "y"].join("-"), [1, 2, 3].join(", "))      # join works on lists of anything
```

`slice(from, to)` is inclusive like a range; `s[i]` is a one-character String. Out of range
indexes are runtime errors (catchable).

## Functions as values

```
fn double(x: Int) -> Int:
	return x * 2

fn apply(f: Fn(Int) -> Int, x: Int) -> Int:
	return f(x)

var f: Fn(Int) -> Int = double
print(f(4), apply(double, 5))
var ops: List[Fn(Int) -> Int] = [double, double]
print([1, 2, 3].map(double), [1, 2, 3].reduce(0, add))
```

- The type is written `Fn(ParamTypes) -> ReturnType` (leave off `-> ...` for none).
- A function name becomes a value wherever a function type is expected: variables, parameters, return values, list/dictionary elements, object fields. A function with untyped parameters needs that context (or annotated parameters) so the compiler knows which version to use.
- A value is a plain function pointer, so calling it costs the same as a direct call. `f(x)`, `table["add"](1, 2)` and `obj.callback(x)` all work. Async functions can be values too (`Fn(Int) -> Task[Int]`).
- There are no lambdas in V1: only names of top-level functions.
- List methods taking functions: `map(f)`, `each(f)`, `reduce(start, f)`, `any(f)`, `all(f)`; `filter(f)` also accepts a function variable.

## Optional types and null safety

`T?` holds either a `T` or `null`. A plain `T` is never null, and an optional cannot be
used as a value until the program has dealt with the null case:

```
var next: Node? = null
var name: String? = findName()
print(name ?? "anonymous")      # fallback when null
print(name!)                    # assert non-null (runtime error if null)
if name != null:
	print(name.length())        # name reads as a plain String here
```

* `x == null` / `x != null` test for null. After `if x != null:` (also `and` chains,
  `while x != null:`, and `if x == null: return` / `throw` / `break` / `continue` for the rest
  of the block) the variable reads as its plain type. Narrowing applies to local variables,
  parameters and top-level variables; it is dropped when the variable is assigned or, for
  top-level variables, when any non-builtin function is called. Fields (`self.next`) are not
  narrowed: copy them into a local first (`var n = self.next`).
* `a ?? b` gives `a` if it is not null, otherwise `b`. `x!` unwraps or raises a catchable runtime error.
* `T` and `null` convert to `T?` implicitly; `T?` never converts to `T` implicitly.
* `dict.get(key)` (one argument) returns `V?`; `dict.get(key, default)` returns `V`.
* Optionals print as the value or `null`.

## `has` and `repeat`

`a has b` is true when a list contains an item, a dictionary a key, or a string a substring
(same as `.contains(b)`). `not (xs has 3)` negates it.

`repeat(n):` runs its body `n` times (an Int expression, evaluated once). `break` and `continue` work.

## Lambdas

`fn(params) => expression` makes an anonymous function:

```
var xs = [1, 2, 3]
print(xs.map(fn(x) => x * 2))
var offset = 10
var shift = fn(x: Int) => x + offset      # captures offset
fn makeAdder(n: Int) -> Fn(Int) -> Int:
	return fn(x) => x + n
```

Parameter types come from the place the lambda is used (`map`, a parameter or variable of
type `Fn(...) -> ...`) or from annotations. The body is a single expression; use a named
function when you need statements. A lambda copies the local variables it uses at the moment
it is created (lists, dictionaries and objects are shared, so changes to those are visible
both ways); top-level variables are used directly. Inside a method a lambda can use fields,
methods and `self`.

## Enums

```
enum Direction:
	Up
	Down
	Left = 10
	Right          # 11

var d = Direction.Up
if d == Direction.Up:
	print(d)                 # Direction.Up
print(d.name, d.value)       # Up 0
match d:
	case Direction.Up:
		print("up")
	default:
		print("other")
print(Direction.values())    # every variant, in order
```

An enum is a separate type: it cannot be mixed with Int or another enum, and only `==` and
`!=` apply. Variants count up from 0 (or from the last explicit value; values must be unique
integer literals). `.name` is the variant's name and `.value` its Int. Enums are declared at the
top level of a file, can be `private`, and are used from other modules as `mod.Direction.Up`.

## Host interface

`extern fn name(params) -> T` (no body) declares a function provided by a host program, and
`export fn` marks a function the host may call; see [EMBEDDING.md](EMBEDDING.md).

## Standard library

Modules shipped inside the compiler; import them with `use std.<name>`:

| Module | Contents |
| --- | --- |
| `std.math` | `PI E TAU`, `sqrt pow exp log log2 log10 sin cos tan asin acos atan atan2 hypot`, `floor ceil round trunc` (return Int), `abs min max clamp sign` (Int or Float), `gcd lerp toRadians toDegrees isNaN` |
| `std.random` | `seed random randInt randFloat chance choice shuffle` |
| `std.time` | `now monotonic since measure` |
| `std.sys` | `args exit getEnv hasEnv write readLine input` |
| `std.fs` | `readText writeText appendText exists readLines` |

```
use std.math
use std.random
print(math.sqrt(2.0), math.max(3, 9), random.randInt(1, 6))
```

The sources are in `stdlib/*.sn` and are ordinary SuperNova modules built on a small set of
`__name` compiler intrinsics (not for use in programs). `readText`/`writeText` block; use the
built-in `await readFile(...)` / `await writeFile(...)` for background I/O. `math.min(a, b)`
needs both arguments to have the same type.

## Error messages

Compile errors show `file:line:col`, the source line and a caret. Misspelled variables, functions,
fields and methods (including string and dictionary methods) get a `did you mean 'x'?` hint.
