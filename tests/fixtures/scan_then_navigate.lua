-- Scans once on the first tick, then explores toward the uplink using only
-- observation.discovered, matching tests/fixtures/success.lua otherwise.
function on_tick(observation)
  if observation.tick == 0 then
    return "scan"
  end

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
