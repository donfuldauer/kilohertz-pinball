-- Spec fixture: fault scenarios, one per switch id. Case 100+ arm the same
-- scenario for the next on_tick instead of running it in on_switch.
local p = pinball
local cases = {}
local acc = 0
local armed = nil

-- 1: plain infinite loop.
cases[1] = function() while true do end end

-- 2: infinite loop that swallows errors with pcall; the budget abort must not
-- be catchable by the script.
cases[2] = function()
  while true do
    pcall(function() while true do end end)
  end
end

-- 3: memory bomb.
cases[3] = function()
  local t = {}
  while true do
    t[#t + 1] = string.rep("x", 60000) .. #t -- stays under the 64 KiB string cap
  end
end

-- 4: memory bomb that swallows the allocation error and retries forever.
cases[4] = function()
  while true do
    pcall(function()
      local t = {}
      while true do t[#t + 1] = string.rep("y", 60000) .. #t end
    end)
  end
end

-- 5: one absurd allocation.
cases[5] = function() local s = string.rep("x", 1 << 40) return #s end

-- 6: explicit error.
cases[6] = function() error("boom") end

-- 7: error value whose __tostring never returns.
cases[7] = function()
  error(setmetatable({}, { __tostring = function() while true do end end }))
end

-- 8/9: argument errors.
cases[8] = function() p.pulse_coil("not a coil", 10) end
cases[9] = function() p.pulse_coil() end

-- 10: calling an undefined global.
cases[10] = function() this_function_does_not_exist() end

-- 11/12: coil holds without any fault.
cases[11] = function() p.hold_coil(3, true) p.hold_coil(5, true) end
cases[12] = function() p.hold_coil(3, false) end

-- 13: hold then hang in the same callback.
cases[13] = function() p.hold_coil(7, true) while true do end end

-- 14: unbounded recursion (non-tail).
cases[14] = function()
  local function f(n) return 1 + f(n + 1) end
  return f(1)
end

-- 15: bounded work well inside the budget; state persists across callbacks.
cases[15] = function()
  for i = 1, 1000 do acc = acc + i end
  p.set_score(1, acc)
end

-- 16: garbage whose finalizers never return.
cases[16] = function()
  for i = 1, 200 do
    setmetatable({}, { __gc = function() while true do end end })
  end
end

-- 17: commands, then an error: pushes before the error are not required to
-- survive, but nothing after it may run.
cases[17] = function()
  p.pulse_coil(1, 10)
  error("after one pulse")
end

-- 18: the same coil held twice.
cases[18] = function() p.hold_coil(3, true) p.hold_coil(3, true) end

-- 19: error raised with a non-string, non-table value.
cases[19] = function() error(nil) end

-- 20: more argument errors.
cases[20] = function() p.set_lamp({}, 1) end
cases[21] = function() p.set_score("one", 10) end
cases[22] = function() p.hold_coil(nil, true) end
cases[23] = function() p.persist_get("credits") end

-- 24: infinite loop inside a metamethod.
cases[24] = function()
  local t = setmetatable({}, { __index = function() while true do end end })
  return t.x
end

-- 25: xpcall with a handler that itself loops.
cases[25] = function()
  while true do
    xpcall(function() error("x") end, function() while true do end end)
  end
end

-- 26: goto-based loop without any function call.
cases[26] = function()
  local i = 0
  ::top::
  i = i + 1
  goto top
end

-- 27: a trivial, healthy callback.
cases[27] = function() p.pulse_coil(2, 20) end

function on_switch(id, closed)
  if id >= 100 then
    armed = id - 100
    return
  end
  local case = cases[id]
  if case then case() end
end

function on_tick()
  if armed then cases[armed]() end
end
