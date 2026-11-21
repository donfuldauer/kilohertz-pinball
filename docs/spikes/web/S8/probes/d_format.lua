-- (d) string.format: every conversion, flags, widths, precisions, limits.
local ints = {0, 1, -1, 7, 255, 256, 65535, -65536, 2^31 // 1, -(2^31 // 1), 4294967295, 4294967296, -4294967297, math.maxinteger, math.mininteger, 1 << 32, 1 << 53, 123456789012345}
local floats = {0.0, -0.0, 1.0, -1.0, 0.1, 1/3, 2.5, 0.5, 1e15, 1e16, 1e100, 1e-5, 1e-300, 2^-1074, 2^-1022, 1.7976931348623157e308, 1/0, -1/0, 2^53, 2^63, -2^63, 123.456, 9.999999999999999e22}
local convs = {'%d', '%i', '%5d', '%-5d|', '%05d', '%+d', '% d', '%x', '%X', '%#x', '%o', '%#o', '%c', '%10.3d', '%.0d'}
local fconvs = {'%e', '%E', '%.0e', '%.20e', '%f', '%.0f', '%.3f', '%10.2f', '%-10.2f|', '%+f', '% f', '%#.0f', '%g', '%G', '%.0g', '%.17g', '%#g', '%.14g', '%a', '%A', '%.3a', '%.0a', '%20.10e', '%099.3f', '%.99f'}
function on_tick()
  for _, c in ipairs(convs) do
    local row = {}
    for _, v in ipairs(ints) do
      local okv, r = pcall(string.format, c, (c == '%c') and (v % 256) or v)
      row[#row + 1] = okv and r or ('E:' .. r)
    end
    out('int', c, table.concat(row, ','))
  end
  for _, c in ipairs(fconvs) do
    local row = {}
    for _, v in ipairs(floats) do
      local okv, r = pcall(string.format, c, v)
      row[#row + 1] = okv and r or ('E:' .. r)
    end
    out('flt', c, table.concat(row, ','))
  end
  for _, v in ipairs(floats) do out('q', try(string.format, '%q', v), tostring(v), v .. '') end
  for _, v in ipairs(ints) do out('qi', try(string.format, '%q', v)) end
  out('qs', try(string.format, '%q', 'a\0b\r\n\t"\\\127\200\255' .. string.char(1, 2, 31)))
  out('s', try(string.format, '%5.2s|%-6s|%.0s|%s', 'abcdef', 'xy', 'zz', 'plain'))
  out('s_embedded0', try(string.format, '%s', 'a\0b'), try(string.format, '%10s', 'a\0b'))
  for _, bad in ipairs({'%u', '%b', '%5', '%100d', '%.100f', '%1000s', '%-+ #0d', '%#q', '%10q', '%ll', '%n', '%p', '%', '%%'}) do
    out('bad', bad, try(string.format, bad, 1))
  end
  out('d_float', try(string.format, '%d', 3.0), try(string.format, '%d', 3.5), try(string.format, '%d', 2^63))
  out('x_float', try(string.format, '%x', -1.0))
  out('c_range', try(string.format, '%c', 300), try(string.format, '%c', -1))
  out('many_args', try(string.format, '%d %d', 1))
  out('nan', tostring(0/0), tostring(-(0/0)), string.format('%f %g %e %a %.3f %10.2f|%-10.2f|', 0/0, 0/0, 0/0, 0/0, -(0/0), 0/0, -(0/0)))
  out('nan_q', try(string.format, '%q', 0/0), try(string.format, '%q', -(0/0)))
  out('nan_concat', (0/0) .. '', math.huge - math.huge .. '', (math.abs(0/0)) .. '')
end
