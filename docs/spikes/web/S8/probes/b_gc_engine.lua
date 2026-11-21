-- (b) GC pacing reaching script-observable results under the engine limits:
-- every tick keeps 3 KB and makes 40 KB of garbage. Near the 2 MiB cap
-- allocations trigger emergency collections, each charged heap/64 to the
-- budget (KHZ 19). Observable: the tick and kind of the first fault (B or M)
-- and the scores set before it (the harness prints EVENTS and CMD).
keep = {} n = 0
function on_tick()
  n = n + 1
  keep[n] = string.rep('k', 3000) .. n
  for i = 1, 10 do local g = string.rep('g', 4000) .. i end
  pinball.set_score(1, n)
end
