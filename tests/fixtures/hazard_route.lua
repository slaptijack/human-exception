-- Deliberately takes the row-y=1-then-column-x=4 route to the uplink in the
-- fixed "first contact" scenario, which passes through the hazard tile at
-- (4, 2), to exercise hazard-entry cost and event reporting end to end.
local route = { "north", "east", "east", "east", "east", "north", "north", "north" }
local step = 0

function on_tick(observation)
  step = step + 1
  return route[step]
end
