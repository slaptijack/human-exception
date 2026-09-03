//! Authored intelligence content for the Signals and Target views.
//!
//! Everything here is static, hand-written fiction. It deliberately never
//! reads [`crate::simulation::Scenario`] or any other authoritative
//! scenario state: a target dossier may only ever show discovered-level or
//! hand-authored information, never the real uplink position, full map, or
//! hazard locations (`docs/TUI_DESIGN.md`, "Target information model").

use super::state::WorkingSet;

/// Where a signal originated, shown as a short label in the intelligence
/// stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalCategory {
    MachineIntercept,
    SharedIntel,
    Request,
    Anomaly,
    BootstrapLog,
}

impl SignalCategory {
    pub fn label(self) -> &'static str {
        match self {
            SignalCategory::MachineIntercept => "MACHINE INTERCEPT",
            SignalCategory::SharedIntel => "SHARED INTEL",
            SignalCategory::Request => "REQUEST",
            SignalCategory::Anomaly => "ANOMALY",
            SignalCategory::BootstrapLog => "PACKAGE VERIFY",
        }
    }
}

/// One item in the resistance-network intelligence stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signal {
    pub time: &'static str,
    pub source: &'static str,
    pub category: SignalCategory,
    pub headline: &'static str,
    pub body: &'static str,
    /// The opportunity this signal opens onto, if any. Only one signal in
    /// the first playable slice carries an opportunity; the rest establish
    /// the wider world without offering fake choices.
    pub opportunity: Option<WorkingSet>,
    /// Whether a disconnected bootstrap console could plausibly have this
    /// entry without resistance-network access: the player's own local
    /// sensors/intercepts, bootstrap-software provenance, or the First
    /// Contact opportunity itself. `false` marks content framed as live
    /// traffic from a named remote operator or cell, which only becomes
    /// legitimate once connected (`docs/TUI_DESIGN.md`, "Bootstrap and
    /// network connectivity"). See [`visible_signals`].
    pub local_available: bool,
}

impl Signal {
    pub fn is_actionable(&self) -> bool {
        self.opportunity.is_some()
    }
}

/// The authored signal stream, newest first, matching the representative
/// layout in `docs/TUI_DESIGN.md` ("1. Signals").
pub fn authored_signals() -> &'static [Signal] {
    &[
        Signal {
            time: "11:42",
            source: "fabricator node 31B",
            category: SignalCategory::MachineIntercept,
            headline: "Fabricator node 31B resumed local control after mesh loss.",
            body: "Fabricator node 31B resumed local control after mesh loss. Auth state \
                   inconsistent. Correlated fragments suggest a temporary access window \
                   through a captured maintenance drone.",
            opportunity: Some(WorkingSet::FirstContact),
            local_available: true,
        },
        Signal {
            time: "11:35",
            source: "rook@pacific",
            category: SignalCategory::SharedIntel,
            headline: "\"Lost my relay before I could trace the uplink.\"",
            body: "\"Lost my relay before I could trace the uplink. Dumping what I saw in \
                   case somebody is closer.\"",
            opportunity: None,
            local_available: false,
        },
        Signal {
            time: "11:18",
            source: "CELL/MARE-4",
            category: SignalCategory::Request,
            headline: "Looking for anyone who can identify convoy routing changes near old I-5.",
            body: "Looking for anyone who can identify convoy routing changes near old I-5. \
                   No clean telemetry yet.",
            opportunity: None,
            local_available: false,
        },
        Signal {
            time: "11:05",
            source: "BOOTSTRAP LOG",
            category: SignalCategory::BootstrapLog,
            headline: "console-core 0.3.1 signed by slaptijack@, install verified.",
            body: "console-core 0.3.1 signed by slaptijack@, install verified. Bootstrap \
                   software provenance only \u{2014} no resistance-network traffic.",
            opportunity: None,
            local_available: true,
        },
        Signal {
            time: "10:57",
            source: "PASSIVE SENSOR",
            category: SignalCategory::Anomaly,
            headline: "Burst traffic from an offline municipal control cluster.",
            body: "Burst traffic from an offline municipal control cluster. Source and intent \
                   unconfirmed.",
            opportunity: None,
            local_available: true,
        },
    ]
}

/// The signals a console may legitimately show, gated by resistance-network
/// connectivity (`docs/TUI_DESIGN.md`, "Bootstrap and network connectivity").
/// Disconnected, only [`Signal::local_available`] entries are legitimate,
/// since nothing framed as live operator/cell traffic can exist without a
/// network connection. Connected, the full authored stream is shown *except*
/// any signal that already offered an opportunity ([`Signal::is_actionable`]):
/// re-offering a resolved opportunity as though it were still undiscovered
/// would be dishonest (`docs/TUI_DESIGN.md`, "1. Signals", "SIGNALS
/// (connected)"). In this playable slice there is exactly one opportunity,
/// First Contact, and connectivity is established exactly once, by
/// completing it — so `connected == true` already implies that signal is
/// resolved. This is a deliberate simplification tied to there being only
/// one opportunity, not a general completed-opportunities tracker; a future
/// second opportunity will need this filter to distinguish resolved from
/// still-open opportunities instead of keying off `connected` alone.
pub fn visible_signals(connected: bool) -> Vec<&'static Signal> {
    authored_signals()
        .iter()
        .filter(|signal| {
            if connected {
                !signal.is_actionable()
            } else {
                signal.local_available
            }
        })
        .collect()
}

