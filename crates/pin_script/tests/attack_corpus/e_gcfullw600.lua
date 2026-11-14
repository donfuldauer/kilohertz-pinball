keep = {} n = 0 phase = 0
local P = string.rep("x", 1000)
function on_tick()
  if phase == 0 then
    for i = 1, 3000 do n = n + 1 keep[n] = {i} end
    if collectgarbage("count") > 600 then phase = 1 end
  else
    local rep = string.rep while true do local s = rep(P, 65) end
  end
end
