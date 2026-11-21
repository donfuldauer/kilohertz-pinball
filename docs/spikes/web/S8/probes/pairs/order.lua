-- S8 deliverable 3: pairs order over address-keyed tables, across ticks,
-- with garbage collection freeing and recreating key objects in between
-- (addresses are reused; creation ids are not).
S8_OUT = {}
local function out(s) S8_OUT[#S8_OUT + 1] = s end
local function order(t) local o = {} for _, v in pairs(t) do o[#o + 1] = v end return table.concat(o, ',') end
local live = {}
n = 0
function on_tick()
  n = n + 1
  local kt, ft, ut, mixed = {}, {}, {}, {}
  for i = 1, 48 do local k = {} kt[k] = i live[(n * 48 + i) % 96 + 1] = k end
  for i = 1, 24 do ft[function() return i end] = i end
  for i = 1, 12 do local _, e = pcall(pinball.pulse_coil, 'bad', i) ut[e] = i end
  local cf = {string.format, string.rep, string.sub, table.insert, table.concat, math.floor, math.abs, print, pairs, ipairs, type, tostring}
  for i, f in ipairs(cf) do mixed[f] = 'c' .. i end
  for i = 1, 16 do mixed[{}] = 't' .. i mixed['s' .. i] = 's' .. i mixed[i * 1.5] = 'f' .. i end
  out('ORDER tick ' .. n .. ' tables ' .. order(kt))
  out('ORDER tick ' .. n .. ' closures ' .. order(ft))
  out('ORDER tick ' .. n .. ' userdata ' .. order(ut))
  out('ORDER tick ' .. n .. ' mixed ' .. order(mixed))
  -- deletion during traversal and rehash on an address-keyed table
  local del = {}
  for k, v in pairs(kt) do if v % 2 == 0 then kt[k] = nil end del[#del + 1] = v end
  for i = 49, 120 do kt[{}] = i end
  out('ORDER tick ' .. n .. ' after_delete_rehash ' .. order(kt) .. ' visited ' .. table.concat(del, ','))
  -- garbage: frees this tick's keys after the next collections
  for i = 1, 3000 do local g = {i} end
end
