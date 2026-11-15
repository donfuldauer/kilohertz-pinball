-- Spec 2.9 detector script: the left flipper coil is held energised every
-- tick (a held cradle); on tick 100 one ball drops from `cradle_feed` onto the
-- raised flipper and settles there. Nothing else.
local n = 0
function on_init() end
function on_switch(id, closed) end
function on_tick()
  pinball.hold_coil(pinball.coil.left_flipper_coil, true)
  n = n + 1
  if n == 100 then
    pinball.spawn_ball(pinball.dispenser.cradle_feed, pinball.profile.standard_steel)
  end
end
