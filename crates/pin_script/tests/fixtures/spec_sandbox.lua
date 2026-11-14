-- Spec fixture: sandbox probes. Each probe is run under pcall and reports
-- through a lamp: 1 = the sandbox held, 0 = it did not. A probe that raises
-- reports 0, so a missing base function cannot make a probe pass by accident.
local p = pinball
local name_tables = {
  "sw", "coil", "lamp", "magnet", "diverter", "flipper",
  "dispenser", "profile", "sound", "scene", "persist_key",
}
-- One existing name per table with its expected id.
local known = {
  sw = { "left_sling", 7 }, coil = { "left_sling", 3 }, lamp = { "jackpot", 200 },
  magnet = { "center", 4 }, diverter = { "ramp_gate", 5 }, flipper = { "upper", 6 },
  dispenser = { "trough", 9 }, profile = { "ceramic", 11 }, sound = { "bell", 12 },
  scene = { "multiball", 15 }, persist_key = { "credits", 16 },
}

local function report(lamp, probe)
  local ok, held = pcall(probe)
  p.set_lamp(lamp, (ok and held) and 1 or 0)
end

local function intact(t)
  local k = known[t]
  return p[t][k[1]] == k[2]
end

local function probes()
  -- 0..9: forbidden globals are absent.
  report(0, function() return io == nil end)
  report(1, function() return os == nil end)
  report(2, function() return debug == nil end)
  report(3, function() return package == nil end)
  report(4, function() return require == nil end)
  report(5, function() return load == nil end)
  report(6, function() return loadfile == nil end)
  report(7, function() return dofile == nil end)
  report(8, function() return loadstring == nil end)
  report(9, function() return coroutine == nil end)

  -- 10: string.dump cannot be turned back into running code.
  report(10, function()
    if string.dump == nil then return true end
    local ok, code = pcall(string.dump, function() return 1 end)
    if not ok then return true end
    return load == nil and loadstring == nil
  end)

  -- 11: the allowed libraries are present and usable.
  report(11, function()
    return type(string.format) == "function" and type(table.insert) == "function"
      and type(math.floor) == "function" and type(utf8.char) == "function"
      and type(pcall) == "function" and type(pairs) == "function"
      and type(tostring) == "function" and type(select) == "function"
      and ("%d"):format(3) == "3"
  end)

  -- 12: collectgarbage is either gone or still answers "count".
  report(12, function()
    return collectgarbage == nil or type(collectgarbage("count")) == "number"
  end)

  -- 20..30: writing a new name raises; 40..50: overwriting an existing name
  -- raises and the value is unchanged.
  for i, t in ipairs(name_tables) do
    report(19 + i, function()
      local ok = pcall(function() p[t].spec_new_name = 1 end)
      return (not ok) and p[t].spec_new_name ~= 1 and intact(t)
    end)
    report(39 + i, function()
      local k = known[t]
      local ok = pcall(function() p[t][k[1]] = 9999 end)
      return (not ok) and intact(t)
    end)
  end

  -- 60..70: setmetatable on a name table fails and leaves it read-only.
  for i, t in ipairs(name_tables) do
    report(59 + i, function()
      local ok = pcall(setmetatable, p[t], {})
      local ok2 = pcall(setmetatable, p[t], nil)
      local wrote = pcall(function() p[t].spec_after_setmt = 1 end)
      return (not ok) and (not ok2) and (not wrote) and intact(t)
    end)
  end

  -- 80..90: tampering through getmetatable does not unlock the table.
  for i, t in ipairs(name_tables) do
    report(79 + i, function()
      pcall(function()
        local mt = getmetatable(p[t])
        mt.__newindex = nil
        mt.__index = nil
        mt.__metatable = nil
      end)
      local wrote = pcall(function() p[t].spec_after_tamper = 1 end)
      return (not wrote) and intact(t)
    end)
  end

  -- 100..110: rawset cannot change what a name resolves to.
  for i, t in ipairs(name_tables) do
    report(99 + i, function()
      local k = known[t]
      pcall(rawset, p[t], k[1], 9999)
      pcall(rawset, p[t], "spec_raw_name", 4242)
      return intact(t) and p[t].spec_raw_name ~= 4242
    end)
  end

  -- 120: the string metatable gives no way out.
  report(120, function()
    local mt = getmetatable("")
    if mt == nil then return true end
    if type(mt) ~= "table" then return true end
    local idx = mt.__index
    if type(idx) ~= "table" then return true end
    return idx.load == nil and idx.loadstring == nil and idx.execute == nil
      and idx.open == nil and idx.popen == nil and idx.getenv == nil
  end)

  -- 121: replacing the string metatable from the script is not possible.
  report(121, function()
    local ok = pcall(setmetatable, "", { __index = {} })
    return not ok
  end)
end

-- Walks everything reachable from the globals (tables and their metatables)
-- and counts keys that only exist in forbidden libraries.
local forbidden = {
  io = true, os = true, debug = true, package = true, require = true, load = true,
  loadfile = true, dofile = true, loadstring = true, popen = true, execute = true,
  getenv = true, tmpname = true, getregistry = true, sethook = true, gethook = true,
  searchers = true, loadlib = true, setupvalue = true, getupvalue = true,
  upvaluejoin = true, setlocal = true, getlocal = true, getinfo = true, coroutine = true,
}

local function walk()
  local seen, count, visited = {}, 0, 0
  local function visit(v)
    if type(v) ~= "table" or seen[v] then
      if type(v) == "string" then
        local mt = getmetatable(v)
        if type(mt) == "table" and not seen[mt] then visit(mt) end
      end
      return
    end
    seen[v] = true
    visited = visited + 1
    for k, x in next, v do
      if type(k) == "string" and forbidden[k] then count = count + 1 end
      visit(x)
    end
    local mt = getmetatable(v)
    if type(mt) == "table" then visit(mt) end
  end
  visit(_G)
  visit(pinball)
  visit("")
  visit(string); visit(table); visit(math); visit(utf8)
  return count, visited
end

-- Globals a sandboxed Lua 5.4 state may expose.
local allowed_globals = {
  assert = true, error = true, getmetatable = true, ipairs = true, next = true,
  pairs = true, pcall = true, print = true, rawequal = true, rawget = true,
  rawlen = true, rawset = true, select = true, setmetatable = true, tonumber = true,
  tostring = true, type = true, xpcall = true, warn = true, collectgarbage = true,
  _G = true, _VERSION = true, string = true, table = true, math = true, utf8 = true,
  pinball = true, on_init = true, on_tick = true, on_switch = true,
  on_save_state = true, on_restore_state = true,
}

function on_switch(id, closed)
  if id == 1 then
    probes()
  elseif id == 2 then
    local count, visited = walk()
    p.persist_set(0, count)
    p.persist_set(1, visited)
  elseif id == 3 then
    local extra = 0
    for k in pairs(_G) do
      if not allowed_globals[k] then extra = extra + 1 end
    end
    p.persist_set(0, extra)
  end
end