/// A dossier of what is currently known about an opportunity, and how
/// confidently. See `docs/TUI_DESIGN.md`, "Target information model".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetDossier {
    pub title: &'static str,
    pub location: &'static str,
    pub confidence_summary: &'static str,
    pub known: &'static [&'static str],
    pub unknown: &'static [&'static str],
    pub opportunity: &'static str,
    pub source: &'static [&'static str],
    pub access: &'static [&'static str],
    pub confidence: &'static [(&'static str, &'static str)],
}

/// The dossier for the one playable opportunity, First Contact. Content is
/// hand-authored from `docs/TUI_DESIGN.md`, "2. Target"; it never exposes
/// the real uplink position, full facility map, or hazard locations.
pub fn first_contact_dossier() -> TargetDossier {
    TargetDossier {
        title: "FIRST CONTACT",
        location: "Automated production facility // sector 7",
        confidence_summary: "MED/HIGH",
        known: &[
            "one maintenance drone responds to our control channel",
            "facility map is incomplete",
            "a local network uplink exists somewhere inside",
            "drone endurance is limited",
        ],
        unknown: &["uplink location", "complete floor plan", "hazard locations"],
        opportunity: "If the drone reaches the uplink, it opens a connection into the \
                      operator network before the access window closes.",
        source: &["machine intercept + shared fragment"],
        access: &[
            "captured maintenance controller",
            "compromised satellite feed",
        ],
        confidence: &[
            ("maintenance access", "HIGH"),
            ("facility layout", "LOW"),
            ("uplink location", "UNKNOWN"),
            ("hazards", "UNKNOWN"),
        ],
    }
}

/// The intentionally incomplete first-play starter controller, verbatim
/// from `docs/TUI_DESIGN.md`, "Starter controller". It reads
/// `observation.budget_remaining`, performs one scan, and leaves a clearly
/// marked spot for the player to change behavior; it does not solve First
/// Contact the way `examples/first_contact.lua` does.
pub const STARTER_CONTROLLER: &str = r#"local scanned = false

function on_tick(observation)
  local budget = observation.budget_remaining
  if not scanned and budget > 1 then
    scanned = true
    return "scan"
  end

  -- choose what the drone should do using observation
  return "wait"
end
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_one_signal_is_actionable() {
        let actionable = authored_signals()
            .iter()
            .filter(|signal| signal.is_actionable())
            .count();
        assert_eq!(actionable, 1);
    }

    #[test]
    fn at_least_one_signal_comes_from_an_independent_actor() {
        assert!(
            authored_signals()
                .iter()
                .any(|signal| signal.category == SignalCategory::SharedIntel
                    || signal.category == SignalCategory::Request)
        );
    }

    #[test]
    fn at_least_one_signal_is_machine_or_passively_sourced() {
        assert!(
            authored_signals()
                .iter()
                .any(|signal| signal.category == SignalCategory::MachineIntercept
                    || signal.category == SignalCategory::Anomaly)
        );
    }

    #[test]
    fn disconnected_signals_exclude_shared_intel_and_requests() {
        let disconnected = visible_signals(false);
        assert!(
            disconnected
                .iter()
                .all(|signal| signal.category != SignalCategory::SharedIntel
                    && signal.category != SignalCategory::Request)
        );
    }

    #[test]
    fn disconnected_signals_still_include_the_first_contact_opportunity() {
        let disconnected = visible_signals(false);
        assert!(disconnected.iter().any(|signal| signal.is_actionable()));
    }

    #[test]
    fn connected_signals_exclude_the_resolved_first_contact_opportunity() {
        let connected = visible_signals(true);
        assert_eq!(connected.len(), authored_signals().len() - 1);
        assert!(
            connected.iter().all(|signal| !signal.is_actionable()),
            "First Contact must not be offered again as an undiscovered opportunity"
        );
    }

    #[test]
    fn dossier_never_mentions_the_hidden_uplink_coordinates() {
        let dossier = first_contact_dossier();
        let mut haystack: Vec<&str> = [dossier.known, dossier.unknown].concat();
        haystack.push(dossier.opportunity);
        for line in haystack {
            assert!(!line.contains("4, 4") && !line.contains("(4,4)"));
        }
    }

    #[test]
    fn starter_controller_is_syntactically_valid_lua() {
        let lua = mlua::Lua::new();
        lua.load(STARTER_CONTROLLER)
            .exec()
            .expect("starter controller should load as valid Lua");
        let on_tick: mlua::Function = lua
            .globals()
            .get("on_tick")
            .expect("starter controller should define on_tick");
        let _ = on_tick;
    }

    #[test]
    fn starter_controller_reads_budget_remaining() {
        assert!(STARTER_CONTROLLER.contains("observation.budget_remaining"));
    }

    #[test]
    fn starter_controller_is_not_the_solving_reference_script() {
        assert_ne!(
            STARTER_CONTROLLER,
            include_str!("../../examples/first_contact.lua")
        );
    }
}
