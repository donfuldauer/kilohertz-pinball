n = 0
function on_tick()
  local x <close> = setmetatable({}, {__close = function() for i = 1, 3e8 do n = n + 1 end end})
  while true do end
end
