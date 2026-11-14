keep = {} n = 0 phase = 0
function on_tick()
  if phase == 0 then
    for i = 1, 6000 do n = n + 1 keep[n] = {i} end
    if collectgarbage("count") > 15900 then phase = 1 end
  else
    while true do pcall(function() local s = {1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32} end) end
  end
end
