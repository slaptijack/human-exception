-- A reference reconnaissance controller for the fixed "First Contact"
-- operation. Run it with:
--
--   cargo run -- examples/first_contact.lua
--
-- Each tick, on_tick(observation) receives a read-only snapshot of the
-- drone's own position, the current tick, the operational budget
-- remaining (observation.budget_remaining), and observation.discovered:
-- every tile learned about so far (its own tile and cardinal neighbours
-- every tick, plus anything revealed by a completed "scan"). It must
-- return one action name: "north", "south", "east", "west", "wait", or
-- "scan".
--
-- The controller starts with no hard-coded knowledge of the facility's
-- layout or the uplink's location. It opens with a scan to map the
-- surrounding area before committing to a direction, keeps its own
-- memory of every tile it has discovered, prefers a known non-hazard
-- tile over a known hazard tile whenever both are available, heads for
-- the uplink once it has been discovered, and scans again whenever it
-- runs out of confirmed safe moves.

-- Persistent memory: every tile discovered so far, indexed by "x,y".
-- Declared outside on_tick so it survives between ticks instead of being
-- rebuilt from observation.discovered on every call.
local known = {}
local uplink = nil
local scanned_start = false

local function tile_key(x, y)
  return x .. "," .. y
end

local function remember(tile)
  known[tile_key(tile.x, tile.y)] = tile
  if tile.uplink then
    uplink = tile
  end
end

local function known_tile(x, y)
  return known[tile_key(x, y)]
end

-- A move is only safe once we've confirmed the tile is traversable and
-- know it isn't a hazard; unconfirmed and hazard tiles are avoided.
local function is_safe(x, y)
  local tile = known_tile(x, y)
  return tile ~= nil and tile.traversable and tile.tile ~= "hazard"
end

function on_tick(observation)
  for _, tile in ipairs(observation.discovered) do
    remember(tile)
  end

  if not scanned_start then
    scanned_start = true
    return "scan"
  end

  local x, y = observation.drone.x, observation.drone.y

  if uplink ~= nil then
    if y < uplink.y and is_safe(x, y + 1) then
      return "north"
    end
    if x < uplink.x and is_safe(x + 1, y) then
      return "east"
    end
    return "wait"
  end

  if is_safe(x, y + 1) then
    return "north"
  end
  if is_safe(x + 1, y) then
    return "east"
  end
  return "scan"
end
