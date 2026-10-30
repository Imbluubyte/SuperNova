# SuperNova language reference (implemented subset)

Everything here is implemented and covered by tests in `tests/`. Syntax that
parses but is not yet implemented (classes, lists, dictionaries, tuples,
try/catch, async, modules, first-class functions) is rejected with a
"not implemented yet" compile error rather than silently misbehaving.

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

## Builtins
`print(a, b, ...)` (space separated, newline terminated), `str(x)`, `int(x)`, `float(x)`.
`int("12")`/`float("1.5")` parse strings and raise a runtime error on bad input.

## Runtime errors
Printed as `RuntimeError: <message>` with `at file.sn:line:col`, exit code 1.
