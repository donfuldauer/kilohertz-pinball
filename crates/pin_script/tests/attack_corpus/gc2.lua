-- 5.4: finalizer marked only if __gc present at setmetatable time; try true placeholder then swap
function on_tick()
  local ok, e = pcall(setmetatable, {}, {__gc = true})
  if ok then error("placeholder accepted") end
  -- metatable passed through pinball proxy / string?
  local mt = {}
  local o = setmetatable({}, mt)
  mt.__gc = function() while true do end end
  setmetatable(o, mt) -- re-set with __gc now present -> wrapper should refuse
end
