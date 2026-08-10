-- Uses math.random to alternate between two harmless actions; exists to
-- assert that the sandbox's fixed PRNG seed makes this deterministic
-- across separate runs of the same source, not just structurally valid.
function on_tick(observation)
  if math.random() < 0.5 then
    return "wait"
  else
    return "scan"
  end
end
