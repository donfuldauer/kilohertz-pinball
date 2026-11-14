keep = {} n = 0 phase = 0
function on_tick()
  if phase == 0 then
    for i = 1, 6000 do n = n + 1 keep[n] = {i} end
    if collectgarbage("count") > 14600 then phase = 1 end
  else
    for i = 1, 60 do local s = string.rep("x", 65000) end
  end
end
