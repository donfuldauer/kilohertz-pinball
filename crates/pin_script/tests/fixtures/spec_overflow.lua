-- Spec fixture: each case pushes `n` commands into one list (n is carried by
-- the host in persist slot 0 so the test chooses how far past capacity to go).
local p = pinball
local push = {
  [1] = function(i) p.pulse_coil(i % 30, 10) end,
  [2] = function(i) p.hold_coil(i % 30, true) end,
  [3] = function(i) p.set_magnet(i % 4, 0.5) end,
  [4] = function(i) p.set_zipper(i % 4, true) end,
  [5] = function(i) p.set_diverter(i % 4, true) end,
  [6] = function(i) p.spawn_ball(0, 0) end,
  [7] = function(i) p.set_lamp(i % 200, 1) end,
  [8] = function(i) p.set_score((i % 4) + 1, i) end,
  [9] = function(i) p.show_scene(i % 4) end,
  [10] = function(i) p.play_sound(i % 4, 1) end,
  [11] = function(i) p.persist_set(i % 60, i) end,
  [12] = function(i) p.set_gi_tint(0, 0, 0, i % 2) end,
}

function on_switch(id, closed)
  local n = p.persist_get(0)
  local f = push[id]
  for i = 1, n do f(i) end
end
