function on_tick() table.insert(setmetatable({}, {__len = function() return math.maxinteger - 1 end}), 1, 1) end
