-- Spec 6.1 fixture rules (Independent Tester): replaces the table's
-- rules.lua in a scratch copy of the built bundle so that the Zipper slide of
-- both lower flippers is engaged (`ZIPPER = true`) or released
-- (`ZIPPER = false`, the line is rewritten by the test) from the first tick,
-- with no game logic. Used only by spec_6_1_contact's Zipper gap test.
local ZIPPER = true

function on_init()
end

function on_tick()
  local f = pinball.flipper
  pinball.set_zipper(f.left_flipper, ZIPPER)
  pinball.set_zipper(f.right_flipper, ZIPPER)
end

function on_switch(id, closed)
end
