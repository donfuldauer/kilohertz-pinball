keep = {} n = 0 phase = 0
local P = string.rep("x", 1000) local T = {} for i = 1, 64 do T[i] = P end
function on_tick()
  if phase == 0 then
    for i = 1, 3000 do n = n + 1 keep[n] = {} end
    if collectgarbage("count") > 1975 then phase = 1 end
  else
    while true do local t = {} for i = 1, 40 do t[i] = {} end end
  end
end
