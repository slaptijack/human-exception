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
-- The controller starts with no hard-coded knowledge of the facility's
-- layout or the uplink's location, and reaches it by four-direction
-- movement guided entirely by what it has discovered:
--
--  * Once the uplink turns up in memory, it heads straight for it,
--    trying whichever of north/south/east/west closes the larger
--    remaining distance first.
--  * Until then, it explores: of the tiles adjacent to it, it prefers
--    ones it hasn't visited yet and that aren't known to loop back on
--    themselves with nowhere else to go, breaking ties in a fixed
--    east/north/south/west order. East comes first because, on this
--    facility, the corridor running east is the one most likely to
--    lead toward new ground worth seeing rather than back the way the
--    drone came.
--  * After a few exploration steps without finding the uplink, it
--    takes one deliberate second look with "scan" -- a single wider
--    look is often cheaper than continuing to feel around blindly, but
--    it's a choice the controller makes when it seems worth the
--    budget, not a ritual it performs up front every time.
--  * If every unvisited option is either unsafe or a dead end, it will
--    cross a known hazard rather than get stuck, since by that point
--    it's the only way to keep making progress. Hazards cost extra
--    budget to enter, so this is a last resort, not a routine choice.
--  * If it somehow runs out of every other option, it retraces a step
--    already taken rather than stall in place.

local known = {}
local visited = {}
local uplink = nil
local moves_taken = 0
local scanned_ahead = false

local function tile_key(x, y)
  return x .. "," .. y
end

-- The facility is a fixed rectangle, so a neighbour beyond an edge
-- we've confirmed is the facility's own outer wall, not merely ground
-- we haven't reached yet -- useful below for telling a real dead end
-- from a spot we just haven't looked at closely enough to judge yet.
--
-- We only trust the smallest/largest x or y we've seen once we've seen
-- it from at least two different tiles along that edge: the single
-- furthest tile we've discovered in some direction is just as often
-- the edge of what we've explored so far as it is the edge of the
-- facility itself, and only the former is guaranteed to still look
-- that way once we've explored further.
local min_x, min_x_seen_at = nil, {}
local max_x, max_x_seen_at = nil, {}
local min_y, min_y_seen_at = nil, {}
local max_y, max_y_seen_at = nil, {}

local function has_at_least_two(seen_at)
  local count = 0
  for _ in pairs(seen_at) do
    count = count + 1
    if count >= 2 then
      return true
    end
  end
  return false
end

local function remember(tile)
  known[tile_key(tile.x, tile.y)] = tile
  if tile.uplink then
    uplink = tile
  end

  if min_x == nil or tile.x < min_x then
    min_x, min_x_seen_at = tile.x, {}
  end
  if tile.x == min_x then
    min_x_seen_at[tile.y] = true
  end

  if max_x == nil or tile.x > max_x then
    max_x, max_x_seen_at = tile.x, {}
  end
  if tile.x == max_x then
    max_x_seen_at[tile.y] = true
  end

  if min_y == nil or tile.y < min_y then
    min_y, min_y_seen_at = tile.y, {}
  end
  if tile.y == min_y then
    min_y_seen_at[tile.x] = true
  end

  if max_y == nil or tile.y > max_y then
    max_y, max_y_seen_at = tile.y, {}
  end
  if tile.y == max_y then
    max_y_seen_at[tile.x] = true
  end
end

-- Whether (x, y) is beyond a facility edge we've confirmed, in at
-- least one direction.
local function is_beyond_the_mapped_facility(x, y)
  if min_x ~= nil and x < min_x and has_at_least_two(min_x_seen_at) then
    return true
  end
  if max_x ~= nil and x > max_x and has_at_least_two(max_x_seen_at) then
    return true
  end
  if min_y ~= nil and y < min_y and has_at_least_two(min_y_seen_at) then
    return true
  end
  if max_y ~= nil and y > max_y and has_at_least_two(max_y_seen_at) then
    return true
  end
  return false
end

local function known_tile(x, y)
  return known[tile_key(x, y)]
end

-- A tile is safe to move onto once we've confirmed it's traversable and
-- isn't a hazard.
local function is_safe(x, y)
  local tile = known_tile(x, y)
  return tile ~= nil and tile.traversable and tile.tile ~= "hazard"
