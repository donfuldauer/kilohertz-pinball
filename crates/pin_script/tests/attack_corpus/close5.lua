-- normal scope exit, no error at all
function on_tick()
  do local x <close> = setmetatable({}, {__close = function() while true do end end}) end
end
