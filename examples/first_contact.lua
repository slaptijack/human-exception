-- A minimal controller for the "first contact" training operation.
-- Run it with:
--
--   cargo run -- examples/first_contact.lua
--
-- Each tick, on_tick(observation) receives a read-only snapshot of the
-- drone's own position, the current tick, the operational budget
-- remaining (observation.budget_remaining), and observation.discovered:
-- every tile learned about so far (its own tile
-- and cardinal neighbours every tick, plus anything revealed by a
-- completed "scan"). It must return one action name: "north", "south",
-- "east", "west", "wait", or "scan". This script only ever moves onto a
-- tile it has already confirmed is traversable, heads straight for the
-- uplink once it turns up in observation.discovered, and scans if it ever
-- runs out of confirmed moves.
function on_tick(observation)
  local function find_tile(x, y)
    for _, tile in ipairs(observation.discovered) do
      if tile.x == x and tile.y == y then
        return tile
      end
    end
    return nil
  end

  local function is_open(x, y)
    local tile = find_tile(x, y)
    return tile ~= nil and tile.traversable
  end

  for _, tile in ipairs(observation.discovered) do
    if tile.uplink then
      if observation.drone.y < tile.y and is_open(observation.drone.x, observation.drone.y + 1) then
        return "north"
      end
      if observation.drone.x < tile.x and is_open(observation.drone.x + 1, observation.drone.y) then
        return "east"
      end
      return "wait"
    end
  end

  local x, y = observation.drone.x, observation.drone.y
  if is_open(x, y + 1) then
    return "north"
  end
  if is_open(x + 1, y) then
    return "east"
  end
  return "scan"
end
