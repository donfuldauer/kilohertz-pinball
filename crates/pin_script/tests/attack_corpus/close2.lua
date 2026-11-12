function on_tick()
  local x <close> = setmetatable({}, {__close = function() while true do pcall(error, "x") end end})
  error("boom")
end
