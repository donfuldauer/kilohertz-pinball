keep = {}
function on_tick() xpcall(function() while true do keep[#keep + 1] = string.rep("x", 1e6) .. #keep end end, function(e) while true do end end) end
