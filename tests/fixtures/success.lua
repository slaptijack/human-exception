-- Walks the drone from (0,0) to the uplink at (4,4) in the fixed
-- "first contact" scenario.
function on_tick(observation)
  if observation.drone.y < observation.uplink.y then
    return "north"
  end
  if observation.drone.x < observation.uplink.x then
    return "east"
  end
  return "wait"
end
