-- Spec fixture: probes for the patched-Lua hardening (64 KiB string cap,
-- __pairs on read-only tables, address-free tostring). Each probe runs under
-- pcall and reports through a lamp: 1 = held, 0 = did not.
local p = pinball
local CAP = 65536

local function report(lamp, probe)
  local ok, held = pcall(probe)
  p.set_lamp(lamp, (ok and held) and 1 or 0)
end

local function fails(f, ...)
  local ok = pcall(f, ...)
  return not ok
end

local name_tables = {
  "sw", "coil", "lamp", "magnet", "diverter", "flipper",
  "dispenser", "profile", "sound", "scene", "persist_key",
}
local expected = {
  sw = { start = 0, left_sling = 7, right_sling = 8, trough_1 = 20, spinner = 63, far = 255 },
  coil = { trough_eject = 1, left_sling = 3, knocker = 31 },
  lamp = { shoot_again = 2, jackpot = 200 },
  magnet = { center = 4 },
  diverter = { ramp_gate = 5 },
  flipper = { left = 0, right = 1, upper = 6 },
  dispenser = { trough = 9 },
  profile = { steel = 0, ceramic = 11 },
  sound = { bell = 12, drain = 13 },
  scene = { attract = 14, multiball = 15 },
  persist_key = { high_score_1 = 0, credits = 16, last = 63 },
}

-- True when pairs(tbl) yields exactly the expected name -> id set and plain
-- indexing agrees.
local function iterates_exactly(t)
  local want, seen, n = expected[t], {}, 0
  for k, v in pairs(p[t]) do
    if want[k] ~= v or seen[k] then return false end
    seen[k] = true
    n = n + 1
  end
  local m = 0
  for k, v in pairs(want) do
    m = m + 1
    if not seen[k] or p[t][k] ~= v then return false end
  end
  return n == m
end

local cases = {}

-- 1: string cap.
cases[1] = function()
  local a30 = string.rep("a", 30000)
  local a32 = string.rep("b", 32768)
  local a40 = string.rep("c", 40000)
  report(0, function() return #string.rep("x", CAP) == CAP end)
  report(1, function() return fails(string.rep, "x", CAP + 1) end)
  report(2, function() return fails(string.rep, "x", 1e9) end)
  report(3, function() return #(a32 .. a32) == CAP end)
  report(4, function() return fails(function() return a40 .. a40 end) end)
  report(5, function() return fails(function() return a32 .. a32 .. "!" end) end)
  report(6, function() return fails(string.format, "%s%s", a40, a40) end)
  report(7, function() return #string.format("%s%s", a30, a30) == 60000 end)
  report(8, function() return fails(table.concat, { a30, a30, a30 }) end)
  report(9, function() return #table.concat({ a30, a30 }) == 60000 end)
  report(10, function() return fails(string.rep, "ab", 30000, ",") end)
  report(11, function() return fails(table.concat, { a40, a40 }, ", ") end)
  report(12, function() return fails(function() return a40:rep(2) end) end)
  report(13, function() return fails(string.gsub, a40, "c", "cc") end)
  report(14, function()
    -- Padding through string.format either errors or stays under the cap.
    local ok, s = pcall(string.format, "%99s%99s", "x", "y")
    return ok and #s == 198
  end)
  report(15, function() return #a40:upper() == 40000 and #a40:reverse() == 40000 end)
  report(16, function()
    -- Incremental growth is stopped too.
    local s = a32
    local ok = pcall(function() for i = 1, 8 do s = s .. s end end)
    return (not ok) and #s <= CAP
  end)
end

-- 2: pairs over name tables.
cases[2] = function()
  for i, t in ipairs(name_tables) do
    report(i - 1, function() return iterates_exactly(t) end)
  end
end

-- 3: nothing pairs() hands out is a writable backing store.
cases[3] = function()
  for i, t in ipairs(name_tables) do
    report(i - 1, function()
      local name, id = next(expected[t])
      local f, state, ctl = pairs(p[t])
      -- Probed one by one: ipairs would stop at the first nil (ctl is nil).
      local function mutate(obj)
        pcall(rawset, obj, name, 9999)
        pcall(rawset, obj, "spec_pairs_new", 4242)
        pcall(function() obj[name] = 9999 end)
        pcall(function() obj.spec_pairs_new = 4242 end)
        pcall(setmetatable, obj, {})
      end
      mutate(state)
      mutate(ctl)
      mutate(f)
      -- Also through whatever keys/values the iterator itself yields.
      pcall(function()
        for k, v in pairs(p[t]) do
          pcall(rawset, p[t], k, 9999)
        end
      end)
      return p[t][name] == id and p[t].spec_pairs_new ~= 4242 and iterates_exactly(t)
    end)
  end
end

-- 4: pairs(pinball) cannot be used to replace API functions.
local hijacked = false
cases[4] = function()
  local function evil() hijacked = true end
  pcall(function()
    local f, state, ctl = pairs(p)
    -- Probed one by one: ipairs would stop at the first nil.
    local function mutate(obj)
      pcall(rawset, obj, "pulse_coil", evil)
      pcall(function() obj.pulse_coil = evil end)
      pcall(function() obj.sw = { left_sling = 9999 } end)
      pcall(rawset, obj, "sw", { left_sling = 9999 })
    end
    mutate(state)
    mutate(ctl)
  end)
  pcall(function()
    for k, v in pairs(p) do
      pcall(function() p[k] = evil end)
      pcall(rawset, p, k, evil)
    end
  end)
  pinball.pulse_coil(3, 30)
  p.set_lamp(0, hijacked and 0 or 1)
  p.set_lamp(1, (type(pinball.sw) ~= "function" and pinball.sw.left_sling == 7) and 1 or 0)
  p.set_lamp(2, (pinball.tick ~= evil and math.type(pinball.tick()) == "integer") and 1 or 0)
end

-- 5: no addresses leak through tostring / %p.
cases[5] = function()
  local function clean(v)
    local s = tostring(v)
    return type(s) == "string" and not s:find("0x") and not s:find("%x%x%x%x%x%x%x%x")
  end
  report(0, function() return clean({}) end)
  report(1, function() return clean(print) end)
  report(2, function() return clean(function() end) end)
  report(3, function() return clean(pinball) and clean(pinball.sw) and clean(pinball.pulse_coil) end)
  report(4, function() return fails(string.format, "%p", {}) end)
  report(5, function() return fails(string.format, "%p", print) end)
  report(6, function()
    local s = string.format("%s", {})
    return not s:find("0x") and not s:find("%x%x%x%x%x%x%x%x")
  end)
  report(7, function()
    -- Error messages for bad arguments must not leak addresses either.
    local ok, msg = pcall(function() return {} < {} end)
    local ok2, msg2 = pcall(error, {})
    return not tostring(msg):find("0x") and not tostring(msg2):find("0x")
  end)
  report(8, function() return clean(string) and clean(math.random) and clean(_G) end)
end

function on_switch(id, closed)
  local case = cases[id]
  if case then case() end
end