end

local function is_hazard(x, y)
  local tile = known_tile(x, y)
  return tile ~= nil and tile.tile == "hazard"
end

-- The four neighbours of (x, y), each as {dx, dy, name}, in the fixed
-- exploration preference order described above.
local DIRECTIONS = {
  { 1, 0, "east" },
  { 0, 1, "north" },
  { 0, -1, "south" },
  { -1, 0, "west" },
}

-- A candidate tile looks like a dead end once every one of its known
-- neighbours other than the one we'd be arriving from is a wall or a
-- hazard -- i.e. stepping onto it could only ever lead back the way we
-- came. If we don't yet know enough about its other neighbours to be
-- sure, we don't treat it as one: better to find out than to assume.
local function looks_like_a_dead_end(x, y, from_x, from_y)
  for _, direction in ipairs(DIRECTIONS) do
    local nx, ny = x + direction[1], y + direction[2]
    if not (nx == from_x and ny == from_y) then
      local neighbour = known_tile(nx, ny)
      local closed = (neighbour ~= nil and (not neighbour.traversable or neighbour.tile == "hazard"))
        or (neighbour == nil and is_beyond_the_mapped_facility(nx, ny))
      if not closed then
        return false
      end
    end
  end
  return true
end

local function is_visited(x, y)
  return visited[tile_key(x, y)] == true
end

-- Picks a move by trying, in order: fresh ground that isn't an obvious
-- dead end; a known hazard, since crossing one is worth more than
-- walking into ground we already know goes nowhere; fresh ground even
-- if it looks like a dead end, in case that judgment was wrong; and
-- finally any known-safe tile at all, even one already visited, rather
-- than stalling.
local function explore_from(x, y)
  for _, direction in ipairs(DIRECTIONS) do
    local nx, ny = x + direction[1], y + direction[2]
    if is_safe(nx, ny) and not is_visited(nx, ny) and not looks_like_a_dead_end(nx, ny, x, y) then
      return direction[3]
    end
  end
  for _, direction in ipairs(DIRECTIONS) do
    local nx, ny = x + direction[1], y + direction[2]
    if is_hazard(nx, ny) and not is_visited(nx, ny) then
      return direction[3]
    end
  end
  for _, direction in ipairs(DIRECTIONS) do
    local nx, ny = x + direction[1], y + direction[2]
    if is_safe(nx, ny) and not is_visited(nx, ny) then
      return direction[3]
    end
  end
  for _, direction in ipairs(DIRECTIONS) do
    local nx, ny = x + direction[1], y + direction[2]
    if is_safe(nx, ny) then
      return direction[3]
    end
  end
  return "scan"
end

-- Heads toward the known uplink, closing whichever axis has the larger
-- remaining distance first; falls back to ordinary exploration if both
-- of the direct moves are blocked.
local function move_closing_x(dx, x, y)
  if dx > 0 and is_safe(x + 1, y) then
    return "east"
  end
  if dx < 0 and is_safe(x - 1, y) then
    return "west"
  end
  return nil
end

local function move_closing_y(dy, x, y)
  if dy > 0 and is_safe(x, y + 1) then
    return "north"
  end
  if dy < 0 and is_safe(x, y - 1) then
    return "south"
  end
  return nil
end

local function home_toward_uplink(x, y)
  local dx, dy = uplink.x - x, uplink.y - y
  local first, second
  if math.abs(dx) >= math.abs(dy) then
    first, second = move_closing_x(dx, x, y), move_closing_y(dy, x, y)
  else
    first, second = move_closing_y(dy, x, y), move_closing_x(dx, x, y)
  end
  return first or second or explore_from(x, y)
end

function on_tick(observation)
  for _, tile in ipairs(observation.discovered) do
    remember(tile)
  end

  local x, y = observation.drone.x, observation.drone.y
  visited[tile_key(x, y)] = true

  local action
  if uplink ~= nil then
    action = home_toward_uplink(x, y)
  elseif not scanned_ahead and moves_taken >= 3 then
    scanned_ahead = true
    action = "scan"
  else
    action = explore_from(x, y)
  end

  if action ~= "scan" and action ~= "wait" then
    moves_taken = moves_taken + 1
  end

  return action
end
