-- Spec 2.9 detector script: one ball from `fly_feed` (thrown over the left
-- wall) on the first tick, nothing else (no game, no serve on `start`).
local done = false
function on_init() end
function on_switch(id, closed) end
function on_tick()
  if not done then
    done = true
    pinball.spawn_ball(pinball.dispenser.fly_feed, pinball.profile.standard_steel)
  end
end
