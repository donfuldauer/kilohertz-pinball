-- Spec fixture: version 1 of a hot-reloaded rule set. Saves state, has no
-- restore hook of its own.
local p = pinball
local n = 0
local hits = {}

function on_tick()
  n = n + 1
  p.set_score(1, n)
end

function on_switch(id, closed)
  if closed then hits[#hits + 1] = id end
  p.pulse_coil(p.coil.knocker, #hits)
end

function on_save_state()
  return {
    n = n,
    hits = hits,
    label = "v1",
    flag = true,
    ratio = 0.5,
    big = 9007199254740993,
    nested = { deep = { value = 42 } },
  }
end
