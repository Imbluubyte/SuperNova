local xs = {}
for i = 1, 5000000 do xs[i] = i % 1000 end
local total = 0
for _ = 1, 20 do
  local ys = {}
  for i = 1, #xs do ys[i] = xs[i] * 2 + 1 end
  local s = 0
  for i = 1, #ys do s = s + ys[i] end
  total = total + s
end
local n = 0
for i = 1, #xs do if xs[i] > 500 then n = n + 1 end end
print(string.format("%d %d", total, n))
