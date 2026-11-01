# SuperNova cheat sheet

Everything here compiles today (the examples are extracted and tested). Files end in `.sn`.
Run with `snc run file.sn`. Not implemented yet: see the last section.

## Basics
```sn
# comment (no semicolons; 4-space indentation, no tabs)
var health = 100            # mutable, type inferred: Int
const max_health = 100      # immutable
var speed: Float = 5        # annotation; Int widens to Float
var name = "Player"
var alive = true
print("hp:", health, speed, name, alive)   # space separated, then newline
```
Types: `Int` (64-bit, wraps on overflow), `Float`, `Bool`, `String`, `null`.
Names: variables `snake_case`, functions `camelCase`. Conversions: `str(x)`, `int(x)`, `float(x)`.

## Operators
```sn
var a = 7
print(a + 2, a - 2, a * 2)     # + - *
print(a / 2, a // 2, a % 3)    # 3.5  3  1   ('/' is always Float; '//' floors)
print(-7 // 2, -7 % 3, 2 ** 10)  # -4  2  1024
print(a > 3 and not (a == 5) or false)    # and  or  not  (operands must be Bool)
a += 1
a -= 1
a *= 2
print(a, "ab" + "cd", "abc" < "abd")
```
No truthiness: `if 1:` is an error, write `if x != 0:`. No chained comparisons (`1 < x < 5`).
`x /= 2` on an `Int` variable is an error; use `x = x // 2`.

## Strings
```sn
var who = "world"
print(f"Hello {who}! 2+2={2 + 2}")    # f-string: any expression inside { }
print(f"braces: {{ }} tab:\there")    # {{ }} literal braces; \n \t \" \\ escapes
print(str(42) + "!", int("12") + 1, float("1.5"))
```
No string indexing, slicing or length yet. No quotes inside `{ }` of an f-string.

## Control flow
```sn
var hp = 40
if hp > 50:
	print("Healthy")
orif hp > 0:                 # 'orif', not 'elif'
	print("Injured")
else:
	print("Dead")

var n = 0
while n < 3:
	n += 1

for i in 1..5:               # inclusive: 1 2 3 4 5
	if i == 2:
		continue
	if i == 4:
		break
	print(i)
for i in 1..<5:              # exclusive end: 1 2 3 4
	print(i)

var kind = "Dragon"
match kind:                  # cases are literals (Int, Float, String, Bool)
	case "Goblin":
		print("small")
	case "Dragon":
		print("big")
	default:
		print("?")
```
Ranges count up only (`5..1` runs zero times) and have no step. A one-line block works: `if x > 1: print(x)`.

## Functions
```sn
fn add(a, b):                          # untyped params: generic, specialised per call
	return a + b

fn greet(who: String, punct: String = "!") -> String:   # types, default, return type
	return "Hi " + who + punct

fn fact(n: Int) -> Int:                # recursion needs -> ReturnType
	if n <= 1:
		return 1
	return n * fact(n - 1)

fn getPosition():                      # several values = a tuple
	return 10, 20

print(add(1, 2), add(1.5, 2), greet("Ana"), greet("Bo", punct="?"), fact(10))
var pos = getPosition()
print(pos[1], pos[2], pos)             # tuple indexes start at 1 and must be literals
```
Functions live at module level (no nested `fn`), can be used before they are declared, and
every path of a function that returns a value must return. Functions are not values yet,
except by name as a `filter` argument.

## Lists (1-based, no negative indexes)
```sn
var nums = [10, 20, 30]
print(nums[1], nums[3], nums.length())     # 10 30 3
nums[2] = 25
nums[1] += 5
var empty: List[Int] = []                  # empty lists need a type
empty.add(7)
var mixed = [1, "two", 3.5, true, null]    # mixed types allowed
print(nums, mixed)
for x in nums:
	print(x)
```
Methods: `length() add(x) remove(x) contains(x) merge(other) copy() clear() reverse() sort()
sum() min() max() mean() filter(...)`. `remove` and `contains` return `Bool`.
Lists are **references**: `var b = a` shares the list; use `a.copy()` for a separate one.
Items taken from a mixed list can only be compared with `==` and `!=`.

### Elementwise arithmetic
```sn
var p = [1, 2, 3]
var q = [4, 5, 6]
print(p * 2, 10 - p, p + q, p * q, -p, p / 2)   # [2,4,6] [9,8,7] [5,7,9] [4,10,18] ...
# lists of different lengths are a runtime error
```

### filter (returns a new list)
```sn
fn isEven(number):
	return number % 2 == 0

fn greaterThan(number, minimum):
	return number > minimum

var data = [1, 2, 3, 4, 5, 10, 15]
var stuff = [1, 2, "Hi", "Hello", 3.5]
print(data.filter(< 4), data.filter(>= 5), data.filter(== 2))   # < <= > >= == !=
print(data.filter(isEven), data.filter(greaterThan, 9))         # function (+ extra args)
print(stuff.filter(str), stuff.filter(int), stuff.filter(float))  # by type
```

## Memory
```sn
gcCollect()                  # run the garbage collector now
print(gcHeapBytes() > 0)     # bytes currently allocated
```
Memory is automatic. Env vars when running a program: `SN_GC_STATS=1` (pause/heap report),
`SN_GC_STRESS=1` (collect on every allocation; slow, used for testing).

## Commands
```
snc run file.sn            compile and run
snc build file.sn -o out   native executable
snc check file.sn          type-check only
snc emit-c file.sn         show the generated C
snc doctor                 self-test this installation
```

## Errors you will see
Compile errors show `file:line:col`, the source line and a caret. Runtime errors print
`RuntimeError: ... at file.sn:line:col` and exit with code 1 (index out of range, division by
zero, empty `min()`, mismatched list lengths, bad `int("x")`, stack overflow, out of memory).

## Not implemented yet
`for` over anything but ranges, lists and dictionaries. These parse but give a clear
"not implemented yet" error. Lambdas and enums are deliberately not in V1.
