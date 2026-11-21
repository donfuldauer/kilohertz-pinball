-- (e) integer/float conversions, arithmetic edges, string<->number.
local function F(x) return ser(x) end
function on_tick()
  local mi, ma = math.mininteger, math.maxinteger
  out('idiv', F(7 // 2), F(-7 // 2), F(7 // -2), F(mi // -1), F(7.0 // 2), F(-7.5 // 2), F(1 // 0.0), F(-1 // 0.0), F(0.0 // 0.0), F(-0.0 // 1))
  out('mod', F(-7 % 3), F(7 % -3), F(mi % -1), F(5.5 % -2), F(-5.5 % 2), F(1 % math.huge), F(-1 % math.huge), F(math.huge % 2), F(5 % -0.0))
  out('idiv0', try(function() return 1 // 0 end), try(function() return 1 % 0 end))
  out('overflow', F(ma + 1), F(mi - 1), F(ma * 2), F(mi * -1), F(-mi), F(math.abs(mi)))
  out('shifts', F(1 << 63), F(1 << 64), F(1 << -1), F(-1 >> 1), F(-1 >> 63), F(-1 >> 64), F(1 << 65), F(3 >> -2), F(~0), F(5 & 3.0), try(function() return 1 << 1.5 end))
  out('tointeger', F(math.tointeger(3.0)), F(math.tointeger(3.5)), F(math.tointeger(2^63)), F(math.tointeger(-2^63)), F(math.tointeger('8')), F(math.tointeger(2^53)))
  out('floorceil', F(math.floor(2^62)), F(math.floor(2^63)), F(math.floor(-2^63)), F(math.ceil(-0.5)), F(math.floor(-0.0)), F(math.floor(1e308)), F(math.ceil(2^63 - 1024)))
  out('ult', tostring(math.ult(1, -1)), tostring(math.ult(-1, 1)), tostring(math.ult(mi, ma)))
  out('fmod', try(math.fmod, 7, 3), try(math.fmod, -7, 3), try(math.fmod, mi, -1), try(math.fmod, 7, 0), try(math.fmod, 7.5, 2), try(math.fmod, -7.5, 0), try(math.fmod, 1, math.huge))
  out('abs', F(math.abs(-0.0)), F(math.abs(mi)), F(math.max(1, 2.0)), F(math.min(-0.0, 0.0)), F(math.max(0.0, -0.0)))
  out('pow', F(2^63), F((-8)^(1/3)), F(0^0), F(math.huge^0), F(2^-1080), F(10^308 * 10))
  out('sqrt', F(math.sqrt(2)), F(math.sqrt(-1) ~= math.sqrt(-1) and 1 or 0), F(math.exp(1)), F(math.log(10)), F(math.log(8, 2)), F(math.log(100, 10)), F(math.sin(1e22)), F(math.atan(1, -1)), F(math.pi))
  local strs = {'10', '0x10', '1e2', '  5  ', '5.', '.5', '0x1p4', '0x.8', '1e', '0x', '9223372036854775807', '9223372036854775808', '-9223372036854775808', '-9223372036854775809', '0x7fffffffffffffff', '0xffffffffffffffff', '0x10000000000000000', '1e308', '1e309', '-1e309', '2.2250738585072011e-308', '4.9e-324', '2e-324', '0x1.fffffffffffffp1023', '0x1p-1074', '0x1p-1075', 'inf', 'nan', '1_0', '١', '0b101', '\t0x1A\n', '1' .. string.rep('0', 30), '0.' .. string.rep('0', 400) .. '1'}
  for _, s in ipairs(strs) do out('tonumber', string.format('%q', s), F(tonumber(s)), try(function() return s + 0 end), F(math.tointeger(s))) end
  out('tonumber_base', F(tonumber('ff', 16)), F(tonumber('zz', 36)), F(tonumber('7fffffffffffffff', 16)), F(tonumber('ffffffffffffffff', 16)), F(tonumber('10000000000000000', 16)), F(tonumber('-10', 2)), try(tonumber, '1', 37))
  -- numeric for loops with float limits and steps
  local function loop(a, b, c) local n, last = 0, nil for i = a, b, c do n = n + 1 last = i if n > 10 then break end end return n .. ':' .. F(last) end
  out('for', loop(1, 3.5, 1), loop(1, 2^63, 2^62), loop(ma - 2, ma, 1), loop(mi + 2, mi, -1), loop(1.0, 2, 0.5), loop(1, 0, -1.5), loop(0.1, 0.35, 0.1), loop(ma, 1e100, 1), loop(1, -math.huge, -1))
  out('for0', try(function() for i = 1, 2, 0 do end end), try(function() for i = 1.0, 2, 0.0 do end end))
  out('eq', tostring(1 == 1.0), tostring(2^53 == (1 << 53)), tostring((1 << 53) + 1 == 2^53 + 1), tostring(ma + 0.0 == ma), tostring(mi + 0.0 == mi), tostring(math.huge > ma), tostring(-0.0 == 0))
  out('lt', tostring(1 < 1.5), tostring(ma < 2^63), tostring(ma + 0.0 < ma), tostring(mi <= -2^63), tostring((1 << 62) < 4.611686018427388e18))
  out('tostr', tostring(1e15), tostring(1e16), tostring(-0.0), tostring(2^63), tostring(1/3), tostring(100 // 1.0), tostring(3 | 0))
  out('str_arith', try(function() return '10' + 1 end), try(function() return '0x10' * '2' end), try(function() return '1e1' // 1 end), try(function() return ' 3 ' - 1 end), try(function() return '3.0' | 0 end), try(function() return '3.5' | 0 end))
end
