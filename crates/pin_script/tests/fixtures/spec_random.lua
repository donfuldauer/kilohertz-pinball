-- Spec fixture: a stateless script whose every command depends on the host
-- generator, through both pinball.random and math.random.
local p = pinball

function on_tick()
  p.pulse_coil(p.random(6), 1)                 -- [1, 6]
  p.pulse_coil(p.random(10, 20), 2)            -- [10, 20]
  p.pulse_coil(math.random(8), 3)              -- [1, 8]
  p.pulse_coil(math.random(100, 105), 4)       -- [100, 105]
  p.set_lamp(0, math.random())                 -- [0, 1)
  p.persist_set(0, p.random(1000000))
  p.persist_set(1, math.random(1, 1000000))
  p.persist_set(2, p.random(1))                -- always 1
  p.persist_set(3, p.random(5, 5))             -- always 5
  p.persist_set(4, p.random(-5, -1))           -- [-5, -1]
  p.persist_set(5, math.random(-2, 2))         -- [-2, 2]
  local a, b = p.random(6), math.random(6)
  p.persist_set(6, (math.type(a) == "integer" and math.type(b) == "integer"
    and math.type(math.random()) == "float") and 1 or 0)
end

function on_switch(id, closed)
  if closed then
    p.pulse_coil(id % 32, p.random(50))
  else
    p.set_score(p.random(4), math.random(0, 999))
  end
end
