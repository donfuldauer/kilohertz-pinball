-- (g) next/pairs order for every key type, deletion during traversal,
-- rehash. Address-keyed sections are marked ADDR (pre-existing D9 issue).
local function order(t) local o = {} for k, v in pairs(t) do o[#o + 1] = tostring(v) end return table.concat(o, ',') end
function on_tick()
  local t = {}
  for i = 1, 30 do t[i * 7919 % 1000] = i end
  out('ORDER int', order(t))
  t = {}
  for i = 1, 30 do t[i + 0.5] = i t[-i * 1e10 - 0.25] = -i end
  t[1/0] = 'inf' t[-1/0] = '-inf' t[2^53] = 'big' t[-0.0] = 'negzero'
  out('ORDER float', order(t))
  t = {}
  for i = 1, 40 do t['k' .. i] = i t[string.rep('L', 41) .. i] = 'L' .. i end
  out('ORDER string', order(t))
  t = {[true] = 'T', [false] = 'F', [1] = 1, x = 'x', [2.5] = 2.5}
  out('ORDER bool_mixed', order(t))
  -- deletion during traversal (allowed: assign nil to existing fields)
  t = {}
  for i = 1, 50 do t['d' .. i] = i end
  local seen = {}
  for k, v in pairs(t) do seen[#seen + 1] = v if v % 3 == 0 then t[k] = nil end end
  out('ORDER delete_during', table.concat(seen, ','), order(t))
  -- rehash: grow after deletions
  for i = 51, 120 do t['d' .. i] = i end
  out('ORDER after_rehash', order(t))
  -- integer keys moving between array and hash parts
  t = {}
  for i = 1, 20 do t[i] = i end
  for i = 1, 20, 2 do t[i] = nil end
  for i = 21, 40 do t[i * 3] = i end
  out('ORDER int_mixed', order(t))
  -- next() with an explicit key
  out('next', try(next, {}, 'nokey'), try(next, {a = 1}, 'a'))
  -- ADDR: tables, Lua closures, C functions and host userdata as keys
  local keys, kt = {}, {}
  for i = 1, 40 do local k = {} keys[i] = k kt[k] = i end
  out('ORDER ADDR tables', order(kt))
  local ft = {}
  for i = 1, 40 do ft[function() return i end] = i end
  out('ORDER ADDR closures', order(ft))
  local ct = {}
  local cf = {string.format, string.rep, string.sub, table.insert, table.concat, math.floor, math.abs, print, pairs, ipairs, type, tostring, tonumber, select, rawget, rawequal, next, error, pcall, utf8.char}
  for i, f in ipairs(cf) do ct[f] = i end
  out('ORDER ADDR cfunctions', order(ct))
  local ut = {}
  for i = 1, 20 do local _, e = pcall(pinball.pulse_coil, 'bad', i) ut[e] = i end
  out('ORDER ADDR userdata', order(ut))
  -- ADDR after garbage: free the keys, make garbage, create new ones
  keys, kt = nil, nil
  for i = 1, 2000 do local g = {i, tostring(i)} end
  local kt2 = {}
  for i = 1, 40 do kt2[{}] = i end
  out('ORDER ADDR tables_after_gc', order(kt2))
end
