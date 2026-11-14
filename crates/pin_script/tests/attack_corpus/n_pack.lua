function on_tick()
  local ok, e = pcall(string.pack, "c1000000", "") 
  local pc, sp = pcall, string.pack
  while true do pc(sp, "c10000000", "") end
end
