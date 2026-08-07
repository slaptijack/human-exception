-- Drives the drone straight into the wall block east of the fixed starting
-- position (0,0): the first move to (1,0) is a wall tile.
function on_tick(observation)
  return "east"
end
