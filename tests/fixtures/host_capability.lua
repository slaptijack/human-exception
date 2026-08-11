-- Reaches for a host capability that shouldn't be available to player
-- scripts; used to assert the sandbox rejects this at load time.
os.execute("true")

function on_tick(observation)
  return "wait"
end
