local total = 0
for i = 1, 2000000 do
  local s = "item " .. i .. " of " .. (i * 2)
  local t = s .. "!"
  total = total + math.tointeger(tonumber(tostring(i))) % 3
end
print(total)
