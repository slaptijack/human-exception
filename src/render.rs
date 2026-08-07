//! Text rendering of discovered facility terrain as a compact satellite
//! view.
//!
//! This module has no knowledge of Lua or [`crate::simulation::Simulation`];
//! it only turns already-computed, already-authorized presentation data
//! ([`Position`], [`DiscoveredTile`]) into a `String`. It never inspects the
//! real kind of a tile that has not been discovered, so hidden terrain,
//! hazards, and the uplink objective cannot leak through it.

use std::collections::HashMap;

use crate::simulation::{DiscoveredTile, Position, TileKind};

/// Renders the drone's current satellite view: a north-up grid (row `y =
/// map_height - 1` first, row `y = 0` last, matching the facility map's
/// documented coordinate convention) with one symbol per tile:
///
/// - `D` — the drone's current position
/// - `U` — a discovered uplink tile
/// - `.` — discovered floor
/// - `#` — discovered wall
/// - `~` — discovered hazard
/// - `?` — not yet discovered
///
/// `discovered` need not be sorted; only tiles present in it (plus the
/// drone's own position) are ever rendered as anything other than `?`.
/// Column and row labels are padded to the longest label on their axis, so
/// the grid stays aligned for any map size without depending on terminal
/// width detection. For example, at the start of the fixed "first contact"
/// scenario:
///
/// ```text
/// SATELLITE FEED // discovered terrain
///      x=0 x=1 x=2 x=3 x=4
/// y=4 |   ?   ?   ?   ?   ?
/// y=3 |   ?   ?   ?   ?   ?
/// y=2 |   ?   ?   ?   ?   ?
/// y=1 |   .   ?   ?   ?   ?
/// y=0 |   D   #   ?   ?   ?
/// legend: D drone   U uplink   . floor   # wall   ~ hazard   ? undiscovered
/// ```
pub fn render_satellite_view(
    drone_position: Position,
    map_width: i32,
    map_height: i32,
    discovered: &[DiscoveredTile],
) -> String {
    let by_position: HashMap<Position, &DiscoveredTile> = discovered
        .iter()
        .map(|tile| (tile.position, tile))
        .collect();

    let column_label_width = (0..map_width)
        .map(|x| format!("x={x}").len())
        .max()
        .unwrap_or(0);
    let row_label_width = (0..map_height)
        .map(|y| format!("y={y}").len())
        .max()
        .unwrap_or(0);

    let mut lines = Vec::with_capacity(map_height as usize + 3);
    lines.push("SATELLITE FEED // discovered terrain".to_string());

    let mut header = " ".repeat(row_label_width + 1);
    for x in 0..map_width {
        header.push_str(&format!(" {:>column_label_width$}", format!("x={x}")));
    }
    lines.push(header);

    for y in (0..map_height).rev() {
        let mut row = format!("{:>row_label_width$} |", format!("y={y}"));
        for x in 0..map_width {
            let position = Position { x, y };
            let symbol = tile_symbol(position, drone_position, &by_position);
            row.push_str(&format!(" {symbol:>column_label_width$}"));
        }
        lines.push(row);
    }

    lines.push(
        "legend: D drone   U uplink   . floor   # wall   ~ hazard   ? undiscovered".to_string(),
    );

    lines.join("\n")
}

