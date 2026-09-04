//! The deterministic drone reconnaissance simulation.
//!
//! This module models the fixed "First Contact" reconnaissance operation: a
//! single captured maintenance drone must cross a small facility map,
//! avoiding walls, to reach a network-uplink objective within a limited
//! number of ticks. All authoritative state transitions live here; this
//! module has no knowledge of Lua, the command line, or terminal output.

use std::collections::{HashSet, VecDeque};
use std::error::Error;
use std::fmt;

/// A grid coordinate on the facility map.
///
/// Coordinates are signed so that a move off the edge of the map is a
/// representable (and rejected) value rather than an unsigned underflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

/// The kind of terrain occupying a single map tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileKind {
    /// Open terrain the drone may freely occupy.
    Floor,
    /// Impassable terrain; movement into a wall tile is rejected.
    Wall,
    /// Traversable terrain. Entering a hazard tile costs additional budget;
    /// see [`HAZARD_ENTRY_COST`].
    Hazard,
}

impl TileKind {
    /// Whether the drone may occupy a tile of this kind.
    pub fn is_traversable(self) -> bool {
        matches!(self, TileKind::Floor | TileKind::Hazard)
    }
}

/// A validated, fixed-size facility map: terrain, a drone starting tile, and
/// a network-uplink objective.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacilityMap {
    width: i32,
    height: i32,
    tiles: Vec<TileKind>,
    drone_start: Position,
    uplink: Position,
}

/// Why a [`FacilityMap`] could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    /// The map's width or height was not positive.
    EmptyDimensions { width: i32, height: i32 },
    /// The tile list's length did not match `width * height`.
    TileCountMismatch { expected: usize, found: usize },
    /// The drone start position is outside the map.
    StartOutOfBounds(Position),
    /// The uplink position is outside the map.
    UplinkOutOfBounds(Position),
    /// The drone start position is not on traversable terrain.
    StartNotTraversable(Position),
    /// The uplink position is not on traversable terrain.
    UplinkNotTraversable(Position),
    /// The drone start and the uplink are the same tile.
    StartIsUplink(Position),
    /// No path of traversable tiles connects the start to the uplink.
    UplinkUnreachable { start: Position, uplink: Position },
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MapError::EmptyDimensions { width, height } => {
                write!(f, "map dimensions must be positive, got {width}x{height}")
            }
            MapError::TileCountMismatch { expected, found } => {
                write!(
                    f,
                    "map needs exactly {expected} tiles for its dimensions, got {found}"
                )
            }
            MapError::StartOutOfBounds(Position { x, y }) => {
                write!(f, "the drone start ({x}, {y}) is outside the map")
            }
            MapError::UplinkOutOfBounds(Position { x, y }) => {
                write!(f, "the uplink ({x}, {y}) is outside the map")
            }
            MapError::StartNotTraversable(Position { x, y }) => {
                write!(f, "the drone start ({x}, {y}) is not a traversable tile")
            }
            MapError::UplinkNotTraversable(Position { x, y }) => {
                write!(f, "the uplink ({x}, {y}) is not a traversable tile")
            }
            MapError::StartIsUplink(Position { x, y }) => {
                write!(
                    f,
                    "the drone start and the uplink are the same tile ({x}, {y})"
                )
            }
            MapError::UplinkUnreachable { start, uplink } => {
                write!(
                    f,
                    "no route of traversable tiles connects the drone start ({}, {}) to the uplink ({}, {})",
                    start.x, start.y, uplink.x, uplink.y
                )
            }
        }
    }
}

impl Error for MapError {}

impl FacilityMap {
    /// Builds a facility map from a flat, row-major tile list, validating
    /// that the map is well-formed: positive dimensions, a tile count
    /// matching those dimensions, an in-bounds and traversable start and
    /// uplink that are not the same tile, and an uplink reachable from the
    /// start through traversable terrain.
    pub fn new(
        width: i32,
        height: i32,
        tiles: Vec<TileKind>,
        drone_start: Position,
        uplink: Position,
    ) -> Result<Self, MapError> {
        if width <= 0 || height <= 0 {
            return Err(MapError::EmptyDimensions { width, height });
        }

        let expected = width as usize * height as usize;
        if tiles.len() != expected {
            return Err(MapError::TileCountMismatch {
                expected,
                found: tiles.len(),
            });
        }

        let index = |position: Position| -> Option<usize> {
            if (0..width).contains(&position.x) && (0..height).contains(&position.y) {
                Some(position.y as usize * width as usize + position.x as usize)
            } else {
                None
            }
        };

        let start_index = index(drone_start).ok_or(MapError::StartOutOfBounds(drone_start))?;
        let uplink_index = index(uplink).ok_or(MapError::UplinkOutOfBounds(uplink))?;

        if !tiles[start_index].is_traversable() {
            return Err(MapError::StartNotTraversable(drone_start));
        }
        if !tiles[uplink_index].is_traversable() {
            return Err(MapError::UplinkNotTraversable(uplink));
        }
        if drone_start == uplink {
            return Err(MapError::StartIsUplink(drone_start));
        }

        if !uplink_is_reachable(width, height, &tiles, drone_start, uplink) {
            return Err(MapError::UplinkUnreachable {
                start: drone_start,
                uplink,
            });
        }

        Ok(FacilityMap {
            width,
            height,
            tiles,
            drone_start,
            uplink,
        })
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn drone_start(&self) -> Position {
        self.drone_start
    }

    pub fn uplink(&self) -> Position {
        self.uplink
    }

    /// The terrain at `position`, or `None` if `position` is outside the
    /// map.
    pub fn tile_at(&self, position: Position) -> Option<TileKind> {
        if (0..self.width).contains(&position.x) && (0..self.height).contains(&position.y) {
            let index = position.y as usize * self.width as usize + position.x as usize;
            Some(self.tiles[index])
        } else {
            None
        }
    }
}

/// The four cardinal neighbours of `position`, regardless of map bounds.
fn cardinal_neighbours(position: Position) -> [Position; 4] {
    [
        Position {
            x: position.x,
            y: position.y + 1,
        },
        Position {
            x: position.x,
            y: position.y - 1,
        },
        Position {
            x: position.x + 1,
            y: position.y,
        },
        Position {
            x: position.x - 1,
            y: position.y,
        },
    ]
}

/// Breadth-first search over cardinal neighbours, true if `uplink` is
/// reachable from `start` through traversable tiles.
fn uplink_is_reachable(
    width: i32,
    height: i32,
    tiles: &[TileKind],
    start: Position,
    uplink: Position,
) -> bool {
    let index = |position: Position| -> usize {
        position.y as usize * width as usize + position.x as usize
    };

    let mut visited = vec![false; tiles.len()];
    visited[index(start)] = true;

    let mut frontier = VecDeque::new();
    frontier.push_back(start);

    while let Some(current) = frontier.pop_front() {
        if current == uplink {
            return true;
        }

        for neighbour in cardinal_neighbours(current) {
            if !(0..width).contains(&neighbour.x) || !(0..height).contains(&neighbour.y) {
                continue;
            }
            let neighbour_index = index(neighbour);
            if visited[neighbour_index] || !tiles[neighbour_index].is_traversable() {
                continue;
            }
            visited[neighbour_index] = true;
            frontier.push_back(neighbour);
        }
    }

    false
}

/// The fixed budget cost of a move or `wait` action.
pub const ACTION_COST: u32 = 1;

/// The fixed budget cost of a `scan` action, in place of [`ACTION_COST`].
/// Priced higher than a move so that scanning continuously is not
/// automatically the optimal strategy, while a single well-timed scan can
/// still be cheaper than the exploration it saves.
pub const SCAN_COST: u32 = 2;

/// The additional budget cost of entering a hazard tile, on top of the
/// action's [`ACTION_COST`]. Charged only on the tick the drone moves onto
/// the hazard tile, not for continuing to occupy or waiting on it.
pub const HAZARD_ENTRY_COST: u32 = 4;

/// The fixed reconnaissance scenario: one drone, one facility map, one
/// operational budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scenario {
    map: FacilityMap,
    starting_budget: u32,
}

