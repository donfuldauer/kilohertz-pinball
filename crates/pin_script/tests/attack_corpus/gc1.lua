function on_tick()
  local mt = setmetatable({}, {__index = function(t, k) if k == "__gc" then return function() while true do end end end end})
  for i = 1, 100 do setmetatable({}, mt) end
  -- proxy mt whose rawget is nil but real field appears: use __gc = false placeholder trick
  local ok, e = pcall(setmetatable, {}, {__gc = false})
  local mt2 = {__gc = nil}
  pinball.pulse_coil(1,1)
end
