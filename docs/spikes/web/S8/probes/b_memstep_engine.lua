-- (b) GC schedule trace: a mixed workload (tables, closures, strings, error
-- objects from the host API) whose accounted heap is printed after every
-- callback (memtrace mode). Identical traces = identical GC schedule.
objs = {} n = 0
function on_tick()
  n = n + 1
  local t = {n, tostring(n), function() return n end, {x = n}}
  objs[n % 64 + 1] = t
  for i = 1, 20 do local s = ('s' .. i) .. n end
  local ok, e = pcall(pinball.pulse_coil, 'bad', 1)
  objs[(n * 7) % 64 + 1] = e
  if n % 50 == 0 then for i = 1, 64 do objs[i] = nil end end
end