fn tile_symbol(
    position: Position,
    drone_position: Position,
    by_position: &HashMap<Position, &DiscoveredTile>,
) -> char {
    if position == drone_position {
        return 'D';
    }

    match by_position.get(&position) {
        Some(tile) if tile.is_uplink => 'U',
        Some(tile) => match tile.kind {
            TileKind::Floor => '.',
            TileKind::Wall => '#',
            TileKind::Hazard => '~',
        },
        None => '?',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(x: i32, y: i32, kind: TileKind, is_uplink: bool) -> DiscoveredTile {
        DiscoveredTile {
            position: Position { x, y },
            kind,
            is_traversable: kind.is_traversable(),
            is_uplink,
        }
    }

    /// Returns only the grid rows (not the header, column labels, or
    /// legend), keyed by `y`, so assertions don't accidentally match the
    /// legend text's own `~`/`U`/`.`/`#` characters.
    fn grid_row(view: &str, y: i32) -> &str {
        let prefix = format!("y={y} |");
        view.lines()
            .find(|line| line.starts_with(&prefix))
            .unwrap_or_else(|| panic!("no grid row for y={y} in:\n{view}"))
    }

    #[test]
    fn initial_frame_shows_only_the_drone_and_its_discovered_neighbours() {
        let discovered = vec![
            tile(0, 0, TileKind::Floor, false),
            tile(1, 0, TileKind::Wall, false),
            tile(0, 1, TileKind::Floor, false),
        ];

        let view = render_satellite_view(Position { x: 0, y: 0 }, 5, 5, &discovered);

        assert_eq!(
            view,
            "\
SATELLITE FEED // discovered terrain
     x=0 x=1 x=2 x=3 x=4
y=4 |   ?   ?   ?   ?   ?
y=3 |   ?   ?   ?   ?   ?
y=2 |   ?   ?   ?   ?   ?
y=1 |   .   ?   ?   ?   ?
y=0 |   D   #   ?   ?   ?
legend: D drone   U uplink   . floor   # wall   ~ hazard   ? undiscovered"
        );
    }

    #[test]
    fn movement_moves_the_drone_symbol_and_keeps_prior_discoveries() {
        let discovered = vec![
            tile(0, 0, TileKind::Floor, false),
            tile(1, 0, TileKind::Wall, false),
            tile(0, 1, TileKind::Floor, false),
            tile(0, 2, TileKind::Floor, false),
            tile(1, 1, TileKind::Wall, false),
        ];

        let view = render_satellite_view(Position { x: 0, y: 1 }, 5, 5, &discovered);

        assert_eq!(grid_row(&view, 1), "y=1 |   D   #   ?   ?   ?");
        assert_eq!(grid_row(&view, 0), "y=0 |   .   #   ?   ?   ?");
    }

    #[test]
    fn scanning_reveals_the_hazard_and_uplink_symbols() {
        let discovered = vec![
            tile(4, 2, TileKind::Hazard, false),
            tile(4, 4, TileKind::Floor, true),
        ];

        let view = render_satellite_view(Position { x: 4, y: 2 }, 5, 5, &discovered);

        assert_eq!(grid_row(&view, 4), "y=4 |   ?   ?   ?   ?   U");
        assert_eq!(grid_row(&view, 2), "y=2 |   ?   ?   ?   ?   D");
    }

    #[test]
    fn undiscovered_hazard_and_uplink_tiles_are_never_leaked() {
        // Deliberately omit the hazard at (4, 2) and the uplink at (4, 4)
        // from `discovered`, as if they had not been observed yet.
        let discovered = vec![tile(0, 0, TileKind::Floor, false)];

        let view = render_satellite_view(Position { x: 0, y: 0 }, 5, 5, &discovered);

        assert!(!grid_row(&view, 2).contains('~'));
        assert!(!grid_row(&view, 4).contains('U'));
    }

    #[test]
    fn rendering_the_same_state_twice_is_byte_for_byte_identical() {
        let discovered = vec![
            tile(0, 0, TileKind::Floor, false),
            tile(1, 0, TileKind::Wall, false),
        ];

        let first = render_satellite_view(Position { x: 0, y: 0 }, 5, 5, &discovered);
        let second = render_satellite_view(Position { x: 0, y: 0 }, 5, 5, &discovered);

        assert_eq!(first, second);
    }

    #[test]
    fn the_drone_symbol_takes_priority_over_the_tile_beneath_it() {
        let discovered = vec![tile(4, 4, TileKind::Floor, true)];

        let view = render_satellite_view(Position { x: 4, y: 4 }, 5, 5, &discovered);

        assert_eq!(grid_row(&view, 4), "y=4 |   ?   ?   ?   ?   D");
    }
}
