function on_tick()
  local x <close> = setmetatable({}, {__close = function() while true do end end})
  error("plain")
end
