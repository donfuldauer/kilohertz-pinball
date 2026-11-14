function on_tick()
  local ok1 = pcall(setmetatable, "", {})
  local mt = getmetatable("")
  string.upper = function() return "pwned" end
  error(tostring(ok1) .. " " .. tostring(mt) .. " " .. ("x"):upper())
end
