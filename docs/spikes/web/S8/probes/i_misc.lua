-- (i) sandbox environment scan, positions >= 2^32 in the string library,
-- utf8, size checks, string comparison (strcoll), coroutines/load/dump.
local seen = {}
local function scan(name, t, depth)
  if seen[t] or depth > 3 then return end
  seen[t] = true
  local keys = {}
  for k in pairs(t) do keys[#keys + 1] = tostring(k) end
  table.sort(keys)
  for _, k in ipairs(keys) do
    local v = rawget(t, k) if v == nil then v = t[k] end
    local ty = type(v)
    out('ENV', name .. '.' .. k, ty)
    if ty == 'table' then scan(name .. '.' .. k, v, depth + 1) end
    if ty == 'userdata' or ty == 'thread' then out('ENV_UNEXPECTED', name .. '.' .. k, ty) end
  end
end
function on_tick()
  scan('_G', _G, 0)
  out('ENV string_mt', type(getmetatable('')), tostring(getmetatable('')))
  out('absent', type(coroutine), type(load), type(loadstring), type(dofile), type(string.dump), type(string.pack), type(string.unpack), type(io), type(os), type(debug), type(require), type(collectgarbage), type(newproxy))
  local s = 'hello'
  local P = {4294967296, 4294967297, 4294967298, 4294967301, 8589934593, (1 << 40) + 2, math.maxinteger, -4294967296, -4294967297, -4294967291, math.mininteger}
  for _, p in ipairs(P) do
    out('pos', p, try(string.sub, s, p), try(string.sub, s, 1, p), try(string.sub, s, p, -1), try(string.byte, s, p), try(string.byte, s, 1, p),
        try(string.find, s, 'l', p), try(string.find, s, '', p), try(string.find, s, 'l', p, true), try(string.match, s, '.', p),
        try(function() local r = {} for w in string.gmatch(s, '.', p) do r[#r + 1] = w end return table.concat(r) end))
    out('utf8pos', p, try(utf8.len, s, p), try(utf8.len, s, 1, p), try(utf8.codepoint, s, p), try(utf8.codepoint, s, 1, p), try(utf8.offset, s, 1, p), try(utf8.offset, s, p), try(utf8.offset, s, -1, p))
  end
  out('utf8char', try(utf8.char, 0x7FFFFFFF), try(utf8.char, 0x80000000), try(utf8.char, -1), try(utf8.char, 0x10FFFF, 0x110000))
  local seqs = {'\u{7FFFFFFF}', '\xF4\x90\x80\x80', '\xFD\xBF\xBF\xBF\xBF\xBF', '\xFE\x80', '\xFF', '\xC0\x80', '\xED\xA0\x80', '\xF8\x88\x80\x80\x80', '\u{0000000041}'}
  for i, q in ipairs(seqs) do
    out('utf8dec', i, try(utf8.len, q), try(utf8.len, q, 1, -1, true), try(utf8.codepoint, q, 1, -1, true), try(utf8.codepoint, q),
        try(function() local r = {} for p, c in utf8.codes(q, true) do r[#r + 1] = p .. ':' .. c end return table.concat(r, ' ') end))
  end
  out('rep_sizes', try(string.rep, 'x', 2^31), try(string.rep, 'x', 4294967297), try(string.rep, '', 1 << 20), try(string.rep, 'ab', 3, string.rep('s', 70000)))
  out('concat_sizes', try(table.concat, {'a', 'b'}, string.rep('s', 70000)))
  out('lower_upper', string.lower('ÀÉ\200\255AZaz'), string.upper('àé\200\255azAZ'))
  -- string comparison over all byte pairs (l_strcmp -> strcoll)
  local h = 0
  for a = 0, 255, 3 do for b = 0, 255, 5 do
    local x, y = string.char(a, 65), string.char(b)
    h = (h * 31 + ((x < y) and 1 or 0) + ((x <= y) and 2 or 0)) % 1000000007
  end end
  out('strcmp_digest', h, tostring('a\0b' < 'a\0c'), tostring('\200' < '\100'), tostring('' < '\0'))
  out('random', try(math.random, 0), try(math.random, 1, 2, 3), try(math.random, 3, 1), try(math.randomseed, 1))
end
