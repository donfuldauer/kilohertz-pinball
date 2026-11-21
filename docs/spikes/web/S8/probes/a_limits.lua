-- (a) struct-size-dependent growth and limits: parser limits, C-stack limit,
-- string caps, error texts carrying counts. Big budget, 16 MiB heap.
function on_tick()
  -- parser limits are reached at load time of the *probe*; test through the
  -- error texts of the libraries that recurse instead (load() is absent).
  local mt = {} mt.__index = function(t, k) return t[k] end
  out('index_recursion', try(function() local t = setmetatable({}, mt) return t.x end))
  out('tostring_recursion', try(function() local t t = setmetatable({}, {__tostring = function(s) return tostring(s) end}) return tostring(t) end))
  out('gsub_recursion', try(function() local function g(s) return (string.gsub(s, '.', g)) end return g('abc') end))
  local d = 0
  local function nest(n) d = n local ok = pcall(nest, n + 1) return ok end
  out('pcall_nest', try(nest, 1), 'depth', d)
  local function cmp(a, b) table.sort({3, 2, 1}, cmp) return a < b end
  out('sort_recursion', try(table.sort, {3, 2, 1}, cmp))
  out('concat_meta', try(function() local m = {} m.__concat = function(a, b) return a .. b end local a = setmetatable({}, m) return a .. 'x' end))
  out('rep_too_large', try(string.rep, 'x', 1 << 40))
  out('rep_too_large2', try(string.rep, 'ab', 0x7fffffff, 'c'))
  out('rep_cap', try(string.rep, 'x', 65537))
  out('rep_ok', #select(2, pcall(string.rep, 'x', 65536)))
  out('concat_cap', try(function() local s = string.rep('x', 40000) return s .. s end))
  local parts = {} for i = 1, 200 do parts[i] = string.rep('y', 300) end
  out('table_concat_cap', try(table.concat, parts))
  out('unpack_too_many', try(table.unpack, {}, 1, 1 << 31))
  out('unpack_neg', try(table.unpack, {}, -(1 << 31), (1 << 31)))
  out('unpack_big', try(function() return select('#', table.unpack({}, 1, 200000)) end))
  out('select_neg', try(select, -1))
  out('char_range', try(string.char, 256))
  out('byte_slice', try(string.byte, string.rep('a', 70), 1, -1) ~= nil)
  out('format_width', try(string.format, '%100d', 1))
  out('format_prec', try(string.format, '%.100f', 1))
  out('format_99', #select(2, pcall(string.format, '%99.99f', 1e300)))
  out('captures', try(string.match, string.rep('a', 40), string.rep('(a)', 33)))
  out('pattern_depth', try(string.match, string.rep('a', 300), string.rep('a?', 300) .. string.rep('a', 300)))
  out('too_complex', try(string.find, string.rep('a', 20000), '.-.-.-b'))
  out('tostring_int_limits', math.maxinteger, math.mininteger, -math.mininteger, math.mininteger // -1)
  out('string_len_limits', #string.rep('z', 65536))
end
