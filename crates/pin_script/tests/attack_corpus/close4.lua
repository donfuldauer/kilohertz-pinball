function on_tick()
  pcall(function()
    local x <close> = setmetatable({}, {__close = function() while true do end end})
    while true do end
  end)
end
