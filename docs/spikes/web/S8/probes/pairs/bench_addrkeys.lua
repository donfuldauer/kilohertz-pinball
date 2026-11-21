-- KHZ 34 micro-benchmark: 2 000 000 lookups/stores with table and closure keys.
S8_OUT = {}
function on_tick()
  local keys, t = {}, {}
  for i = 1, 1000 do local k = (i % 2 == 0) and {} or function() return i end keys[i] = k t[k] = i end
  local s = 0
  for r = 1, 1000 do for i = 1, 1000 do s = s + t[keys[i]] end end
  for r = 1, 1000 do for i = 1, 1000, 7 do t[keys[i]] = r end end
  S8_OUT[1] = tostring(s)
end
