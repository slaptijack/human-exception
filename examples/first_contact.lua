-- A minimal controller for the "first contact" training operation.
-- Run it with:
--
--   cargo run -- examples/first_contact.lua
--
-- Each tick, on_tick(observation) receives a read-only snapshot of the
-- drone's position, the uplink's position, the current tick, and the
-- ticks remaining, and must return one action name: "north", "south",
-- "east", "west", or "wait". This script closes the vertical gap first,
-- then the horizontal gap, then waits once it has arrived.
function on_tick(observation)
  if observation.drone.y < observation.uplink.y then
    return "north"
  end
  if observation.drone.x < observation.uplink.x then
    return "east"
  end
  return "wait"
end
