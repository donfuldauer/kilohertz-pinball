-- KHZ 36 probe: NaN text (sign of generated NaNs is CPU dependent).
S8_OUT = {}
function on_tick()
  local z = 0
  local a, b, c = z / z, math.huge - math.huge, math.sqrt(-1)
  S8_OUT[1] = 'tostring ' .. tostring(a) .. ' ' .. tostring(-a) .. ' ' .. tostring(b) .. ' ' .. tostring(c) .. ' ' .. tostring(math.log(-1)) .. ' ' .. tostring(math.acos(2))
  S8_OUT[2] = 'format ' .. string.format('%f|%g|%e|%a|%5.1f|%-6g|%+f', a, a, b, c, a, -a, a)
  S8_OUT[3] = 'concat ' .. (a .. '') .. ' ' .. (math.fmod(1, 0.0) .. '') .. ' ' .. string.format('%q', a)
  pinball.set_score(1, #tostring(a))
end
