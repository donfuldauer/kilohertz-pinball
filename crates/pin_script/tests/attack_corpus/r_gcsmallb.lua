local P = string.rep("x", 1000)
-- ordinary allocation-heavy script with a small live heap (control)
function on_tick() for i = 1, 60 do local s = string.rep(P, 65) end end
