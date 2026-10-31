xs = [i % 1000 for i in range(1, 5000001)]
total = 0
for _ in range(20):
    ys = [x * 2 + 1 for x in xs]
    total += sum(ys)
print(total, len([x for x in xs if x > 500]))
