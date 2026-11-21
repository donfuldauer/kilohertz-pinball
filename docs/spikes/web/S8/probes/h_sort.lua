-- (h) table.sort: invalid order functions, large arrays (pivot choice),
-- errors and resulting permutations.
local function arr(n, seed) local t, x = {}, seed for i = 1, n do x = (x * 1103515245 + 12345) % 2147483648 t[i] = x % 1000 end return t end
function on_tick()
  for _, n in ipairs({5, 50, 99, 100, 101, 150, 300, 1000}) do
    local t = arr(n, n)
    local ok, e = pcall(table.sort, t, function(a, b) return true end)
    out('always_true', n, tostring(ok), tostring(e), table.concat(t, ',', 1, math.min(n, 30)))
    t = arr(n, n + 1)
    ok, e = pcall(table.sort, t, function(a, b) return a <= b end)
    out('le', n, tostring(ok), tostring(e), table.concat(t, ',', 1, math.min(n, 30)))
    t = arr(n, n + 2)
    local c = 0
    ok, e = pcall(table.sort, t, function(a, b) c = c + 1 return (c % 3 == 0) end)
    out('erratic', n, tostring(ok), tostring(e), c, table.concat(t, ',', 1, math.min(n, 30)))
    t = arr(n, n + 3)
    table.sort(t)
    out('ok', n, table.concat(t, ',', 1, math.min(n, 30)))
    t = arr(n, n + 4)
    table.sort(t, function(a, b) return a > b end)
    out('desc', n, table.concat(t, ',', 1, math.min(n, 30)))
  end
  out('mixed_types', try(table.sort, {1, 'a', 2}))
  out('nan', try(function() local t = {3, 0/0, 1, 0/0, 2} table.sort(t) return t[1], t[5] end))
  out('too_big', try(table.sort, setmetatable({}, {__len = function() return math.maxinteger end})))
end
