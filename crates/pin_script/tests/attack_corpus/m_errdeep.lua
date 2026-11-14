local function d(n) if n == 0 then pinball.pulse_coil() end return d(n - 1) + 1 end function on_tick() while true do pcall(d, 150) end end
