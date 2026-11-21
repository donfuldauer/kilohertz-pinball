-- S8 probe helpers (prepended to every probe by run_audit.sh).
S8_OUT = {}
local _out = S8_OUT
function out(...) local t = {...} for i = 1, select('#', ...) do t[i] = tostring(t[i]) end _out[#_out + 1] = table.concat(t, ' ') end
-- Describe a value without addresses: floats exact (%a), strings quoted.
function ser(v)
  local ty = type(v)
  if ty == 'number' then
    if math.type(v) == 'integer' then return 'i:' .. string.format('%d', v) end
    if v ~= v then return 'f:nan(' .. tostring(v) .. ')' end
    return 'f:' .. string.format('%a', v) .. '=' .. tostring(v)
  elseif ty == 'string' then return 's:' .. string.format('%q', v)
  else return ty .. ':' .. tostring(v) end
end
-- pcall a function; result line "ok <values>" or "err <message>".
function try(f, ...)
  local r = table.pack(pcall(f, ...))
  if r[1] then
    local t = {}
    for i = 2, r.n do t[#t + 1] = ser(r[i]) end
    return 'ok ' .. table.concat(t, ' ')
  else
    return 'err ' .. tostring(r[2])
  end
end
function on_tick()
  local d = 0
  local function nest(k) d = k if k >= 25 then return true end local ok, r = pcall(nest, k + 1) return ok and r end
  out('pcall_nest_target', 25, 'result', try(nest, 1), 'depth', d)
end
