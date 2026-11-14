local s = string.rep("a", 30000)
function on_tick() string.find(s, ".-.-.-.-b") end
