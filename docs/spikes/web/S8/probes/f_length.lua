-- (f) '#' with holes after different construction/resize histories, and the
-- table library boundaries.
local function hist(build)
  local t = build()
  return #t
end
function on_tick()
  local r = {}
  r[#r + 1] = hist(function() return {1, 2, nil, 4} end)
  r[#r + 1] = hist(function() return {1, 2, nil, 4, nil} end)
  r[#r + 1] = hist(function() return {nil, nil, 3} end)
  r[#r + 1] = hist(function() local t = {} for i = 1, 10 do t[i] = i end t[5] = nil return t end)
  r[#r + 1] = hist(function() local t = {} for i = 1, 10 do t[i] = i end t[10] = nil t[9] = nil return t end)
  r[#r + 1] = hist(function() local t = {} for i = 10, 1, -1 do t[i] = i end t[3] = nil return t end)
  r[#r + 1] = hist(function() local t = {} for i = 1, 100 do t[i * 2] = i end return t end)
  r[#r + 1] = hist(function() local t = {} for i = 1, 64 do t[i] = i end for i = 1, 64, 3 do t[i] = nil end return t end)
  r[#r + 1] = hist(function() local t = {n = 1} for i = 1, 17 do t[i] = i end t[17] = nil t[16] = nil t[1] = nil return t end)
  r[#r + 1] = hist(function() local t = {} t[1] = 1 t[2] = 2 t[4] = 4 t[3] = 3 t[8] = 8 t[3] = nil return t end)
  out('len', table.concat(r, ','))
  -- grow/shrink patterns, then length after each step
  local t, lens = {}, {}
  for i = 1, 300 do t[i] = i if i % 7 == 0 then t[i - 3] = nil end lens[#lens + 1] = #t end
  for i = 300, 1, -2 do t[i] = nil lens[#lens + 1] = #t end
  out('lens', table.concat(lens, ','))
  out('insert', try(table.insert, {1, 2, 3}, 5, 'x'), try(table.insert, {1, 2, 3}, 0, 'x'), try(table.insert, {1, 2, 3}, 4, 'x'), try(table.insert, {}, 1, 2, 3))
  out('remove', try(table.remove, {}), try(table.remove, {}, 0), try(table.remove, {1}, 2), try(table.remove, {1}, 3), try(table.remove, {1, 2, 3}, -1))
  out('unpack', try(table.unpack, {1, 2, 3}, -1, 1), try(table.unpack, {1, 2, 3}, math.maxinteger - 1, math.maxinteger), try(table.unpack, {1, 2, 3}, math.mininteger, math.mininteger + 1))
  out('concat', try(table.concat, {1, 2.5, 'x'}, '-'), try(table.concat, {1, {}, 3}), try(table.concat, {1, 2, 3}, ',', 2, 5), try(table.concat, {}, ',', math.maxinteger, math.maxinteger))
  out('move', try(table.move, {1, 2, 3}, 1, 3, 2), try(table.move, {1, 2, 3}, -1, math.maxinteger, 1), try(table.move, {1, 2, 3}, 1, 3, math.maxinteger))
  out('pack', try(function() local p = table.pack(1, nil, 3) return p.n, #p end))
end
