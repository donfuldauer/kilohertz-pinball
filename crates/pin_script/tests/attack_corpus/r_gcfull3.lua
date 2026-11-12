keep = {} n = 0 phase = 0
function on_tick()
  if phase == 0 then
    for i = 1, 6000 do n = n + 1 keep[n] = {i} end
    if collectgarbage("count") > 15200 then phase = 1 end
  else
    for i = 1, 300 do local s = {i, i, i, i, i, i, i, i, i, i, i, i, i, i, i, i, i, i, i, i, i, i, i, i, i, i, i, i, i, i, i, i} end
  end
end
