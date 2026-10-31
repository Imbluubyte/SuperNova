# SuperNova language reference (implemented subset)

Everything here is implemented and covered by tests in `tests/`. Syntax that
parses but is not yet implemented (dictionaries, try/catch, async,
modules, first-class functions) is rejected with a "not implemented yet"
compile error rather than silently misbehaving.

## Basics
* Indentation-based blocks (4 spaces; tabs are rejected), `#` comments, no semicolons.
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