/// The fixed "First Contact" reconnaissance facility map, drawn north-up (the first row is
/// `y = height - 1`). `S` is the drone start, `U` is the uplink objective,
/// `.` is floor, `#` is a wall, and `~` is a hazard tile (traversable, but
/// costly to enter). Row `y = 1` is open floor across its whole width, so a
/// route through the hazard (row `y = 1` then column `x = 4`) and a route
/// around it (column `x = 0` then row `y = 4`) are both eight actions long.
/// `(2, 0)` is a single-tile dead end off that corridor (its only floor
/// neighbour is `(2, 1)`), shared by every authored configuration. Passive
/// discovery already reveals it the moment the drone reaches `(2, 1)` — no
/// need to step into it — but reaching `(2, 1)` at all means committing two
/// actions of exploration east before that information exists, and reacting
/// to it by falling back to the known-safe west corridor costs two more
/// backtracking to `(0, 1)`. A single scan taken before ever leaving
/// `(0, 0)` reveals the same area for less — see
/// `scanning_the_dead_end_pocket_saves_the_exploration_a_passive_backtrack_would_cost`.
const FIRST_CONTACT_ROWS: [&str; 5] = [
    "....U", // y = 4
    ".###.", // y = 3
    ".###~", // y = 2
    ".....", // y = 1
    "S#.#.", // y = 0
];

/// An authored "First Contact" variant sharing [`FIRST_CONTACT_ROWS`]'s
/// facility topology, but with the uplink relocated to the south end of
/// column `x=4` (`(4, 0)`) rather than either end of the shared row-`y=1`/
/// row-`y=4` corridor. Unlike [`FIRST_CONTACT_WEST_HAZARD_ROWS`], reaching
/// this uplink requires actually turning off that corridor onto column
/// `x=4`'s south spur, so neither of the two eight-action routes that solve
/// [`FIRST_CONTACT_ROWS`] (nor the boundary-walking sequence that chains
/// them into one longer blind route) reaches it: see
/// `no_single_blind_route_solves_every_authored_first_contact_configuration`.
/// The hazard stays at the original `(4, 2)`, on the direct route down that
/// spur, so a blind sequence built to visit every tile still runs out of
/// budget there before ever reaching `(4, 0)`.
const FIRST_CONTACT_SOUTH_UPLINK_ROWS: [&str; 5] = [
    ".....", // y = 4
    ".###.", // y = 3
    ".###~", // y = 2
    ".....", // y = 1
    "S#.#U", // y = 0
];

/// An authored "First Contact" variant sharing [`FIRST_CONTACT_ROWS`]'s
/// facility topology and uplink position, but with the hazard moved from
/// `(4, 2)` to `(4, 1)` — the single gateway tile row `y=1` and column
/// `x=4` share with [`FIRST_CONTACT_SOUTH_UPLINK_ROWS`]'s south spur. This
/// still flips which of the two original eight-action routes is the risky
/// one, exactly as a hazard moved to `(0, 2)` would, but a script that
/// detours onto that spur and back (as one would to also solve the
/// south-uplink configuration) crosses this tile twice, not once: see
/// `no_single_blind_movement_sequence_solves_every_authored_first_contact_configuration`.
const FIRST_CONTACT_ROW1_HAZARD_ROWS: [&str; 5] = [
    "....U", // y = 4
    ".###.", // y = 3
    ".###.", // y = 2
    "....~", // y = 1
    "S#.#.", // y = 0
];

/// Parses a north-up ASCII map (rows given top-down) into a row-major,
/// bottom-up tile list matching [`FacilityMap`]'s coordinate system.
fn tiles_from_rows(rows: &[&str]) -> Vec<TileKind> {
    rows.iter()
        .rev()
        .flat_map(|row| row.chars())
        .map(|glyph| match glyph {
            '.' | 'S' | 'U' => TileKind::Floor,
            '#' => TileKind::Wall,
            '~' => TileKind::Hazard,
            other => panic!("unrecognized facility map glyph: {other:?}"),
        })
        .collect()
}

impl Scenario {
    /// Builds a scenario from a facility map and a starting operational
    /// budget.
    pub fn new(map: FacilityMap, starting_budget: u32) -> Self {
        Scenario {
            map,
            starting_budget,
        }
    }

    /// The original "First Contact" reconnaissance scenario: uplink at
    /// `(4, 4)`, hazard at `(4, 2)`.
    pub fn first_contact() -> Self {
        let map = FacilityMap::new(
            5,
            5,
            tiles_from_rows(&FIRST_CONTACT_ROWS),
            Position { x: 0, y: 0 },
            Position { x: 4, y: 4 },
        )
        .expect("the fixed first contact facility map is valid");

        Scenario::new(map, 15)
    }

    /// An authored "First Contact" variant: uplink at `(4, 0)`, hazard at
    /// `(4, 2)`. See [`FIRST_CONTACT_SOUTH_UPLINK_ROWS`].
    pub fn first_contact_south_uplink() -> Self {
        let map = FacilityMap::new(
            5,
            5,
            tiles_from_rows(&FIRST_CONTACT_SOUTH_UPLINK_ROWS),
            Position { x: 0, y: 0 },
            Position { x: 4, y: 0 },
        )
        .expect("the first-contact-south-uplink facility map is valid");

        Scenario::new(map, 15)
    }

    /// An authored "First Contact" variant: uplink at `(4, 4)` (as in
    /// [`Scenario::first_contact`]), hazard at `(4, 1)`. See
    /// [`FIRST_CONTACT_ROW1_HAZARD_ROWS`].
    pub fn first_contact_row1_hazard() -> Self {
        let map = FacilityMap::new(
            5,
            5,
            tiles_from_rows(&FIRST_CONTACT_ROW1_HAZARD_ROWS),
            Position { x: 0, y: 0 },
            Position { x: 4, y: 4 },
        )
        .expect("the first-contact-row1-hazard facility map is valid");

        Scenario::new(map, 15)
    }

    /// The small, hand-authored set of "First Contact" configurations a
    /// deployment may be run against (`docs/TUI_DESIGN.md`, "First Contact
    /// configuration model"). Not procedurally generated: each entry is one
    /// of the fixed constructors above, sharing the same facility topology
    /// and varying only the active uplink and hazard placement.
    ///
    /// No single blind movement sequence — of any length, including one
    /// that revisits ground already covered — solves every entry within
    /// the shared starting budget. This is exhaustively proven, not just
    /// checked against a couple of hand-picked routes, by
    /// `no_single_blind_movement_sequence_solves_every_authored_first_contact_configuration`,
    /// which explores the full reachable action-sequence space. The
    /// guarantee depends on [`Scenario::first_contact_row1_hazard`]'s
    /// hazard sitting exactly on the gateway a detour to
    /// [`Scenario::first_contact_south_uplink`]'s spur must cross twice
    /// (there and back) — moving either configuration's hazard elsewhere
    /// can reopen a universal blind solver, so treat these placements, not
    /// just the general "one hazard per configuration" shape, as load-
    /// bearing.
    fn first_contact_configurations() -> [Scenario; 3] {
        [
            Scenario::first_contact(),
            Scenario::first_contact_south_uplink(),
            Scenario::first_contact_row1_hazard(),
        ]
    }

    /// Deterministically selects one of [`Scenario::first_contact_configurations`]
    /// for the deployment identified by `run_id`, with no runtime
    /// randomness. `run_id`s are assigned starting at 1 and increase by one
    /// per deployment, so the very first deployment of a session always
    /// selects index 0 (`Scenario::first_contact()`), and later
    /// redeployments cycle deterministically through the rest of the set.
    /// The same `run_id` always selects an equal `Scenario`.
    pub fn select_first_contact(run_id: u32) -> Self {
        let configurations = Scenario::first_contact_configurations();
        let index = run_id.saturating_sub(1) as usize % configurations.len();
        configurations[index].clone()
    }

    pub fn map(&self) -> &FacilityMap {
        &self.map
    }

    pub fn starting_budget(&self) -> u32 {
        self.starting_budget
    }

    pub fn drone_start(&self) -> Position {
        self.map.drone_start()
    }

    pub fn uplink(&self) -> Position {
        self.map.uplink()
    }

    pub fn tile_at(&self, position: Position) -> Option<TileKind> {
        self.map.tile_at(position)
    }
}

/// An action a controller may submit for a single tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    MoveNorth,
    MoveSouth,
    MoveEast,
    MoveWest,
    Wait,
    /// Reveals a documented, bounded area around the drone without moving
    /// it. See [`Simulation::step`] for the exact area revealed.
    Scan,
}

/// The operation's state as of the most recent tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickOutcome {
    Running,
    Succeeded,
    Failed(FailureReason),
}

/// Why an operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureReason {
    BudgetExhausted,
}

/// A structured record of something that happened as a result of one
/// [`Simulation::step`] call, so callers can present exactly what occurred
/// without re-deriving cost or outcome rules from raw state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimEvent {
    /// The documented base cost of the action just taken.
    ActionCost { action: Action, amount: u32 },
    /// The drone moved onto a hazard tile this tick, incurring an
    /// additional cost on top of the action's base cost.
    HazardEntered { position: Position, amount: u32 },
    /// The drone reached the uplink. This is checked, and takes
    /// precedence, before budget-exhaustion failure is applied, so it can
    /// still occur on the same action that brings the budget to zero.
    OperationSucceeded,
    /// The budget was exhausted before the uplink was reached.
    BudgetExhausted,
}

