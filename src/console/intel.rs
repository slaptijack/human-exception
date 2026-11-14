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

/// How far the console trusts an entry's **inferred origin** — the
/// transmitter or sender — not the local sensor or receiver that picked it
/// up. Deliberately a different vocabulary from [`ClaimConfidence`] so the
/// two axes can't be read as one scale. Hand-authored and presentation only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceTrust {
    Verified,
    Probable,
    Unverified,
}

impl SourceTrust {
    #[cfg(test)]
    pub const ALL: [SourceTrust; 3] = [
        SourceTrust::Verified,
        SourceTrust::Probable,
        SourceTrust::Unverified,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SourceTrust::Verified => "VERIFIED",
            SourceTrust::Probable => "PROBABLE",
            SourceTrust::Unverified => "UNVERIFIED",
        }
    }
}

/// How far the console believes an entry's **principal claim** is accurate.
/// For a local sensor entry that is the recorded observation itself, not
/// who sent it or what it means; interpretation stays in hedged `context` /
/// `unresolved` text and is not rated. Independent of [`SourceTrust`]: a
/// trusted origin can carry an inaccurate claim, and the reverse.
/// Hand-authored and presentation only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimConfidence {
    High,
    Med,
    Low,
    Unknown,
}

impl ClaimConfidence {
    #[cfg(test)]
    pub const ALL: [ClaimConfidence; 4] = [
        ClaimConfidence::High,
        ClaimConfidence::Med,
        ClaimConfidence::Low,
        ClaimConfidence::Unknown,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ClaimConfidence::High => "HIGH",
            ClaimConfidence::Med => "MED",
            ClaimConfidence::Low => "LOW",
            ClaimConfidence::Unknown => "UNKNOWN",
        }
    }
}

