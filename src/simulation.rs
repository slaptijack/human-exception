//! The deterministic drone training simulation.
//!
//! This module models the fixed "first contact" training operation: a single
//! captured maintenance drone must reach a network-uplink objective within a
//! limited number of ticks. All authoritative state transitions live here;
//! this module has no knowledge of Lua, the command line, or terminal
//! output.

use std::error::Error;
use std::fmt;

/// A grid coordinate in the training scenario.
///
/// Coordinates are signed so that a move off the edge of the scenario is a
/// representable (and rejected) value rather than an unsigned underflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

/// The fixed training scenario: one drone, one uplink, one tick budget,
/// bounded by a fixed-size area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scenario {
    pub width: i32,
    pub height: i32,
    pub drone_start: Position,
    pub uplink: Position,
    pub tick_limit: u32,
}

impl Scenario {
    /// The one fixed "first contact" training scenario.
    pub fn first_contact() -> Self {
        Scenario {
            width: 5,
            height: 5,
            drone_start: Position { x: 0, y: 0 },
            uplink: Position { x: 4, y: 4 },
            tick_limit: 20,
        }
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
    TickLimitReached,
}

/// An action that was rejected without mutating simulation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionError {
    /// The operation has already succeeded or failed; no further actions
    /// are accepted.
    SimulationEnded,
    /// The requested move would leave the bounded training area.
    OutOfBounds,
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
                write!(f, "that move would leave the bounded training area")
            }
        }
    }
}

impl Error for ActionError {}

/// The authoritative, deterministic state of a training operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Simulation {
    scenario: Scenario,
    drone_position: Position,
    ticks_elapsed: u32,
    outcome: TickOutcome,
}

impl Simulation {
    /// Starts a new simulation of the fixed "first contact" scenario.
    ///
    /// A new simulation always starts from the same state.
    pub fn new() -> Self {
        Self::from_scenario(Scenario::first_contact())
    }

