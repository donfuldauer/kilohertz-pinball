function on_tick() table.remove(setmetatable({}, {__len = function() return math.maxinteger end}), 1) end
