function on_tick()
  error(setmetatable({}, {__tostring = function() while true do end end, __index = function() while true do end end, __name = "x"}))
end
