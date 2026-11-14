local s = string.rep("x", 65536)
function on_save_state() local t = {} for i = 1, 20000 do t[i] = s end return t end
function on_tick() end
