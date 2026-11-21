-- (c) addresses: tostring / %s / %p / concatenation of every reachable type.
function on_tick()
  local ok, err = pcall(pinball.pulse_coil, 'x', 1)
  local vals = {{}, print, on_tick, string.format, function() end, err, pinball, pinball.sw, setmetatable({}, {__name = 'N'})}
  for i, v in ipairs(vals) do
    out('tostring', i, type(v), tostring(v), string.format('%s', v), try(string.format, '%p', v))
    out('concat', i, try(function() return 'x' .. v end))
  end
  out('err_type', type(err), tostring(err))
  out('print', try(print, {}, 1))
  out('tostring_nil', tostring(nil), tostring(true), tostring(12), tostring(1.5))
  out('index_err', try(function() local t = nil return t.x end))
  out('call_err', try(function() local t = {} return t() end))
  out('arith_err', try(function() return {} + 1 end))
  out('compare_err', try(function() return {} < {} end))
end
