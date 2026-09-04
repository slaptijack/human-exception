-- Deliberately takes the shared row-y=1 corridor east, then turns south
-- onto first_contact_south_uplink()'s uplink spur at (4, 0), the same
-- hazard-free route `simulation::tests::the_south_uplink_configuration_admits_a_shorter_passive_route`
-- exercises directly. Scripted like tests/fixtures/hazard_route.lua, rather
-- than reactive like tests/fixtures/success.lua, because this authored
-- configuration's uplink sits south of the shared corridor, which the other
-- fixtures' shared "prefer north, then east" navigation never turns toward
-- (bringing the reference/starter controllers up to date with every
-- authored configuration is tracked separately, not by this fixture).
local route = { "north", "east", "east", "east", "east", "south" }
local step = 0

function on_tick(observation)
  step = step + 1
  return route[step]
end
