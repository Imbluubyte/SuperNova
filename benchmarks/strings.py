total = 0
for i in range(1, 2000001):
    s = f"item {i} of {i * 2}"
    t = s + "!"
    total += int(str(i)) % 3
print(total)
