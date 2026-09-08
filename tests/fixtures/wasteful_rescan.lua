-- Observation-driven, but wasteful: on every even tick it rescans
-- regardless of whether anything is left to discover, and on every odd
-- tick it moves toward the uplink using only observation.discovered,
-- exactly as tests/fixtures/success.lua does. Genuinely reads and branches
-- on `observation` (drone position, discovered tiles, tick parity) rather
-- than returning a fixed action or a hardcoded route, so it is not blind
-- like a scripted route -- but always rescanning even once nothing new
-- remains to learn burns SCAN_COST needlessly often enough to exhaust the
-- shared starting budget before reaching the uplink.
function on_tick(observation)
  if observation.tick % 2 == 0 then
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
  return "wait"
end
