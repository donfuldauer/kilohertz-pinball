-- Spec fixture: version 2. Restores what any earlier version saved and
-- reports what it saw through the command queue on every tick.
local p = pinball
local n = 0
local hits = {}
local restored = 0
local intact = 0
local from = "none"

function on_restore_state(s)
  restored = restored + 1
  intact = (type(s) == "table"
    and math.type(s.n) == "integer"
    and type(s.hits) == "table"
    and s.flag == true
    and s.ratio == 0.5
    and s.big == 9007199254740993 and math.type(s.big) == "integer"
    and type(s.nested) == "table" and type(s.nested.deep) == "table"
    and s.nested.deep.value == 42) and 1 or 0
  n = s.n
  hits = s.hits
  from = s.label
end

function on_tick()
  n = n + 1
  p.set_score(2, n)
  p.persist_set(0, restored)
  p.persist_set(1, intact)
  p.persist_set(2, #hits)
  p.persist_set(3, (from == "v1" and 1) or (from == "v2" and 2) or 0)
  for i, id in ipairs(hits) do p.pulse_coil(id, i) end
end

function on_switch(id, closed)
  if closed then hits[#hits + 1] = id end
end

function on_save_state()
  return {
    n = n,
    hits = hits,
    label = "v2",
    flag = true,
    ratio = 0.5,
    big = 9007199254740993,
    nested = { deep = { value = 42 } },
  }
end