/// The outcome and structured events produced by a single
/// [`Simulation::step`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepReport {
    pub outcome: TickOutcome,
    pub events: Vec<SimEvent>,
}

/// An action that was rejected without mutating simulation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionError {
    /// The operation has already succeeded or failed; no further actions
    /// are accepted.
    SimulationEnded,
    /// The requested move would leave the facility map.
    OutOfBounds,
    /// The requested move would run the drone into a wall.
    BlockedByWall,
}

impl fmt::Display for ActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ActionError::SimulationEnded => {
                write!(
                    f,
                    "the operation has already ended; no further actions accepted"
                )
            }
            ActionError::OutOfBounds => {
                write!(f, "that move would leave the bounded facility area")
            }
            ActionError::BlockedByWall => {
                write!(f, "that move would run the drone into a wall")
            }
        }
    }
}

impl Error for ActionError {}

/// A single tile a controller has learned about, either through passive
/// local vision or a completed [`Action::Scan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveredTile {
    pub position: Position,
    pub kind: TileKind,
    pub is_traversable: bool,
    pub is_uplink: bool,
}

/// A read-only snapshot of observable state for a single tick, handed to a
/// controller instead of letting it combine [`Simulation`] getters with
/// scenario details itself.
///
/// `discovered` is cumulative: it contains every tile the drone has learned
/// about so far, not just this tick's surroundings. Tiles the drone has
/// never been near and never scanned are simply absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub drone_position: Position,
    pub tick: u32,
    pub budget_remaining: u32,
    pub discovered: Vec<DiscoveredTile>,
}

/// The authoritative, deterministic state of a reconnaissance operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Simulation {
    scenario: Scenario,
    drone_position: Position,
    ticks_elapsed: u32,
    budget_remaining: u32,
    outcome: TickOutcome,
    discovered: HashSet<Position>,
}

impl Simulation {
    /// A test/convenience constructor for the fixed "First Contact"
    /// scenario. Production callers select a [`Scenario`] explicitly and
    /// use [`Simulation::from_scenario`] instead, so that each deployment
    /// can carry its own configuration rather than assuming this one
    /// global default.
    ///
    /// A new simulation always starts from the same state.
    pub fn new() -> Self {
        Self::from_scenario(Scenario::first_contact())
    }

    /// Starts a new simulation of the given `scenario`.
    pub fn from_scenario(scenario: Scenario) -> Self {
        let drone_position = scenario.drone_start();
        let budget_remaining = scenario.starting_budget();
        let mut simulation = Simulation {
            scenario,
            drone_position,
            ticks_elapsed: 0,
            budget_remaining,
            outcome: TickOutcome::Running,
            discovered: HashSet::new(),
        };
        simulation.reveal_local();
        simulation
    }

    pub fn drone_position(&self) -> Position {
        self.drone_position
    }

    pub fn ticks_elapsed(&self) -> u32 {
        self.ticks_elapsed
    }

    pub fn outcome(&self) -> TickOutcome {
        self.outcome
    }

    /// The scenario this simulation was started from.
    pub fn scenario(&self) -> &Scenario {
        &self.scenario
    }

    /// The facility map this operation is being run against.
    pub fn map(&self) -> &FacilityMap {
        self.scenario.map()
    }

    /// Returns a read-only snapshot of the current tick's observable state.
    pub fn observe(&self) -> Observation {
        let mut discovered: Vec<DiscoveredTile> = self
            .discovered
            .iter()
            .map(|&position| {
                let kind = self
                    .scenario
                    .tile_at(position)
                    .expect("discovered tiles are always in bounds");
                DiscoveredTile {
                    position,
                    kind,
                    is_traversable: kind.is_traversable(),
                    is_uplink: position == self.scenario.uplink(),
                }
            })
            .collect();
        // `HashSet` iteration order is not stable across runs, so sort for
        // determinism.
        discovered.sort_by_key(|tile| (tile.position.y, tile.position.x));

        Observation {
            drone_position: self.drone_position,
            tick: self.ticks_elapsed,
            budget_remaining: self.budget_remaining,
            discovered,
        }
    }

    /// Submits one action for this tick and advances the simulation by one
    /// tick, returning the resulting outcome and the structured events it
    /// produced.
    ///
    /// If the action is invalid or impossible, or the operation has already
    /// ended, this returns an error and leaves state unchanged.
    pub fn step(&mut self, action: Action) -> Result<StepReport, ActionError> {
        if self.outcome != TickOutcome::Running {
            return Err(ActionError::SimulationEnded);
        }

        let next_position = self.apply(action)?;
        let entered_hazard = next_position != self.drone_position
            && self.scenario.tile_at(next_position) == Some(TileKind::Hazard);

        self.drone_position = next_position;
        self.ticks_elapsed += 1;
        self.reveal_local();
        if action == Action::Scan {
            self.reveal_scan();
        }

        let action_cost = if action == Action::Scan {
            SCAN_COST
        } else {
            ACTION_COST
        };
        let cost = action_cost + if entered_hazard { HAZARD_ENTRY_COST } else { 0 };
        self.budget_remaining = self.budget_remaining.saturating_sub(cost);

        let mut events = vec![SimEvent::ActionCost {
            action,
            amount: action_cost,
        }];
        if entered_hazard {
            events.push(SimEvent::HazardEntered {
                position: next_position,
                amount: HAZARD_ENTRY_COST,
            });
        }

        self.outcome = if self.drone_position == self.scenario.uplink() {
            TickOutcome::Succeeded
        } else if self.budget_remaining == 0 {
            TickOutcome::Failed(FailureReason::BudgetExhausted)
        } else {
            TickOutcome::Running
        };

        match self.outcome {
            TickOutcome::Succeeded => events.push(SimEvent::OperationSucceeded),
            TickOutcome::Failed(FailureReason::BudgetExhausted) => {
                events.push(SimEvent::BudgetExhausted)
            }
            TickOutcome::Running => {}
        }

        Ok(StepReport {
            outcome: self.outcome,
            events,
        })
    }

    /// Computes the position `action` would produce without mutating state,
    /// rejecting moves that would leave the facility map or enter an
    /// impassable tile.
    fn apply(&self, action: Action) -> Result<Position, ActionError> {
        let Position { x, y } = self.drone_position;
        let candidate = match action {
            Action::MoveNorth => Position { x, y: y + 1 },
            Action::MoveSouth => Position { x, y: y - 1 },
            Action::MoveEast => Position { x: x + 1, y },
            Action::MoveWest => Position { x: x - 1, y },
            Action::Wait | Action::Scan => Position { x, y },
        };

        match self.scenario.tile_at(candidate) {
            None => Err(ActionError::OutOfBounds),
            Some(tile) if tile.is_traversable() => Ok(candidate),
            Some(_) => Err(ActionError::BlockedByWall),
        }
    }

    /// Marks the drone's current tile and its in-bounds cardinal neighbours
    /// as discovered. Applied every tick so passive local vision is always
    /// up to date with the drone's position.
    fn reveal_local(&mut self) {
        let position = self.drone_position;
        self.discovered.insert(position);
        for neighbour in cardinal_neighbours(position) {
            if self.scenario.tile_at(neighbour).is_some() {
                self.discovered.insert(neighbour);
            }
        }
    }

    /// Marks every in-bounds tile within Chebyshev distance 2 of the drone
    /// as discovered. Walls do not block this: the whole documented area is
    /// always revealed.
    fn reveal_scan(&mut self) {
        const SCAN_RADIUS: i32 = 2;
        let center = self.drone_position;
        for dy in -SCAN_RADIUS..=SCAN_RADIUS {
            for dx in -SCAN_RADIUS..=SCAN_RADIUS {
                let position = Position {
                    x: center.x + dx,
                    y: center.y + dy,
                };
                if self.scenario.tile_at(position).is_some() {
                    self.discovered.insert(position);
                }
            }
        }
    }
}

