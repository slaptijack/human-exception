-- Observation-driven, but wasteful: rescans every tick regardless of what
-- is already discovered, rather than moving toward the uplink or scanning
-- only when it would add new information. Reads `observation` (unlike a
-- blind scripted route) but never acts on it, so it reliably exhausts the
-- shared starting budget on scans alone before ever reaching the uplink.
-- Used to prove wasteful-but-legitimate-shaped play can fail on budget for
-- a mechanically understandable reason.
function on_tick(observation)
  return "scan"
end