    fn from_scenario(scenario: Scenario) -> Self {
        Simulation {
            scenario,
            drone_position: scenario.drone_start,
            ticks_elapsed: 0,
            outcome: TickOutcome::Running,
        }
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

    /// Submits one action for this tick and advances the simulation by one
    /// tick, returning the resulting outcome.
    ///
    /// If the action is invalid or impossible, or the operation has already
    /// ended, this returns an error and leaves state unchanged.
    pub fn step(&mut self, action: Action) -> Result<TickOutcome, ActionError> {
        if self.outcome != TickOutcome::Running {
            return Err(ActionError::SimulationEnded);
        }

        let next_position = self.apply(action)?;

        self.drone_position = next_position;
        self.ticks_elapsed += 1;
        self.outcome = if self.drone_position == self.scenario.uplink {
            TickOutcome::Succeeded
        } else if self.ticks_elapsed >= self.scenario.tick_limit {
            TickOutcome::Failed(FailureReason::TickLimitReached)
        } else {
            TickOutcome::Running
        };

        Ok(self.outcome)
    }

    /// Computes the position `action` would produce without mutating state,
    /// rejecting moves that would leave the bounded training area.
    fn apply(&self, action: Action) -> Result<Position, ActionError> {
        let Position { x, y } = self.drone_position;
        let candidate = match action {
            Action::MoveNorth => Position { x, y: y + 1 },
            Action::MoveSouth => Position { x, y: y - 1 },
            Action::MoveEast => Position { x: x + 1, y },
            Action::MoveWest => Position { x: x - 1, y },
            Action::Wait => Position { x, y },
        };

        let in_bounds = (0..self.scenario.width).contains(&candidate.x)
            && (0..self.scenario.height).contains(&candidate.y);

        if in_bounds {
            Ok(candidate)
        } else {
            Err(ActionError::OutOfBounds)
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

    fn small_scenario(tick_limit: u32) -> Scenario {
        Scenario {
            width: 3,
            height: 3,
            drone_start: Position { x: 0, y: 0 },
            uplink: Position { x: 2, y: 2 },
            tick_limit,
        }
    }

    #[test]
    fn new_simulation_has_fixed_starting_state() {
        let sim = Simulation::new();
        let scenario = Scenario::first_contact();

        assert_eq!(sim.drone_position(), scenario.drone_start);
        assert_eq!(sim.ticks_elapsed(), 0);
        assert_eq!(sim.outcome(), TickOutcome::Running);
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

        let mut a = Simulation::from_scenario(small_scenario(20));
        let mut b = Simulation::from_scenario(small_scenario(20));

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
    fn move_north_updates_position() {
        let mut sim = Simulation::from_scenario(small_scenario(20));
        sim.step(Action::MoveNorth).unwrap();
        assert_eq!(sim.drone_position(), Position { x: 0, y: 1 });
    }

    #[test]
    fn move_south_updates_position() {
        let mut sim = Simulation::from_scenario(small_scenario(20));
        sim.step(Action::MoveNorth).unwrap();
        sim.step(Action::MoveSouth).unwrap();
        assert_eq!(sim.drone_position(), Position { x: 0, y: 0 });
    }

    #[test]
    fn move_east_updates_position() {
        let mut sim = Simulation::from_scenario(small_scenario(20));
        sim.step(Action::MoveEast).unwrap();
        assert_eq!(sim.drone_position(), Position { x: 1, y: 0 });
    }

    #[test]
    fn move_west_updates_position() {
        let mut sim = Simulation::from_scenario(small_scenario(20));
        sim.step(Action::MoveEast).unwrap();
        sim.step(Action::MoveWest).unwrap();
        assert_eq!(sim.drone_position(), Position { x: 0, y: 0 });
    }

    #[test]
    fn wait_does_not_change_position_but_advances_tick() {
        let mut sim = Simulation::from_scenario(small_scenario(20));
        sim.step(Action::Wait).unwrap();
        assert_eq!(sim.drone_position(), Position { x: 0, y: 0 });
        assert_eq!(sim.ticks_elapsed(), 1);
    }

    #[test]
    fn valid_route_to_uplink_succeeds() {
        let mut sim = Simulation::from_scenario(small_scenario(20));

        assert_eq!(sim.step(Action::MoveNorth).unwrap(), TickOutcome::Running);
        assert_eq!(sim.step(Action::MoveNorth).unwrap(), TickOutcome::Running);
        assert_eq!(sim.step(Action::MoveEast).unwrap(), TickOutcome::Running);
        let outcome = sim.step(Action::MoveEast).unwrap();

        assert_eq!(outcome, TickOutcome::Succeeded);
        assert_eq!(sim.drone_position(), Position { x: 2, y: 2 });
    }

    #[test]
    fn waiting_until_tick_limit_fails() {
        let mut sim = Simulation::from_scenario(small_scenario(3));

        assert_eq!(sim.step(Action::Wait).unwrap(), TickOutcome::Running);
        assert_eq!(sim.step(Action::Wait).unwrap(), TickOutcome::Running);
        let outcome = sim.step(Action::Wait).unwrap();

        assert_eq!(
            outcome,
            TickOutcome::Failed(FailureReason::TickLimitReached)
        );
    }

    #[test]
    fn out_of_bounds_move_is_rejected_without_mutating_state() {
        let mut sim = Simulation::from_scenario(small_scenario(20));
        let before = sim;

        let result = sim.step(Action::MoveSouth);

        assert_eq!(result, Err(ActionError::OutOfBounds));
        assert_eq!(sim, before);
    }

    #[test]
    fn step_after_completion_is_rejected() {
        let mut sim = Simulation::from_scenario(small_scenario(1));
        sim.step(Action::Wait).unwrap();
        assert_eq!(
            sim.outcome(),
            TickOutcome::Failed(FailureReason::TickLimitReached)
        );

        let before = sim;
        let result = sim.step(Action::Wait);

        assert_eq!(result, Err(ActionError::SimulationEnded));
        assert_eq!(sim, before);
    }
}