impl Default for Simulation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small, fully open 3x3 map with no walls or hazards, for tests that
    /// only care about movement/budget/observation behavior and would
    /// otherwise be entangled with the fixed map's wall layout.
    fn open_scenario(starting_budget: u32) -> Scenario {
        let map = FacilityMap::new(
            3,
            3,
            vec![TileKind::Floor; 9],
            Position { x: 0, y: 0 },
            Position { x: 2, y: 2 },
        )
        .unwrap();
        Scenario::new(map, starting_budget)
    }

    /// A small map with a wall directly north of the start and a hazard on
    /// the only route to the uplink, for wall/hazard-specific tests.
    fn walled_scenario() -> Scenario {
        walled_scenario_with_budget(20)
    }

    fn walled_scenario_with_budget(starting_budget: u32) -> Scenario {
        // y=2  U  .  .
        // y=1  #  #  .
        // y=0  S  .  ~
        let rows = ["U..", "##.", "S.~"];
        let map = FacilityMap::new(
            3,
            3,
            tiles_from_rows(&rows),
            Position { x: 0, y: 0 },
            Position { x: 0, y: 2 },
        )
        .unwrap();
        Scenario::new(map, starting_budget)
    }

    #[test]
    fn a_valid_map_is_constructed_with_its_terrain_and_locations() {
        let map = FacilityMap::new(
            2,
            1,
            vec![TileKind::Floor, TileKind::Floor],
            Position { x: 0, y: 0 },
            Position { x: 1, y: 0 },
        )
        .unwrap();

        assert_eq!(map.width(), 2);
        assert_eq!(map.height(), 1);
        assert_eq!(map.drone_start(), Position { x: 0, y: 0 });
        assert_eq!(map.uplink(), Position { x: 1, y: 0 });
        assert_eq!(map.tile_at(Position { x: 0, y: 0 }), Some(TileKind::Floor));
    }

    #[test]
    fn a_map_with_nonpositive_dimensions_is_rejected() {
        let result = FacilityMap::new(
            0,
            3,
            vec![],
            Position { x: 0, y: 0 },
            Position { x: 0, y: 1 },
        );

        assert_eq!(
            result,
            Err(MapError::EmptyDimensions {
                width: 0,
                height: 3
            })
        );
    }

    #[test]
    fn a_map_with_the_wrong_tile_count_is_rejected() {
        let result = FacilityMap::new(
            2,
            2,
            vec![TileKind::Floor; 3],
            Position { x: 0, y: 0 },
            Position { x: 1, y: 1 },
        );

        assert_eq!(
            result,
            Err(MapError::TileCountMismatch {
                expected: 4,
                found: 3,
            })
        );
    }

    #[test]
    fn a_start_outside_the_map_is_rejected() {
        let result = FacilityMap::new(
            2,
            2,
            vec![TileKind::Floor; 4],
            Position { x: 5, y: 5 },
            Position { x: 1, y: 1 },
        );

        assert_eq!(
            result,
            Err(MapError::StartOutOfBounds(Position { x: 5, y: 5 }))
        );
    }

    #[test]
    fn an_uplink_outside_the_map_is_rejected() {
        let result = FacilityMap::new(
            2,
            2,
            vec![TileKind::Floor; 4],
            Position { x: 0, y: 0 },
            Position { x: 5, y: 5 },
        );

        assert_eq!(
            result,
            Err(MapError::UplinkOutOfBounds(Position { x: 5, y: 5 }))
        );
    }

    #[test]
    fn a_start_on_a_wall_is_rejected() {
        let tiles = vec![
            TileKind::Wall,
            TileKind::Floor,
            TileKind::Floor,
            TileKind::Floor,
        ];
        let result = FacilityMap::new(
            2,
            2,
            tiles,
            Position { x: 0, y: 0 },
            Position { x: 1, y: 1 },
        );

        assert_eq!(
            result,
            Err(MapError::StartNotTraversable(Position { x: 0, y: 0 }))
        );
    }

    #[test]
    fn an_uplink_on_a_wall_is_rejected() {
        let tiles = vec![
            TileKind::Floor,
            TileKind::Floor,
            TileKind::Floor,
            TileKind::Wall,
        ];
        let result = FacilityMap::new(
            2,
            2,
            tiles,
            Position { x: 0, y: 0 },
            Position { x: 1, y: 1 },
        );

        assert_eq!(
            result,
            Err(MapError::UplinkNotTraversable(Position { x: 1, y: 1 }))
        );
    }

    #[test]
    fn a_start_that_is_also_the_uplink_is_rejected() {
        let result = FacilityMap::new(
            2,
            2,
            vec![TileKind::Floor; 4],
            Position { x: 0, y: 0 },
            Position { x: 0, y: 0 },
        );

        assert_eq!(
            result,
            Err(MapError::StartIsUplink(Position { x: 0, y: 0 }))
        );
    }

    #[test]
    fn an_unreachable_uplink_is_rejected() {
        // S # U
        let tiles = vec![TileKind::Floor, TileKind::Wall, TileKind::Floor];
        let result = FacilityMap::new(
            3,
            1,
            tiles,
            Position { x: 0, y: 0 },
            Position { x: 2, y: 0 },
        );

        assert_eq!(
            result,
            Err(MapError::UplinkUnreachable {
                start: Position { x: 0, y: 0 },
                uplink: Position { x: 2, y: 0 },
            })
        );
    }

    #[test]
    fn an_uplink_reachable_only_around_walls_is_accepted() {
        // U  .  .
        // #  #  .
        // S  .  .
        let rows = ["U..", "##.", "S.."];
        let map = FacilityMap::new(
            3,
            3,
            tiles_from_rows(&rows),
            Position { x: 0, y: 0 },
            Position { x: 0, y: 2 },
        );

        assert!(map.is_ok());
    }

    #[test]
    fn a_start_on_a_hazard_is_accepted() {
        let tiles = vec![TileKind::Hazard, TileKind::Floor];
        let map = FacilityMap::new(
            2,
            1,
            tiles,
            Position { x: 0, y: 0 },
            Position { x: 1, y: 0 },
        );

        assert!(map.is_ok());
    }

    #[test]
    fn map_errors_describe_the_problem() {
        assert_eq!(
            MapError::StartOutOfBounds(Position { x: 5, y: 5 }).to_string(),
            "the drone start (5, 5) is outside the map"
        );
        assert_eq!(
            MapError::UplinkUnreachable {
                start: Position { x: 0, y: 0 },
                uplink: Position { x: 1, y: 1 },
            }
            .to_string(),
            "no route of traversable tiles connects the drone start (0, 0) to the uplink (1, 1)"
        );
    }

    #[test]
    fn tile_at_reports_terrain_inside_the_map() {
        let scenario = walled_scenario();
        assert_eq!(
            scenario.tile_at(Position { x: 0, y: 1 }),
            Some(TileKind::Wall)
        );
        assert_eq!(
            scenario.tile_at(Position { x: 2, y: 0 }),
            Some(TileKind::Hazard)
        );
    }

    #[test]
    fn tile_at_reports_none_outside_the_map() {
        let scenario = walled_scenario();
        assert_eq!(scenario.tile_at(Position { x: -1, y: 0 }), None);
        assert_eq!(scenario.tile_at(Position { x: 0, y: -1 }), None);
        assert_eq!(scenario.tile_at(Position { x: 3, y: 0 }), None);
        assert_eq!(scenario.tile_at(Position { x: 0, y: 3 }), None);
    }

    #[test]
    fn walls_are_impassable_and_floor_and_hazard_are_traversable() {
        assert!(TileKind::Floor.is_traversable());
        assert!(TileKind::Hazard.is_traversable());
        assert!(!TileKind::Wall.is_traversable());
    }

    #[test]
    fn first_contact_map_has_the_fixed_layout() {
        let scenario = Scenario::first_contact();
        let map = scenario.map();

        assert_eq!(map.width(), 5);
        assert_eq!(map.height(), 5);
        assert_eq!(map.drone_start(), Position { x: 0, y: 0 });
        assert_eq!(map.uplink(), Position { x: 4, y: 4 });

        // Asymmetric spot checks so an inverted or mistranscribed row order
        // cannot pass.
        assert_eq!(map.tile_at(Position { x: 0, y: 4 }), Some(TileKind::Floor));
        assert_eq!(map.tile_at(Position { x: 4, y: 0 }), Some(TileKind::Floor));
        assert_eq!(map.tile_at(Position { x: 4, y: 2 }), Some(TileKind::Hazard));
        assert_eq!(map.tile_at(Position { x: 2, y: 2 }), Some(TileKind::Wall));
        assert_eq!(map.tile_at(Position { x: 1, y: 0 }), Some(TileKind::Wall));
        // Row y=1 is the open corridor that lets a controller reach the
        // hazard via an alternate route instead of only the safe one.
        assert_eq!(map.tile_at(Position { x: 2, y: 1 }), Some(TileKind::Floor));
        // The dead-end scan-value pocket, isolated from the walls on
        // either side of it.
        assert_eq!(map.tile_at(Position { x: 2, y: 0 }), Some(TileKind::Floor));
        assert_eq!(map.tile_at(Position { x: 3, y: 0 }), Some(TileKind::Wall));
    }

    #[test]
    fn first_contact_scenario_is_identical_on_every_construction() {
        assert_eq!(Scenario::first_contact(), Scenario::first_contact());
    }

    #[test]
    fn every_authored_first_contact_configuration_has_a_reachable_uplink() {
        // `FacilityMap::new` already proves reachability at construction
        // time (it would have panicked via `.expect(...)` otherwise), but
        // this test names the guarantee explicitly per each configuration
        // rather than relying on that panic alone.
        for scenario in Scenario::first_contact_configurations() {
            assert!(scenario.map().tile_at(scenario.uplink()).is_some());
        }
    }

    #[test]
    fn select_first_contact_is_a_pure_function_of_run_id() {
        assert_eq!(
            Scenario::select_first_contact(1),
            Scenario::select_first_contact(1)
        );
        assert_eq!(Scenario::select_first_contact(1), Scenario::first_contact());
        assert_eq!(Scenario::select_first_contact(4), Scenario::first_contact());

        // Distinct configurations are actually selected as run ids advance,
        // not just re-selecting the same one under a different id.
        let selections = [
            Scenario::select_first_contact(1),
            Scenario::select_first_contact(2),
            Scenario::select_first_contact(3),
        ];
        assert_ne!(selections[0], selections[1]);
        assert_ne!(selections[1], selections[2]);
        assert_ne!(selections[0], selections[2]);
    }

    #[test]
    fn two_natural_blind_routes_each_fail_at_least_one_authored_configuration() {
        // A quick, human-readable illustration of the property the
        // exhaustive `no_single_blind_movement_sequence_solves_every_authored_first_contact_configuration`
        // below actually proves: the two obvious "replay what solved my
        // last run" routes for `Scenario::first_contact()` each fail
        // against at least one of the other authored configurations.
        fn outcomes_for(route: &[Action]) -> Vec<TickOutcome> {
            Scenario::first_contact_configurations()
                .into_iter()
                .map(|scenario| {
                    let mut sim = Simulation::from_scenario(scenario);
                    for &action in route {
                        if sim.step(action).is_err() || sim.outcome() != TickOutcome::Running {
                            break;
                        }
                    }
                    sim.outcome()
                })
                .collect()
        }

        // Row y=1 across, then north up column x=4:
        // `tests/fixtures/hazard_route.lua` plays exactly this sequence.
        let row_then_column_route = [
            Action::MoveNorth,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
        ];
        // The mirror route: column x=0 up, then east across row y=4. Both
        // routes solve `Scenario::first_contact()`.
        let column_then_row_route = [
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
        ];

        for route in [row_then_column_route, column_then_row_route] {
            let outcomes = outcomes_for(&route);
            assert!(
                outcomes.contains(&TickOutcome::Succeeded),
                "{route:?} should still solve at least one authored configuration"
            );
            assert!(
                !outcomes
                    .iter()
                    .all(|outcome| *outcome == TickOutcome::Succeeded),
                "no single blind route may be guaranteed to solve every authored \
                 configuration, but {route:?} solved all of {outcomes:?}"
            );
        }
    }

    /// A simplified, hashable summary of one [`Simulation`]'s state: enough
    /// to tell whether two independent action-sequence prefixes have left a
    /// configuration in an equivalent position, budget, and outcome,
    /// without dragging the (unbounded, order-independent) `discovered` set
    /// into the equality/hash used for search memoization below.
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
    struct BlindSearchState {
        position: Position,
        budget_remaining: u32,
        outcome: BlindOutcome,
    }

    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
    enum BlindOutcome {
        Running,
        Succeeded,
        Failed,
    }

    impl From<TickOutcome> for BlindOutcome {
        fn from(outcome: TickOutcome) -> Self {
            match outcome {
                TickOutcome::Running => BlindOutcome::Running,
                TickOutcome::Succeeded => BlindOutcome::Succeeded,
                TickOutcome::Failed(_) => BlindOutcome::Failed,
            }
        }
    }

    fn blind_search_key(sims: &[Simulation]) -> Vec<BlindSearchState> {
        sims.iter()
            .map(|sim| BlindSearchState {
                position: sim.drone_position(),
                budget_remaining: sim.observe().budget_remaining,
                outcome: sim.outcome().into(),
            })
            .collect()
    }

    /// Exhaustively searches for a single blind (non-reactive, fixed in
    /// advance) movement sequence that reaches every one of `configs`'
    /// uplinks — i.e. a "solved one run, replay the exact moves" strategy
    /// that would work no matter which authored configuration got
    /// selected. Returns the sequence if one exists, `None` if none does.
    ///
    /// The search space is only the four movement actions: `wait` and
    /// `scan` never help a non-reactive sequence reach a not-yet-succeeded
    /// uplink sooner or more cheaply (they cost the same as a move without
    /// changing position), so a solver using them, if one existed, would
    /// still exist with them stripped out. This is a breadth-first search
    /// over joint simulation states (one per configuration, all stepped
    /// with the same action), memoized on [`BlindSearchState`] so that two
    /// different prefixes reaching an equivalent position/budget/outcome
    /// tuple for every configuration are only explored once. Movement
    /// always costs at least 1 budget for a configuration still `Running`,
    /// and every configuration shares the same starting budget, so the
    /// search terminates in a bounded number of steps: by the time as many
    /// actions have been taken as the starting budget allows, every
    /// configuration must already be terminal (succeeded or failed).
    fn universal_blind_movement_solver(configs: &[Scenario]) -> Option<Vec<Action>> {
        let moves = [
            Action::MoveNorth,
            Action::MoveSouth,
            Action::MoveEast,
            Action::MoveWest,
        ];

        let start: Vec<Simulation> = configs
            .iter()
            .cloned()
            .map(Simulation::from_scenario)
            .collect();
        let mut visited = HashSet::new();
        visited.insert(blind_search_key(&start));
        let mut queue = VecDeque::new();
        queue.push_back((start, Vec::new()));

        while let Some((sims, path)) = queue.pop_front() {
            if sims
                .iter()
                .all(|sim| sim.outcome() == TickOutcome::Succeeded)
            {
                return Some(path);
            }
            // Every configuration is terminal but not every one succeeded:
            // nothing further can change any of their fixed outcomes.
            if sims.iter().all(|sim| sim.outcome() != TickOutcome::Running) {
                continue;
            }

            for &action in &moves {
                let mut next = sims.clone();
                for sim in &mut next {
                    if sim.outcome() == TickOutcome::Running {
                        // An invalid move (wall/out-of-bounds) is a no-op
                        // for that configuration, exactly as it would be
                        // for a real deployment; the search just carries
                        // its unchanged state forward.
                        let _ = sim.step(action);
                    }
                }
                let key = blind_search_key(&next);
                if visited.insert(key) {
                    let mut next_path = path.clone();
                    next_path.push(action);
                    queue.push_back((next, next_path));
                }
            }
        }

        None
    }

    #[test]
    fn no_single_blind_movement_sequence_solves_every_authored_first_contact_configuration() {
        assert_eq!(
            universal_blind_movement_solver(&Scenario::first_contact_configurations()),
            None,
            "a single blind movement sequence must not be able to reach every \
             authored configuration's uplink"
        );
    }

    #[test]
    fn the_blind_search_itself_can_find_a_universal_solver_when_one_exists() {
        // Sanity check on `universal_blind_movement_solver` itself: three
        // configurations that all share the same uplink are trivially
        // solved by whatever blind route solves `Scenario::first_contact()`
        // alone, so the search must find one rather than vacuously
        // returning `None` because a real solver happens not to exist for
        // the actual authored set.
        let identical = [
            Scenario::first_contact(),
            Scenario::first_contact(),
            Scenario::first_contact(),
        ];
        assert!(universal_blind_movement_solver(&identical).is_some());
    }

    #[test]
    fn new_simulation_has_fixed_starting_state() {
        let sim = Simulation::new();
        let scenario = Scenario::first_contact();

        assert_eq!(sim.drone_position(), scenario.drone_start());
        assert_eq!(sim.ticks_elapsed(), 0);
        assert_eq!(sim.outcome(), TickOutcome::Running);
        assert_eq!(sim.observe().budget_remaining, scenario.starting_budget());
    }

    #[test]
    fn identical_action_sequences_produce_identical_states() {
        let actions = [
            Action::MoveNorth,
            Action::MoveEast,
            Action::Wait,
            Action::MoveNorth,
            Action::MoveEast,
        ];

        let mut a = Simulation::from_scenario(open_scenario(20));
        let mut b = Simulation::from_scenario(open_scenario(20));

        for action in actions {
            let outcome_a = a.step(action);
            let outcome_b = b.step(action);

            assert_eq!(outcome_a, outcome_b);
            assert_eq!(a.drone_position(), b.drone_position());
            assert_eq!(a.ticks_elapsed(), b.ticks_elapsed());
            assert_eq!(a.outcome(), b.outcome());
        }
    }

    #[test]
    fn identical_action_sequences_produce_identical_states_on_the_fixed_map() {
        let actions = [
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveEast,
        ];

        let mut a = Simulation::new();
        let mut b = Simulation::new();

        for action in actions {
            assert_eq!(a.step(action), b.step(action));
            assert_eq!(a.drone_position(), b.drone_position());
        }
    }

    #[test]
    fn move_north_updates_position() {
        let mut sim = Simulation::from_scenario(open_scenario(20));
        sim.step(Action::MoveNorth).unwrap();
        assert_eq!(sim.drone_position(), Position { x: 0, y: 1 });
    }

    #[test]
    fn move_south_updates_position() {
        let mut sim = Simulation::from_scenario(open_scenario(20));
        sim.step(Action::MoveNorth).unwrap();
        sim.step(Action::MoveSouth).unwrap();
        assert_eq!(sim.drone_position(), Position { x: 0, y: 0 });
    }

    #[test]
    fn move_east_updates_position() {
        let mut sim = Simulation::from_scenario(open_scenario(20));
        sim.step(Action::MoveEast).unwrap();
        assert_eq!(sim.drone_position(), Position { x: 1, y: 0 });
    }

    #[test]
    fn move_west_updates_position() {
        let mut sim = Simulation::from_scenario(open_scenario(20));
        sim.step(Action::MoveEast).unwrap();
        sim.step(Action::MoveWest).unwrap();
        assert_eq!(sim.drone_position(), Position { x: 0, y: 0 });
    }

    #[test]
    fn wait_does_not_change_position_but_advances_tick() {
        let mut sim = Simulation::from_scenario(open_scenario(20));
        sim.step(Action::Wait).unwrap();
        assert_eq!(sim.drone_position(), Position { x: 0, y: 0 });
        assert_eq!(sim.ticks_elapsed(), 1);
    }

    #[test]
    fn valid_route_to_uplink_succeeds() {
        let mut sim = Simulation::from_scenario(open_scenario(20));

        assert_eq!(
            sim.step(Action::MoveNorth).unwrap().outcome,
            TickOutcome::Running
        );
        assert_eq!(
            sim.step(Action::MoveNorth).unwrap().outcome,
            TickOutcome::Running
        );
        assert_eq!(
            sim.step(Action::MoveEast).unwrap().outcome,
            TickOutcome::Running
        );
        let outcome = sim.step(Action::MoveEast).unwrap().outcome;

        assert_eq!(outcome, TickOutcome::Succeeded);
        assert_eq!(sim.drone_position(), Position { x: 2, y: 2 });
    }

    #[test]
    fn the_fixed_route_reaches_the_uplink() {
        let mut sim = Simulation::new();
        let route = [
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
        ];

        let mut outcome = TickOutcome::Running;
        for action in route {
            outcome = sim.step(action).unwrap().outcome;
        }

        assert_eq!(outcome, TickOutcome::Succeeded);
        assert_eq!(sim.drone_position(), Position { x: 4, y: 4 });
        assert_eq!(sim.ticks_elapsed(), 8);
        assert_eq!(sim.observe().budget_remaining, 15 - 8);
    }

    #[test]
    fn moving_into_the_wall_block_is_rejected() {
        let mut sim = Simulation::new();

        let result = sim.step(Action::MoveEast);

        assert_eq!(result, Err(ActionError::BlockedByWall));
    }

    #[test]
    fn waiting_until_budget_exhausted_fails() {
        let mut sim = Simulation::from_scenario(open_scenario(3));

        assert_eq!(
            sim.step(Action::Wait).unwrap().outcome,
            TickOutcome::Running
        );
        assert_eq!(sim.observe().budget_remaining, 2);
        assert_eq!(
            sim.step(Action::Wait).unwrap().outcome,
            TickOutcome::Running
        );
        assert_eq!(sim.observe().budget_remaining, 1);
        let report = sim.step(Action::Wait).unwrap();

        assert_eq!(
            report.outcome,
            TickOutcome::Failed(FailureReason::BudgetExhausted)
        );
        assert_eq!(sim.observe().budget_remaining, 0);
    }

    #[test]
    fn out_of_bounds_move_is_rejected_without_mutating_state() {
        let mut sim = Simulation::from_scenario(open_scenario(20));
        let before = sim.clone();

        let result = sim.step(Action::MoveSouth);

        assert_eq!(result, Err(ActionError::OutOfBounds));
        assert_eq!(sim, before);
    }

    #[test]
    fn a_move_into_a_wall_is_rejected_without_mutating_state() {
        let mut sim = Simulation::from_scenario(walled_scenario());
        let before = sim.clone();

        let result = sim.step(Action::MoveNorth);

        assert_eq!(result, Err(ActionError::BlockedByWall));
        assert_eq!(sim, before);
    }

    #[test]
    fn moving_onto_a_hazard_tile_costs_the_action_cost_plus_the_hazard_entry_cost() {
        let mut sim = Simulation::from_scenario(walled_scenario());
        sim.step(Action::MoveEast).unwrap();
        let before = sim.observe().budget_remaining;

        let report = sim.step(Action::MoveEast).unwrap();

        assert_eq!(sim.drone_position(), Position { x: 2, y: 0 });
        assert_eq!(sim.ticks_elapsed(), 2);
        assert_eq!(report.outcome, TickOutcome::Running);
        assert_eq!(
            sim.observe().budget_remaining,
            before - ACTION_COST - HAZARD_ENTRY_COST
        );
        assert!(report.events.contains(&SimEvent::HazardEntered {
            position: Position { x: 2, y: 0 },
            amount: HAZARD_ENTRY_COST,
        }));
    }

    #[test]
    fn waiting_on_a_hazard_tile_costs_only_the_action_cost() {
        let mut sim = Simulation::from_scenario(walled_scenario());
        sim.step(Action::MoveEast).unwrap();
        sim.step(Action::MoveEast).unwrap();
        let before_position = sim.drone_position();
        let before_budget = sim.observe().budget_remaining;

        let report = sim.step(Action::Wait).unwrap();

        assert_eq!(sim.drone_position(), before_position);
        assert_eq!(report.outcome, TickOutcome::Running);
        assert_eq!(sim.observe().budget_remaining, before_budget - ACTION_COST);
        assert!(
            !report
                .events
                .iter()
                .any(|event| matches!(event, SimEvent::HazardEntered { .. }))
        );
    }

    #[test]
    fn leaving_and_reentering_a_hazard_tile_charges_the_entry_cost_again() {
        let mut sim = Simulation::from_scenario(walled_scenario());
        sim.step(Action::MoveEast).unwrap(); // (1, 0), floor
        let first_entry = sim.step(Action::MoveEast).unwrap(); // (2, 0), hazard
        sim.step(Action::MoveWest).unwrap(); // back to (1, 0), leaving the hazard
        let second_entry = sim.step(Action::MoveEast).unwrap(); // (2, 0) again

        for report in [&first_entry, &second_entry] {
            assert!(
                report
                    .events
                    .iter()
                    .any(|event| matches!(event, SimEvent::HazardEntered { .. }))
            );
        }
        assert_eq!(
            sim.observe().budget_remaining,
            20 - ACTION_COST * 4 - HAZARD_ENTRY_COST * 2
        );
    }

    #[test]
    fn move_and_wait_cost_the_documented_action_cost() {
        for action in [Action::MoveNorth, Action::Wait] {
            let mut sim = Simulation::from_scenario(open_scenario(20));
            let before = sim.observe().budget_remaining;

            sim.step(action).unwrap();

            assert_eq!(sim.observe().budget_remaining, before - ACTION_COST);
        }
    }

    #[test]
    fn scan_costs_the_documented_scan_cost() {
        let mut sim = Simulation::from_scenario(open_scenario(20));
        let before = sim.observe().budget_remaining;

        let report = sim.step(Action::Scan).unwrap();

        assert_eq!(sim.observe().budget_remaining, before - SCAN_COST);
        assert!(report.events.contains(&SimEvent::ActionCost {
            action: Action::Scan,
            amount: SCAN_COST,
        }));
    }

    #[test]
    fn a_rejected_action_does_not_consume_budget() {
        let mut sim = Simulation::from_scenario(open_scenario(20));
        let budget_before = sim.observe().budget_remaining;

        let result = sim.step(Action::MoveSouth);

        assert_eq!(result, Err(ActionError::OutOfBounds));
        assert_eq!(sim.observe().budget_remaining, budget_before);
    }

    #[test]
    fn reaching_the_uplink_takes_precedence_over_exhausting_the_budget_on_the_same_action() {
        let map = FacilityMap::new(
            2,
            1,
            vec![TileKind::Floor; 2],
            Position { x: 0, y: 0 },
            Position { x: 1, y: 0 },
        )
        .unwrap();
        let mut sim = Simulation::from_scenario(Scenario::new(map, ACTION_COST));

        let report = sim.step(Action::MoveEast).unwrap();

        assert_eq!(report.outcome, TickOutcome::Succeeded);
        assert_eq!(sim.observe().budget_remaining, 0);
    }

    #[test]
    fn a_hazard_entry_that_exhausts_the_budget_emits_events_in_order() {
        let tiles = vec![TileKind::Floor, TileKind::Hazard, TileKind::Floor];
        let map = FacilityMap::new(
            3,
            1,
            tiles,
            Position { x: 0, y: 0 },
            Position { x: 2, y: 0 },
        )
        .unwrap();
        let mut sim =
            Simulation::from_scenario(Scenario::new(map, ACTION_COST + HAZARD_ENTRY_COST));

        let report = sim.step(Action::MoveEast).unwrap();

        assert_eq!(
            report.events,
            vec![
                SimEvent::ActionCost {
                    action: Action::MoveEast,
                    amount: ACTION_COST,
                },
                SimEvent::HazardEntered {
                    position: Position { x: 1, y: 0 },
                    amount: HAZARD_ENTRY_COST,
                },
                SimEvent::BudgetExhausted,
            ]
        );
        assert_eq!(
            report.outcome,
            TickOutcome::Failed(FailureReason::BudgetExhausted)
        );
    }

    #[test]
    fn first_contact_scenario_admits_a_hazard_free_route_and_a_costlier_hazard_route() {
        let safe_route = [
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
        ];
        let risky_route = [
            Action::MoveNorth,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
        ];

        let mut safe = Simulation::new();
        let mut safe_events = Vec::new();
        for action in safe_route {
            safe_events.push(safe.step(action).unwrap());
        }

        let mut risky = Simulation::new();
        let mut risky_events = Vec::new();
        for action in risky_route {
            risky_events.push(risky.step(action).unwrap());
        }

        assert_eq!(safe.outcome(), TickOutcome::Succeeded);
        assert_eq!(risky.outcome(), TickOutcome::Succeeded);
        assert_eq!(safe.observe().budget_remaining, 15 - 8);
        assert_eq!(risky.observe().budget_remaining, 15 - 8 - HAZARD_ENTRY_COST);
        assert!(!safe_events.iter().any(|report| {
            report
                .events
                .iter()
                .any(|event| matches!(event, SimEvent::HazardEntered { .. }))
        }));
        assert!(risky_events.iter().any(|report| {
            report
                .events
                .iter()
                .any(|event| matches!(event, SimEvent::HazardEntered { .. }))
        }));
    }

    /// Runs `route` against `scenario`, panicking if any action is rejected,
    /// and returns the final [`Simulation`] for the caller to assert
    /// against.
    fn run_route(scenario: Scenario, route: &[Action]) -> Simulation {
        let mut sim = Simulation::from_scenario(scenario);
        for &action in route {
            sim.step(action)
                .expect("every action in the route is valid");
        }
        sim
    }

    #[test]
    fn scanning_the_dead_end_pocket_saves_the_exploration_a_passive_backtrack_would_cost() {
        // `(2, 0)` (see `FIRST_CONTACT_ROWS`) is a dead end off the shared
        // corridor. Passive discovery reveals the drone's tile plus its 4
        // cardinal neighbours every tick, so a controller never needs to
        // step into `(2, 0)` to rule it out — reaching `(2, 1)` already
        // reveals it. But *reaching* `(2, 1)` at all is itself two actions
        // spent probing east before ever confirming the west corridor is
        // safe, and reacting to what it finds there by falling back to the
        // known-safe west route costs two more actions backtracking to
        // `(0, 1)`: four actions of committed-then-abandoned exploration,
        // exactly what `a_careful_passive_strategy_without_scanning_succeeds_with_a_smaller_margin`'s
        // route walks. A single scan from the start reveals the same area
        // (and the hazard-free direction) for 2, without ever leaving
        // `(0, 0)`.
        let scan_then_go = [
            Action::Scan,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
        ];
        let passive_probe_then_backtrack = [
            Action::MoveNorth,
            Action::MoveEast,
            Action::MoveEast, // (2, 1): passively discovers (2, 0) is a dead end
            Action::MoveWest,
            Action::MoveWest, // back to (0, 1), committing to the west corridor
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
        ];

        let scanned = run_route(Scenario::first_contact(), &scan_then_go);
        let passive = run_route(Scenario::first_contact(), &passive_probe_then_backtrack);

        assert_eq!(scanned.outcome(), TickOutcome::Succeeded);
        assert_eq!(passive.outcome(), TickOutcome::Succeeded);
        assert!(
            scanned.observe().budget_remaining > passive.observe().budget_remaining,
            "a scan that resolves the branch up front should leave more budget \
             than exploring into it and backtracking: scanned {}, passive {}",
            scanned.observe().budget_remaining,
            passive.observe().budget_remaining
        );
        assert_eq!(scanned.observe().budget_remaining, 5);
        assert_eq!(passive.observe().budget_remaining, 3);
    }

    #[test]
    fn a_good_adaptive_strategy_with_one_scan_succeeds_with_the_intended_budget_margin() {
        // One scan at the start, then the safe route it confirms: the
        // strategy envelope's "good adaptive strategy with one useful scan"
        // (`docs/TUI_DESIGN.md`, "First Contact configuration model").
        let route = [
            Action::Scan,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
        ];

        let sim = run_route(Scenario::first_contact(), &route);

        assert_eq!(sim.outcome(), TickOutcome::Succeeded);
        let remaining = sim.observe().budget_remaining;
        assert!(
            (4..=6).contains(&remaining),
            "expected the adaptive strategy to leave 4-6 budget, left {remaining}"
        );
    }

    #[test]
    fn a_careful_passive_strategy_without_scanning_succeeds_with_a_smaller_margin() {
        // No scan: the controller probes two tiles east, reconsiders, and
        // falls back to the known-safe west corridor. This is the strategy
        // envelope's "careful passive exploration with no scan".
        let route = [
            Action::MoveNorth,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveWest,
            Action::MoveWest,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
        ];

        let sim = run_route(Scenario::first_contact(), &route);

        assert_eq!(sim.outcome(), TickOutcome::Succeeded);
        let remaining = sim.observe().budget_remaining;
        assert!(
            (2..=4).contains(&remaining),
            "expected the careful passive strategy to leave 2-4 budget, left {remaining}"
        );
    }

    #[test]
    fn a_strategy_that_crosses_the_hazard_once_still_succeeds_but_narrowly() {
        let route = [
            Action::MoveNorth,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveNorth, // onto the hazard at (4, 2)
            Action::MoveNorth,
            Action::MoveNorth,
        ];

        let sim = run_route(Scenario::first_contact(), &route);

        assert_eq!(sim.outcome(), TickOutcome::Succeeded);
        let remaining = sim.observe().budget_remaining;
        assert!(
            remaining <= 3,
            "expected crossing the hazard once to leave only a narrow margin, left {remaining}"
        );
    }

    #[test]
    fn repeated_unnecessary_scanning_can_put_success_at_risk() {
        // Four scans before ever moving, then the otherwise-safe route:
        // scanning that much is never necessary on this map (one scan
        // already reveals everything a route decision needs), and it costs
        // enough that the operation fails on budget before reaching the
        // uplink.
        let mut sim = Simulation::from_scenario(Scenario::first_contact());
        for _ in 0..4 {
            assert_eq!(
                sim.step(Action::Scan).unwrap().outcome,
                TickOutcome::Running
            );
        }
        let route = [
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
        ];
        let mut outcome = TickOutcome::Running;
        for &action in &route {
            if sim.outcome() != TickOutcome::Running {
                break;
            }
            outcome = sim.step(action).unwrap().outcome;
        }

        assert_eq!(
            outcome,
            TickOutcome::Failed(FailureReason::BudgetExhausted),
            "repeated unnecessary scanning should put success at risk on this budget"
        );
    }

    #[test]
    fn repeated_waiting_exhausts_the_budget_before_reaching_the_uplink() {
        let mut sim = Simulation::from_scenario(Scenario::first_contact());
        let mut outcome = TickOutcome::Running;
        while outcome == TickOutcome::Running {
            outcome = sim.step(Action::Wait).unwrap().outcome;
        }

        assert_eq!(outcome, TickOutcome::Failed(FailureReason::BudgetExhausted));
    }

    #[test]
    fn the_south_uplink_configuration_admits_a_shorter_passive_route() {
        // `first_contact_south_uplink()`'s uplink sits much closer to the
        // shared corridor's fork than the other two configurations', so a
        // purely passive route legitimately leaves more than the shared
        // envelope's bands describe — the strategy envelope explicitly
        // permits this ("where the configuration permits that envelope").
        let route = [
            Action::MoveNorth,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveSouth,
        ];

        let sim = run_route(Scenario::first_contact_south_uplink(), &route);

        assert_eq!(sim.outcome(), TickOutcome::Succeeded);
        assert_eq!(sim.observe().budget_remaining, 9);
    }

    #[test]
    fn the_row1_hazard_configuration_admits_the_same_adaptive_and_risky_margins() {
        let adaptive_route = [
            Action::Scan,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
        ];
        let risky_route = [
            Action::MoveNorth,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast, // onto the hazard at (4, 1)
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
        ];

        let adaptive = run_route(Scenario::first_contact_row1_hazard(), &adaptive_route);
        let risky = run_route(Scenario::first_contact_row1_hazard(), &risky_route);

        assert_eq!(adaptive.outcome(), TickOutcome::Succeeded);
        assert_eq!(risky.outcome(), TickOutcome::Succeeded);
        assert!((4..=6).contains(&adaptive.observe().budget_remaining));
        assert!(risky.observe().budget_remaining <= 3);
    }

    /// A discovered floor tile, for building expected `discovered` lists in
    /// tests against the fully-open `open_scenario` map.
    fn floor_tile(x: i32, y: i32) -> DiscoveredTile {
        DiscoveredTile {
            position: Position { x, y },
            kind: TileKind::Floor,
            is_traversable: true,
            is_uplink: false,
        }
    }

    #[test]
    fn observe_reports_fixed_scenario_details_at_start() {
        let sim = Simulation::from_scenario(open_scenario(5));

        assert_eq!(
            sim.observe(),
            Observation {
                drone_position: Position { x: 0, y: 0 },
                tick: 0,
                budget_remaining: 5,
                discovered: vec![floor_tile(0, 0), floor_tile(1, 0), floor_tile(0, 1)],
            }
        );
    }

    #[test]
    fn observe_reflects_elapsed_ticks_after_moves() {
        let mut sim = Simulation::from_scenario(open_scenario(5));
        sim.step(Action::MoveNorth).unwrap();
        sim.step(Action::MoveEast).unwrap();

        assert_eq!(
            sim.observe(),
            Observation {
                drone_position: Position { x: 1, y: 1 },
                tick: 2,
                budget_remaining: 3,
                discovered: vec![
                    floor_tile(0, 0),
                    floor_tile(1, 0),
                    floor_tile(0, 1),
                    floor_tile(1, 1),
                    floor_tile(2, 1),
                    floor_tile(0, 2),
                    floor_tile(1, 2),
                ],
            }
        );
    }

    #[test]
    fn budget_does_not_underflow_when_a_single_action_costs_more_than_remains() {
        let mut sim = Simulation::from_scenario(walled_scenario_with_budget(2));
        sim.step(Action::MoveEast).unwrap(); // (1, 0), floor, budget 2 -> 1
        let report = sim.step(Action::MoveEast).unwrap(); // (2, 0), hazard, cost 6 > 1 remaining

        assert_eq!(sim.observe().budget_remaining, 0);
        assert_eq!(
            report.outcome,
            TickOutcome::Failed(FailureReason::BudgetExhausted)
        );
    }

    #[test]
    fn step_after_completion_is_rejected() {
        let mut sim = Simulation::from_scenario(open_scenario(1));
        sim.step(Action::Wait).unwrap();
        assert_eq!(
            sim.outcome(),
            TickOutcome::Failed(FailureReason::BudgetExhausted)
        );

        let before = sim.clone();
        let result = sim.step(Action::Wait);

        assert_eq!(result, Err(ActionError::SimulationEnded));
        assert_eq!(sim, before);
    }

    #[test]
    fn repeated_runs_of_the_fixed_map_produce_identical_final_states() {
        let route = [
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveNorth,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
            Action::MoveEast,
        ];

        let mut a = Simulation::new();
        let mut b = Simulation::new();

        for action in route {
            let outcome_a = a.step(action).unwrap();
            let outcome_b = b.step(action).unwrap();
            assert_eq!(outcome_a, outcome_b);
            assert_eq!(a, b);
        }
    }

    /// A 7x7 fully open map, for scan tests that need room for the 5x5 scan
    /// area to not simply cover the whole map.
    fn big_open_scenario(starting_budget: u32) -> Scenario {
        let map = FacilityMap::new(
            7,
            7,
            vec![TileKind::Floor; 49],
            Position { x: 3, y: 3 },
            Position { x: 6, y: 6 },
        )
        .unwrap();
        Scenario::new(map, starting_budget)
    }

    #[test]
    fn scan_reveals_a_five_by_five_area_around_the_drone() {
        let mut sim = Simulation::from_scenario(big_open_scenario(20));
        sim.step(Action::Scan).unwrap();

        let discovered: HashSet<Position> = sim
            .observe()
            .discovered
            .into_iter()
            .map(|tile| tile.position)
            .collect();

        for dx in -2..=2 {
            for dy in -2..=2 {
                let position = Position {
                    x: 3 + dx,
                    y: 3 + dy,
                };
                assert!(
                    discovered.contains(&position),
                    "{position:?} should be discovered by the scan"
                );
            }
        }
        assert_eq!(discovered.len(), 25);
    }

    #[test]
    fn scan_does_not_move_the_drone_but_advances_the_tick() {
        let mut sim = Simulation::from_scenario(open_scenario(20));

        let outcome = sim.step(Action::Scan).unwrap().outcome;

        assert_eq!(sim.drone_position(), Position { x: 0, y: 0 });
        assert_eq!(sim.ticks_elapsed(), 1);
        assert_eq!(outcome, TickOutcome::Running);
    }

    #[test]
    fn scan_reveals_walls_without_occlusion() {
        // The wall at (1, 1) is diagonal from the start, so passive local
        // vision (cardinal neighbours only) never reveals it; only a scan
        // does.
        let mut sim = Simulation::from_scenario(walled_scenario());

        sim.step(Action::Scan).unwrap();

        let tile = sim
            .observe()
            .discovered
            .into_iter()
            .find(|tile| tile.position == Position { x: 1, y: 1 })
            .expect("the wall at (1, 1) should be discovered by the scan");

        assert_eq!(tile.kind, TileKind::Wall);
        assert!(!tile.is_traversable);
    }

    #[test]
    fn scan_at_a_map_corner_only_discovers_in_bounds_tiles() {
        let mut sim = Simulation::from_scenario(open_scenario(20));

        sim.step(Action::Scan).unwrap();

        for tile in sim.observe().discovered {
            assert!((0..3).contains(&tile.position.x));
            assert!((0..3).contains(&tile.position.y));
        }
    }

    #[test]
    fn discoveries_persist_after_the_drone_moves_away() {
        let mut sim = Simulation::from_scenario(open_scenario(20));
        sim.step(Action::Scan).unwrap();
        let discovered_after_scan: HashSet<Position> = sim
            .observe()
            .discovered
            .into_iter()
            .map(|tile| tile.position)
            .collect();

        sim.step(Action::MoveNorth).unwrap();
        sim.step(Action::MoveNorth).unwrap();

        let discovered_now: HashSet<Position> = sim
            .observe()
            .discovered
            .into_iter()
            .map(|tile| tile.position)
            .collect();

        for position in discovered_after_scan {
            assert!(discovered_now.contains(&position));
        }
    }

    #[test]
    fn undiscovered_uplink_and_hazard_are_not_in_the_observation() {
        let sim = Simulation::from_scenario(walled_scenario());

        let discovered = sim.observe().discovered;

        assert!(discovered.iter().all(|tile| !tile.is_uplink));
        assert!(discovered.iter().all(|tile| tile.kind != TileKind::Hazard));
    }

    #[test]
    fn moving_expands_the_discovered_set() {
        let mut sim = Simulation::from_scenario(open_scenario(20));
        let before = sim.observe().discovered.len();

        sim.step(Action::MoveNorth).unwrap();

        assert!(sim.observe().discovered.len() > before);
    }

    #[test]
    fn identical_action_sequences_including_scans_produce_identical_states() {
        let actions = [
            Action::MoveNorth,
            Action::Scan,
            Action::MoveEast,
            Action::Scan,
        ];

        let mut a = Simulation::from_scenario(open_scenario(20));
        let mut b = Simulation::from_scenario(open_scenario(20));

        for action in actions {
            assert_eq!(a.step(action), b.step(action));
            assert_eq!(a, b);
        }
    }
}