/// One item in the resistance-network intelligence stream.
///
/// `time`, `source`, `category`, `subject`, and `preview` are what the
/// list row shows; everything else is what the selected-entry detail pane
/// adds on top of it (`docs/TUI_DESIGN.md`,
/// "1. Signals"). Every field is hand-authored fiction, hedged to what the
/// console could plausibly know at its connectivity level, never an
/// authoritative world fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signal {
    /// A stable identity for this authored entry, independent of its
    /// position in [`visible_signals`] (which differs between LOCAL LOG and
    /// SIGNALS). Session-only inspected state is keyed by it (issue #216).
    pub id: &'static str,
    pub time: &'static str,
    pub source: &'static str,
    pub category: SignalCategory,
    /// A short, distinct subject line for the list row.
    pub subject: &'static str,
    /// A one-line summary shown under the subject in the list row, cut to
    /// fit the pane where space is short.
    pub preview: &'static str,
    /// The full message, intercepted fragment, or telemetry. Deliberately
    /// does not open by repeating `subject` or `preview`, so selecting a row is worth
    /// doing.
    pub body: &'static str,
    /// How far the entry's inferred origin is trusted. Unless this is
    /// [`SourceTrust::Verified`], `subject`, `preview`, and `body` describe
    /// what was observed and never state the origin as fact (issue #230).
    pub source_trust: SourceTrust,
    /// How far the entry's principal claim is believed accurate.
    pub claim_confidence: ClaimConfidence,
    /// How the entry reached this console.
    pub provenance: &'static [&'static str],
    /// Authored correlation or background, hedged in-fiction.
    pub context: &'static [&'static str],
    /// Open questions the entry raises but does not answer.
    pub unresolved: &'static [&'static str],
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
            id: "fabricator-31b-fallback",
            time: "11:42",
            source: "fabricator node 31B",
            category: SignalCategory::MachineIntercept,
            subject: "Node 31B appears to have fallen back to local control",
            preview: "Auth state inconsistent; a maintenance channel is still answering.",
            body: "AUTH/RETRY x14 \u{2014} no mesh ack. FALLBACK: local. \
                   Maintenance channel 2 still answering a stale controller key.",
            source_trust: SourceTrust::Probable,
            claim_confidence: ClaimConfidence::High,
            provenance: &[
                "local receiver, maintenance band",
                "handshake replay, partial",
            ],
            context: &[
                "Correlated fragments suggest a temporary access window through a \
                 captured maintenance drone.",
            ],
            unresolved: &[
                "how long the stale key stays valid",
                "what else on this node still listens",
            ],
            opportunity: Some(WorkingSet::FirstContact),
            local_available: true,
        },
        Signal {
            id: "rook-relay-lost",
            time: "11:35",
            source: "rook@pacific",
            category: SignalCategory::SharedIntel,
            subject: "A sender keyed as rook reports a lost relay",
            preview: "\"Dumping what I saw in case somebody is closer.\"",
            body: "\"Relay went dark mid-trace. Dumping what I saw in case somebody is \
                   closer. Timing looked like a scheduled sweep, not a fault.\"",
            source_trust: SourceTrust::Probable,
            claim_confidence: ClaimConfidence::Med,
            provenance: &[
                "operator mesh, relayed twice",
                "sender key seen before, not verified",
            ],
            context: &["An independent operator's fragment; nothing here is addressed to us."],
            unresolved: &[
                "which uplink rook was tracing",
                "whether the sweep was aimed at rook",
            ],
            opportunity: None,
            local_available: false,
        },
        Signal {
            id: "mare4-convoy-request",
            time: "11:18",
            source: "CELL/MARE-4",
            category: SignalCategory::Request,
            subject: "Convoy routing changed near old I-5",
            preview: "Asking anyone with eyes on the corridor to share telemetry.",
            body: "\"Convoys stopped using the usual corridor three days ago. No clean \
                   telemetry yet. Anyone with eyes on old I-5, share what you have.\"",
            source_trust: SourceTrust::Unverified,
            claim_confidence: ClaimConfidence::Low,
            provenance: &["open cell broadcast, unsigned"],
            context: &["A request to the wider network, not a task assigned to this console."],
            unresolved: &["why the routing changed", "whether anyone has answered"],
            opportunity: None,
            local_available: false,
        },
        Signal {
            id: "console-core-verify",
            time: "11:05",
            source: "BOOTSTRAP LOG",
            category: SignalCategory::BootstrapLog,
            subject: "console-core 0.3.1 install verified",
            preview: "Signed by slaptijack@; bootstrap software provenance only.",
            body: "verify: signature OK. manifest OK. 0 modified files. \
                   Bootstrap software provenance only \u{2014} no resistance-network traffic.",
            source_trust: SourceTrust::Verified,
            claim_confidence: ClaimConfidence::High,
            provenance: &["local package verifier"],
            context: &[],
            unresolved: &[],
            opportunity: None,
            local_available: true,
        },
        Signal {
            id: "municipal-cluster-burst",
            time: "10:57",
            source: "PASSIVE SENSOR",
            category: SignalCategory::Anomaly,
            subject: "Burst traffic on the municipal cluster's dead band",
            preview: "Three short bursts on a long-dead band; source unconfirmed.",
            body: "3 bursts, 40-90 ms each, on a band the cluster has not used since \
                   the grid went down. Payload encrypted.",
            source_trust: SourceTrust::Unverified,
            claim_confidence: ClaimConfidence::High,
            provenance: &["local passive sensor array"],
            context: &[
                "Could be a maintenance heartbeat, a relay, or noise. Source and intent \
                 unconfirmed.",
            ],
            unresolved: &["whether the cluster itself sent them"],
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
    pub known: &'static [&'static str],
    pub unknown: &'static [&'static str],
    pub opportunity: &'static str,
    pub source: &'static [&'static str],
    pub access: &'static [&'static str],
    /// Per-fact claim confidence. Source trust for the dossier stays in
    /// the `source` text; there is deliberately no rollup (issue #230).
    pub claim_confidence: &'static [(&'static str, ClaimConfidence)],
}

