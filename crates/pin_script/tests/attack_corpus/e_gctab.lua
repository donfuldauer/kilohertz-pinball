keep = {} n = 0 phase = 0
function on_tick()
  if phase == 0 then
    for i = 1, 3000 do n = n + 1 keep[n] = {i} end
    if collectgarbage("count") > 1500 then phase = 1 end
  else
    for i = 1, 4000 do local s = {i, i, i, i} end
  end
end
