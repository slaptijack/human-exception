-- A reference reconnaissance controller for the "First Contact"
-- operation. Run it with:
--
--   cargo run -- --developer-mode examples/first_contact.lua
--
-- Each tick, on_tick(observation) receives a read-only snapshot of the
-- drone's own position, the current tick, the operational budget
-- remaining (observation.budget_remaining), and observation.discovered:
-- every tile learned about so far (its own tile and cardinal neighbours
-- every tick, plus anything revealed by a completed "scan"). It must
-- return one action name: "north", "south", "east", "west", "wait", or
-- "scan".
--
-- The strategy is five plain rules, checked in order every tick:
--
--  1. Remember every tile discovered so far, and remember everywhere
--     the drone has already been.
--  2. If the uplink has been discovered, move toward it one step at a
--     time, using only ground already confirmed safe.
--  3. Otherwise, prefer a safe tile the drone hasn't visited yet,
--     breaking ties in a fixed order (see DIRECTIONS below) -- an
--     arbitrary but consistent tie-break, not a claim that any one
--     direction matters more than another.
--  4. If there's no such tile (every neighbour is either unsafe or
--     already visited) and the drone hasn't just looked around, scan:
--     a wider look is the obvious next move once nearby ground is used
--     up.
--  5. If a scan doesn't turn up anywhere fresh to go either, the
--     drone is boxed in by what little it knows: cross a known hazard
--     if one is available rather than sit still, and failing even
--     that, step back onto ground it's already covered.
--
-- This strategy has no privileged knowledge of the facility: it reacts
-- only to what it discovers and to its own memory of where it's been.
-- It reliably solves the operation `--developer-mode` runs. Deployed
-- through the console, where a deployment can draw a different
-- authored configuration, it's known to run out of budget on at least
-- one of them: reaching the uplink can require the drone to guess,
-- from an identical-looking fork early on, which of two directions
-- actually leads there, and a wrong guess here costs more to correct
-- than the operation's budget allows. See issue #199 for that finding
-- in full, including whether the fix belongs in this controller or in
-- the operation's tuning.

local known = {}
local visited = {}
local uplink = nil
local scanned_while_stuck = false

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

-- A tile is safe to move onto once we've confirmed it's traversable
-- and isn't a hazard.
local function is_safe(x, y)
  local tile = known_tile(x, y)
  return tile ~= nil and tile.traversable and tile.tile ~= "hazard"
end

local function is_hazard(x, y)
  local tile = known_tile(x, y)
  return tile ~= nil and tile.tile == "hazard"
end

local function is_visited(x, y)
  return visited[tile_key(x, y)] == true
end

-- The four neighbours of (x, y), in a fixed, arbitrary tie-break order.
local DIRECTIONS = {
  { 0, 1, "north" },
  { 1, 0, "east" },
  { 0, -1, "south" },
  { -1, 0, "west" },
}

-- Rule 2: step toward the known uplink using only confirmed-safe
-- ground, closing whichever axis is further away first. Falls back to
-- ordinary exploration if the direct move isn't yet confirmed safe.
local function home_toward_uplink(x, y)
  local dx, dy = uplink.x - x, uplink.y - y
  local moves = {}
  if math.abs(dx) >= math.abs(dy) then
    if dx > 0 then
      table.insert(moves, "east")
    elseif dx < 0 then
      table.insert(moves, "west")
    end
    if dy > 0 then
      table.insert(moves, "north")
    elseif dy < 0 then
      table.insert(moves, "south")
    end
  else
    if dy > 0 then
      table.insert(moves, "north")
    elseif dy < 0 then
      table.insert(moves, "south")
    end
    if dx > 0 then
      table.insert(moves, "east")
    elseif dx < 0 then
      table.insert(moves, "west")
    end
  end
  for _, name in ipairs(moves) do
    for _, direction in ipairs(DIRECTIONS) do
      if direction[3] == name and is_safe(x + direction[1], y + direction[2]) then
        return name
      end
    end
  end
  return nil
end

-- Rule 3: an unvisited safe neighbour, in the fixed tie-break order.
local function unvisited_safe_neighbour(x, y)
  for _, direction in ipairs(DIRECTIONS) do
    local nx, ny = x + direction[1], y + direction[2]
    if is_safe(nx, ny) and not is_visited(nx, ny) then
      return direction[3]
    end
  end
  return nil
end

-- Rule 5's hazard fallback: a hazard neighbour worth crossing because
-- it's the only way to keep moving.
local function unvisited_hazard_neighbour(x, y)
  for _, direction in ipairs(DIRECTIONS) do
    local nx, ny = x + direction[1], y + direction[2]
    if is_hazard(nx, ny) and not is_visited(nx, ny) then
      return direction[3]
    end
  end
  return nil
end

-- Rule 5's backtrack fallback: any confirmed-safe neighbour at all,
-- even one already visited, so the drone keeps moving rather than
-- stalling.
local function any_safe_neighbour(x, y)
  for _, direction in ipairs(DIRECTIONS) do
    if is_safe(x + direction[1], y + direction[2]) then
      return direction[3]
    end
  end
  return nil
end

function on_tick(observation)
  for _, tile in ipairs(observation.discovered) do
    remember(tile)
  end

  local x, y = observation.drone.x, observation.drone.y
  visited[tile_key(x, y)] = true

  if uplink ~= nil then
    local towards_uplink = home_toward_uplink(x, y)
    if towards_uplink ~= nil then
      scanned_while_stuck = false
      return towards_uplink
    end
  end

  local fresh_move = unvisited_safe_neighbour(x, y)
  if fresh_move ~= nil then
    scanned_while_stuck = false
    return fresh_move
  end

  if not scanned_while_stuck then
    scanned_while_stuck = true
    return "scan"
  end

  return unvisited_hazard_neighbour(x, y) or any_safe_neighbour(x, y) or "scan"
end
