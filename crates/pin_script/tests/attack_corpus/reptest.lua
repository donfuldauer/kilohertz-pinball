local function ref(s, n, sep) local t = {} for i = 1, n do t[i] = s end return table.concat(t, sep or "") end
function on_tick()
  local cases = {{"x", 1}, {"x", 2}, {"x", 3}, {"ab", 7}, {"abc", 1000}, {"", 5, ","}, {"ab", 1, "--"}, {"ab", 2, "--"}, {"ab", 3, "--"}, {"ab", 17, "xyz"}, {"a", 65536}, {"a", 32768, "b"}, {"abc", 21845}, {"q", 100, ""}, {string.rep("z", 300), 200, "0123456789"}}
  for _, c in ipairs(cases) do
    local got = string.rep(c[1], c[2], c[3])
    if got ~= ref(c[1], c[2], c[3]) then error("MISMATCH " .. c[1]:sub(1, 5) .. " " .. c[2]) end
  end
  assert(string.rep("x", 0) == "" and string.rep("x", -3) == "" and string.rep("", 10) == "")
  assert(#string.rep("a", 65536) == 65536)
  assert(not pcall(string.rep, "a", 65537)) assert(not pcall(string.rep, "ab", 32768, "c"))
  assert(#string.rep("ab", 21845, "c") == 65534)
  assert(not pcall(string.rep, "ab", 21846, "c"))
  assert(#string.rep("ab", 16384, "cd") == 65534)
  error("REP OK")
end