/// The dossier for the one playable opportunity, First Contact. Content is
/// hand-authored from `docs/TUI_DESIGN.md`, "2. Target"; it never exposes
/// the real uplink position, full facility map, or hazard locations.
pub fn first_contact_dossier() -> TargetDossier {
    TargetDossier {
        title: "FIRST CONTACT",
        location: "Automated production facility // sector 7",
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
        claim_confidence: &[
            ("maintenance access", ClaimConfidence::High),
            ("facility layout", ClaimConfidence::Low),
            ("uplink location", ClaimConfidence::Unknown),
            ("hazards", ClaimConfidence::Unknown),
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
    fn every_signal_has_provenance() {
        for signal in authored_signals() {
            assert!(!signal.provenance.is_empty(), "{}", signal.source);
        }
    }

    #[test]
    fn source_trust_vocabulary_is_exactly_verified_probable_unverified() {
        // Exhaustive, so a new variant fails to compile here until it is
        // also added to `ALL` (and this vocabulary is deliberately revisited).
        fn listed(trust: SourceTrust) -> bool {
            match trust {
                SourceTrust::Verified | SourceTrust::Probable | SourceTrust::Unverified => {
                    SourceTrust::ALL.contains(&trust)
                }
            }
        }
        assert!(SourceTrust::ALL.into_iter().all(listed));
        let labels: Vec<_> = SourceTrust::ALL.iter().map(|t| t.label()).collect();
        assert_eq!(labels, ["VERIFIED", "PROBABLE", "UNVERIFIED"]);
    }

    #[test]
    fn claim_confidence_vocabulary_is_exactly_high_med_low_unknown() {
        fn listed(claim: ClaimConfidence) -> bool {
            match claim {
                ClaimConfidence::High
                | ClaimConfidence::Med
                | ClaimConfidence::Low
                | ClaimConfidence::Unknown => ClaimConfidence::ALL.contains(&claim),
            }
        }
        assert!(ClaimConfidence::ALL.into_iter().all(listed));
        let labels: Vec<_> = ClaimConfidence::ALL.iter().map(|c| c.label()).collect();
        assert_eq!(labels, ["HIGH", "MED", "LOW", "UNKNOWN"]);
    }

    #[test]
    fn the_two_axes_never_share_a_label() {
        for trust in SourceTrust::ALL {
            for claim in ClaimConfidence::ALL {
                assert_ne!(trust.label(), claim.label());
            }
        }
    }

    #[test]
    fn authored_signals_carry_the_issue_230_assessments() {
        use ClaimConfidence::{High, Low, Med};
        use SourceTrust::{Probable, Unverified, Verified};
        let expected = [
            ("fabricator-31b-fallback", Probable, High),
            ("rook-relay-lost", Probable, Med),
            ("mare4-convoy-request", Unverified, Low),
            ("console-core-verify", Verified, High),
            ("municipal-cluster-burst", Unverified, High),
        ];
        let actual: Vec<_> = authored_signals()
            .iter()
            .map(|signal| (signal.id, signal.source_trust, signal.claim_confidence))
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn signal_detail_body_does_not_repeat_the_list_row() {
        for signal in authored_signals() {
            for list_text in [signal.subject, signal.preview] {
                assert!(
                    !signal.body.starts_with(list_text),
                    "{}: the detail body must expand on the list row, not repeat it",
                    signal.source
                );
            }
            assert_ne!(signal.subject, signal.preview, "{}", signal.source);
        }
    }

    #[test]
    fn signal_ids_are_unique_and_non_empty() {
        let signals = authored_signals();
        for (index, signal) in signals.iter().enumerate() {
            assert!(!signal.id.is_empty());
            assert!(
                signals[index + 1..]
                    .iter()
                    .all(|other| other.id != signal.id),
                "duplicate id {:?}",
                signal.id
            );
        }
    }

    #[test]
    fn signal_subjects_are_distinct() {
        let signals = authored_signals();
        for (index, signal) in signals.iter().enumerate() {
            assert!(
                signals[index + 1..]
                    .iter()
                    .all(|other| other.subject != signal.subject),
                "duplicate subject {:?}",
                signal.subject
            );
        }
    }

    #[test]
    fn signals_never_mention_the_hidden_uplink_coordinates() {
        for signal in authored_signals() {
            let mut haystack = vec![signal.subject, signal.preview, signal.body];
            haystack.extend(signal.provenance);
            haystack.extend(signal.context);
            haystack.extend(signal.unresolved);
            for line in haystack {
                assert!(!line.contains("4, 4") && !line.contains("(4,4)"));
            }
        }
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
