# Resistance console TUI design

This document defines the product behavior, information hierarchy, interaction rules, and representative layout for the first playable *Human Exception* resistance console.

It is the design contract for Epic #41. Implementation issues may choose appropriate Rust libraries, widgets, rendering primitives, execution-limit mechanisms, and internal architecture, but should not invent materially different player behavior without an explicit product decision.

## Design objective

The console should feel like equipment used by an independent hacker participating in a loose resistance network, not a mission terminal operated by a military command structure.

The player is not assigned work. The console surfaces things happening in the world: intercepted machine traffic, fragments published by other hackers, requests from local cells, infrastructure anomalies, and opportunities that may be worth exploiting. The player decides what deserves attention according to their own interests.

The primary loop is:

**notice → inspect → choose → edit → deploy → observe → learn → iterate or leave**

For the first playable slice there is only one real operation, First Contact. The interface may show other signals to establish the wider world, but it must not fake multiple playable choices. Only genuinely implemented opportunities are actionable.

The player should always be able to answer:

1. What am I looking at, and where did this information come from?
2. Why might I care about it?
3. What do I currently know, and what is still unknown?
4. What code am I about to deploy?
5. What exact code produced the run I am reviewing?
6. What did that code cause the machine to do?
7. What do I want to try next?

## Resistance model

The resistance is a confederation, not an institution.

There is no central command assigning missions, no quest giver, and no authoritative job board. Individual hackers and small cells act independently and share what they can when interests overlap.

The console therefore presents:

- intercepted machine traffic;
- intelligence published by other hackers;
- requests or questions from resistance cells;
- observations from compromised infrastructure;
- anomalies detected by the player's own systems;
- actionable opportunities inferred from one or more of those signals.

Use language such as **signal**, **intercept**, **shared intel**, **source**, **confidence**, **opportunity**, **target**, and **working set**.

Avoid language such as **mission assigned**, **orders**, **commander**, **quest**, **objective received**, or anything implying a central authority has decided what the player should do.

A target can still have an objective once the player chooses to act. The distinction is that the player voluntarily adopts that objective because the opportunity interests them.

## Target terminal

Supported minimum and primary design target: **120×40**. The console does not attempt partial gameplay below this geometry.

Every view uses a primary pane plus a contextual secondary pane, both always visible (§ [Responsive behavior](#responsive-behavior)).

Below 120 columns or 40 rows, do not silently truncate critical information. Show an in-world resize notice and preserve the ability to quit.

Color may reinforce state, but no information may depend on color alone. Unicode box drawing and symbols are welcome where supported; ASCII equivalents should remain possible. The identity is the resistance console and compromised telemetry, not any particular glyph set.

## Overall navigation

The major states are:

**Signals → Target → Controller → Operation → After Action**

From After Action, the player may return to **Controller** to iterate or **Signals** to direct their attention elsewhere.

**Help** is contextual and may be opened from any major state without destroying the underlying state.

Global navigation:

- `F1` Help
- `F2` Signals
- `F3` Target
- `F4` Controller
- `F5` Operation
- `F6` Deploy / redeploy current controller
- `Ctrl+Q` Quit

Function keys are intentional because they remain available while editing Lua and do not steal ordinary source-text keystrokes.

`F3`, `F4`, `F5`, and `F6` may be unavailable until their prerequisite state exists. Unavailable actions should be visibly disabled rather than silently ignored.

Some views bind additional local keys beyond this global set. `F7` resets the controller in Controller. `F8` moves focus to the next pane in the current view — see [Pane focus](#pane-focus). These are documented with the views they apply to.

### Navigation while a deployment is active

An active deployment does **not** continue executing invisibly in the background when the player leaves Operation.

- `F1` opens Help without changing execution state. If the run was running, presentation and execution pause while Help is open and resume when Help is dismissed. If it was already paused, it remains paused.
- `F5` returns to Operation and preserves the current paused/running state.
- `F2`, `F3`, or `F4` while a run is active first pauses the run, then navigates. Returning to Operation leaves the run paused so the player can inspect state before explicitly resuming with `Space`.
- Navigating away never cancels the run and never advances simulation ticks in the background.
- `F6` starts a new run from a clean scenario state. If another run is active, require confirmation before replacing it.

This rule keeps the console understandable: no simulation state changes while the player is looking somewhere else.

### Quit safety

Controller source is session-only in this epic, so modified source must not be discarded accidentally.

- `Ctrl+Q` exits immediately only when the current controller is unmodified or no working set exists.
- If the controller is modified, `Ctrl+Q` opens a confirmation that explicitly states the edits will be lost because cross-launch persistence is not implemented.
- If a run is active, the same confirmation also states that the active run will be abandoned.
- Confirming exits and restores terminal state; cancelling returns to the prior view without changing source or simulation state.

## Pane focus

Every multi-pane view has exactly one focused pane at a time. This section is
the interaction contract for that focus: which panes exist, which is focused by
default, how `F8` moves between them, how focus interacts with other input, and
how it persists. It governs *player-visible behavior only* — it does not
prescribe a `PaneId` enum, a widget registry, a trait hierarchy, or any other
implementation mechanism. That is left to #82 and later issues.

### Panes per view

Each current view has one or two named panes:

| View | Panes |
| --- | --- |
| Help | Help |
| Signals | Signals list, Selected signal |
| Target | Target intelligence, Provenance/access |
| Controller | Controller source, Lua field reference |
| Operation | Satellite feed, Operation telemetry |
| After Action | Report, Final satellite frame |

No current view has more than two panes.

### Default focus

| View | Default focused pane |
| --- | --- |
| Help | Help |
| Signals | Signals list |
| Target | Target intelligence |
| Controller | Controller source |
| Operation | Satellite feed |
| After Action | Report |

### F8 — next pane

`F8` moves focus to the next pane in the current view; both panes of a
two-pane view remain visible throughout (§ [Responsive
behavior](#responsive-behavior)), so `F8` only moves which one is focused. In
Help, which has a single pane, `F8` is inert. The same is true whenever the
currently rendered surface doesn't actually present a second pane to move
to: Operation before anything has ever been deployed, and After Action
before anything has ever been deployed, are each a single placeholder pane,
not the two-pane composition their row in [Panes per view](#panes-per-view)
describes once a deployment exists — the focus marker and the footer's `F8`
hint are both absent there too. (An active, not-yet-finished deployment
viewed via After Action is a different case: both panes already render real
content there, so that remains an ordinary two-pane view with `F8` and the
marker both working normally.) The same placeholder-style inertness applies
while a confirmation dialog (quit, controller-reset, or deployment-replace)
is pending: `F8` is swallowed along with every other non-dialog key, and the
footer hint is suppressed for as long as the dialog is open, even in a view
that is otherwise two-pane.

### Input priority

Input is resolved in this order:

1. **Global safety/modal controls** — quit confirmation, controller-reset
   confirmation, deployment-replace confirmation.
2. **Global and view-level commands** — `F1`–`F7`, and view-level bindings such
   as Target's `Enter`/`Esc` or Operation's `Space`/`Enter`.
3. **Focus movement** — `F8`.
4. **Focused-pane input** — input that only the currently focused pane
   interprets.

A modal confirmation is never hidden behind an unfocused pane; if Controller
source is modified, the reset confirmation triggered by `F7` remains visible
regardless of which pane is currently focused.

### Non-color focus cue

Whenever a view currently renders more than one focusable pane, the focused
pane's title carries a non-color marker (for example, a leading `>`) so focus
is identifiable without relying on color. Bold or color styling may reinforce
the cue but must never be the only way to identify it. The exact glyph and
styling are a rendering concern for a later issue; this contract only
requires that some non-color cue exists and identifies exactly one pane
whenever such a choice actually exists.

The cue is absent, by design, wherever [F8 — next pane](#f8--next-pane)
documents focus movement as unavailable: Help, the Operation/After Action
placeholders before anything has ever been deployed, and a pending
confirmation dialog. None of those surfaces present a real choice between
panes, so a marker there would claim a choice that doesn't exist.

### Focus persistence

- Focus is remembered independently per view. Leaving a view and returning
  restores that view's last focused pane.
- Opening Help (`F1`) and dismissing it never changes the underlying view's
  remembered focus.
- A fresh After Action result always focuses the Report pane, regardless of
  what was focused the last time After Action was visited, so the outcome
  hierarchy (§5) is what the player sees immediately, without requiring
  `F8`.

### Read-only panes and unsupported input

A pane with no pane-local input today (Target's two panes, Operation's
satellite feed) may still be focused, since focus determines input ownership
and the non-color cue independently of any pane having something to do.
Pressing a key with no pane-local meaning in the focused pane stays inert.
This contract does not manufacture new pane-local interactions merely so
that every pane has something to do.

### Pane-local vs. view-level input today

| View | Pane-local input | View-level input |
| --- | --- | --- |
| Help | Up/Down scroll (its one pane) | — |
| Signals | Up/Down select, Enter activate (Signals list); none (Selected signal) | — |
| Target | none | Enter (work opportunity), Esc (back) |
| Controller | Editing, cursor movement, selection, undo/redo, indent/unindent, typed text, paste (Controller source); none (Lua field reference) | Ctrl+V validate, F6 deploy, F7 reset |
| Operation | none (either pane) | Space (pause/resume), Enter (step) |
| After Action | Up/Down scroll (Report); none (Final satellite frame) | F2, F4, F5, F6 |

This table is a description of existing behavior, not a change to it. It exists
so implementation issues know, for each key, whether focus should gate it.

## Persistent frame

Every major view uses the same outer frame, but the header reflects whether the player has merely observed an opportunity or has chosen to work it.

Before selecting a target:

```text
┌ HUMAN EXCEPTION // RESISTANCE CONSOLE ───────────────────────────────────────────────────────────────────────────┐
│ MESH: DEGRADED        SATLINK: COMPROMISED        SIGNALS: 04        WORKING SET: none                           │
├──────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                                                  │
│                                            ACTIVE WORKSPACE                                                      │
│                                                                                                                  │
├──────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ F1 Help   F2 Signals   F3 Target   F4 Controller   F5 Operation   F6 Deploy                           Ctrl+Q Quit│
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

After the player chooses an opportunity:

```text
┌ HUMAN EXCEPTION // RESISTANCE CONSOLE ───────────────────────────────────────────────────────────────────────────┐
│ MESH: DEGRADED   WORKING SET: FIRST CONTACT   LINK: COMPROMISED   CONTROLLER: modified   STATUS: READY           │
├──────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                                                  │
│                                            ACTIVE WORKSPACE                                                      │
│                                                                                                                  │
├──────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ F1 Help   F2 Signals   F3 Target   F4 Controller   F5 Operation   F6 Deploy                           Ctrl+Q Quit│
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

The header should communicate state, not decorate it.

Persistent header priorities:

1. resistance-network / link condition;
2. current working set, if any;
3. controller state (`starter`, `modified`, `invalid`), when relevant;
4. operation state (`READY`, `RUNNING`, `PAUSED`, `SUCCESS`, `FAILED`), when relevant.

The phrase **working set** is intentional: choosing an opportunity means “this is what I am working on now,” not “this mission has been assigned to me.”

## 1. Signals

Signals is the default screen after startup.

It should feel like a living intelligence stream assembled from unreliable, decentralized sources. It is not a clean list of quests.

Representative 120×40 layout:

```text
┌ HUMAN EXCEPTION // RESISTANCE CONSOLE ─────────────────────────────────────────────────────────────────────────┐
│ MESH: DEGRADED        SATLINK: COMPROMISED        SIGNALS: 04        WORKING SET: none                         │
├───────────────────────────────────────────────────────────────┬────────────────────────────────────────────────┤
│ SIGNALS                                                       │ SELECTED SIGNAL                                │
│                                                               │                                                │
│ > 11:42  MACHINE INTERCEPT                                    │ MACHINE INTERCEPT // sector 7                  │
│   Fabricator node 31B resumed local control after mesh loss.  │ confidence: HIGH                               │
│   auth state inconsistent.                              [OPEN]│                                                │
│                                                               │ A maintenance unit inside an automated facility│
│   11:35  rook@pacific // SHARED INTEL                         │ appears to have fallen back to local control.  │
│   “Lost my relay before I could trace the uplink.             │ Authentication traffic is incomplete.          │
│    Dumping what I saw in case somebody is closer.”            │                                                │
│                                                               │ Correlated fragments suggest a temporary access│
│   11:18  CELL/MARE-4 // REQUEST                               │ window through the captured maintenance drone. │
│   Looking for anyone who can identify convoy routing          │                                                │
│   changes near old I-5. No clean telemetry yet.               │ ACTIONABLE: FIRST CONTACT                      │
│                                                               │ Enter  inspect opportunity                     │
│   10:57  PASSIVE SENSOR // ANOMALY                            │                                                │
│   Burst traffic from an offline municipal control cluster.    │                                                │
│                                                               │                                                │
├────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ ↑↓ Select   Enter Inspect   F1 Help   F2 Signals                                             Ctrl+Q Quit       │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### Signals behavior

The left pane is chronological intelligence, not a task list. Items may be actionable, informational, unresolved, or merely atmospheric.

The right pane shows the currently selected signal in more detail and may correlate related fragments.

For the first playable slice:

- show several authored signals from different sources;
- only First Contact is actionable;
- do not make non-actionable signals selectable as fake missions;
- it is acceptable for them to be inspectable as information, but they must clearly lack a deployable opportunity;
- at least one signal should come from another independent hacker or cell;
- at least one signal should be machine-generated or passively intercepted.

The actionable marker should be restrained. `[OPEN]` or an equivalent status is preferable to a bright “NEW QUEST” affordance.

Selecting an actionable signal and pressing `Enter` opens **Target**. It does not automatically commit the player to acting.

If the player already has First Contact as the current working set, selecting it again must preserve the current controller source. Re-entering Target never reloads or replaces source by itself.

## 2. Target

Target is the dossier for an opportunity the player is considering or has chosen to work.

It should answer: what is known, how do we know it, what is uncertain, what access is available, and what could be gained by acting?

Representative layout:

```text
┌ HUMAN EXCEPTION // RESISTANCE CONSOLE ─────────────────────────────────────────────────────────────────────┐
│ MESH: DEGRADED        TARGET: FIRST CONTACT        CONFIDENCE: MED/HIGH        WORKING SET: none           │
├───────────────────────────────────────────────────────────────┬────────────────────────────────────────────┤
│ TARGET INTELLIGENCE                                           │ PROVENANCE / ACCESS                        │
│                                                               │                                            │
│ FIRST CONTACT                                                 │ SOURCE                                     │
│ Automated production facility // sector 7                     │ machine intercept + shared fragment        │
│                                                               │                                            │
│ KNOWN                                                         │ ACCESS                                     │
│ • one maintenance drone responds to our control channel       │ captured maintenance controller            │
│ • facility map is incomplete                                  │ compromised satellite feed                 │
│ • a local network uplink exists somewhere inside              │                                            │
│ • drone endurance is limited                                  │ CONFIDENCE                                 │
│                                                               │ maintenance access      HIGH               │
│ UNKNOWN                                                       │ facility layout         LOW                │
│ • uplink location                                             │ uplink location         UNKNOWN            │
│ • complete floor plan                                         │ hazards                 UNKNOWN            │
│ • hazard locations                                            │                                            │
│                                                               │ OPPORTUNITY                                │
│ If the drone reaches the uplink, we may obtain a foothold     │ No one is waiting for us to do this.       │
│ in the facility network before the access window closes.      │ The window may not last.                   │
│                                                               │                                            │
│ Enter  work this opportunity                                  │ Esc  back to signals                       │
├────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ F1 Help   F2 Signals   Enter Work Opportunity                                                   Ctrl+Q Quit│
└────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### Target information model

A target dossier may contain:

- **provenance** — where the information came from;
- **confidence** — how trustworthy a claim appears;
- **known facts** — information already established;
- **unknowns** — meaningful gaps the player must account for;
- **available access/hardware** — what the player can currently control or observe;
- **constraints** — budget, link stability, time window in fiction, or other operation limits;
- **opportunity** — why acting could be useful or interesting.

Do not reveal hidden authoritative scenario state. Specifically, First Contact must not expose the concealed uplink position, full facility map, or undiscovered hazard positions.

### Choosing to work an opportunity

Pressing `Enter` on **work this opportunity** creates the current working set and loads the starter controller **only when First Contact is not already the active working set**.

If First Contact is already the working set, `Enter` returns to the existing Controller state and preserves all edits. It must not silently reload the starter or replace source.

A future design that intentionally replaces an active working set must either preserve that set's source or ask for explicit confirmation before discarding modified source.

This is the point at which the UI transitions from passive intelligence gathering to active preparation.

The player may always return to Signals later. Choosing an opportunity is not accepting an irrevocable quest.

## 3. Controller

The Controller view is available after the player chooses to work an opportunity.

The editor owns most of the screen. API help is secondary.

```text
┌ HUMAN EXCEPTION // RESISTANCE CONSOLE ────────────────────────────────────────────────────────────────────────────┐
│ WORKING SET: FIRST CONTACT   LINK: COMPROMISED   CONTROLLER: modified   STATUS: READY                             │
├────────────────────────────────────────────────────────────────────────────┬──────────────────────────────────────┤
│ CAPTURED CONTROLLER // first_contact.lua                                   │ LUA FIELD REFERENCE                  │
│                                                                            │                                      │
│  1  local scanned = false                                                  │ on_tick(observation)                 │
│  2                                                                         │                                      │
│  3  function on_tick(observation)                                          │ observation.drone.x / .y             │
│  4    local budget = observation.budget_remaining                          │ observation.budget_remaining         │
│  5    if not scanned and budget > 1 then                                   │ observation.discovered               │
│  6      scanned = true                                                     │                                      │
│  7      return "scan"                                                      │ return:                              │
│  8    end                                                                  │ north south east west                │
│  9                                                                         │ wait scan                            │
│ 10    -- choose what the drone should do using observation                 │                                      │
│ 11    return "wait"                                                        │ F1 opens complete reference          │
│ 12  end                                                                    │                                      │
├───────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ F1 Help   F2 Signals   F3 Target   F4 Controller   F5 Operation   F6 Deploy   F7 Reset                 Ctrl+Q Quit│
└───────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

Product requirements:

- Controller source is the default focused pane the first time this view opens for a working set; returning to Controller later restores whichever pane was last focused there, per [Pane focus](#pane-focus);
- line numbers are visible but not part of the source;
- source survives navigation between views;
- modified state is visible in the persistent header;
- a terminal paste (the terminal's own paste action, not a bound key) inserts the pasted text at the cursor in one operation, preserving embedded newlines and whitespace;
- `F7` restores the starter controller, with confirmation if edits would be lost;
- Lua syntax/load validation errors appear adjacent to the source with useful line information where available;
- the sidebar is a cheat sheet, not a complete manual;
- no mouse is required.

### The editor contract

This section is the interaction contract for the Controller source editor
itself: which keys do what, in what priority, and what state survives which
action. The editor is backed by an embedded terminal code-editor foundation
(`ratatui-code-editor`, selected in #90) wrapped by the authoritative
`ControllerDocument` adapter (`src/console/document.rs`); `EditOp`
(`src/console/editor.rs`) is the shared vocabulary of edit operations the
Controller input handling dispatches onto it. This section governs
*player-visible behavior only* — it does not prescribe an undo-stack
representation or other implementation detail beyond what
`ControllerDocument` already exposes.

#### Minimum editor experience

- Grapheme-safe cursor movement: `Left`/`Right`/`Up`/`Down`, `Home`/`End`,
  `PageUp`/`PageDown`, and a word-movement pair, `Ctrl+Left`/`Ctrl+Right`.
  Vertical movement remembers the column the player last moved to
  horizontally, so moving through a shorter line and back doesn't forget how
  far right the cursor was.
- The vertical and horizontal viewport auto-scrolls to keep the cursor
  visible, accounting for wide/combining characters — with one accepted,
  temporary exception; see [Known limitation: wide-glyph cursor
  visibility](#known-limitation-wide-glyph-cursor-visibility) below. There is
  no separately persisted scroll offset for this pane — the visible viewport
  is always derived from the current cursor position, so it never needs
  independent resetting; contrast the Help and After-Action-report panes,
  which do remember an explicit scroll offset because they have no cursor to
  derive one from.
- Line numbers are visible but are not part of the source and are never
  included in a copy or in what is validated/deployed.
- The cursor is visibly rendered whenever Controller source is the focused
  pane, with the same accepted, temporary exception as the viewport
  guarantee above; see [Known limitation: wide-glyph cursor
  visibility](#known-limitation-wide-glyph-cursor-visibility).
- **Selection:** `Shift`+any cursor-movement key (arrows, `Home`/`End`,
  `PageUp`/`PageDown`, word movement) extends a selection from an anchor at
  the point `Shift` was first held; `Ctrl+A` selects the whole document.
  Typing a character, `Tab`, or pressing `Backspace`/`Delete` while a
  selection is active replaces the selection rather than acting at the
  cursor alone.
- **Undo/redo:** `Ctrl+Z` undoes and `Ctrl+Y` redoes, stepping backward and
  forward through discrete edits — typed insertion, `Backspace`/`Delete`, a
  pasted block, and a selection replacement each count as one undoable
  step. `Ctrl+Shift+Z` is not bound: without an extended keyboard protocol
  it can arrive indistinguishable from plain `Ctrl+Z` and silently undo
  instead of redo, the same reasoning that keeps `Ctrl+V` (not
  `Ctrl+Enter`) as validate's advertised binding.
- `Tab` indents the current line, or every line an active selection
  touches, by one language-appropriate unit as ordinary space characters —
  two spaces for Lua — never a literal tab byte in the source;
  `Shift+Tab` removes one indent unit the same way. This preserves today's
  decision to avoid mixing tabs and spaces in player-visible source, while
  moving from exactly one space to a real indent width.
- Comfortable editing of long lines and of programs larger than the starter:
  no arbitrary line-length or document-size cap.
- Lua syntax highlighting is desirable but is **not required** for this
  epic, matching #88. No Lua syntax highlighting exists in the current
  implementation; a reliable, unhighlighted editor is preferable to a
  fragile grammar integration.

#### Known limitation: wide-glyph cursor visibility

`ratatui-code-editor` 0.0.6 (adopted in #90) has one accepted, temporary gap
in both the viewport guarantee and the visible-cursor requirement stated
above: `Editor::focus()` decides whether to scroll horizontally by comparing
the cursor's raw *character-count* column against the viewport's
*terminal-cell* width, while `get_visible_cursor()` correctly computes the
visual, grapheme-width-based column. Whenever a line's visual width exceeds
its character count by enough to cross the viewport boundary, character
count under-counts the true visual width, so `focus()` can conclude no scroll
is needed while the cursor is actually off-screen and unrendered.

**Affected:** cursor-visibility-follows-viewport (and, as a direct
consequence, the visible-cursor requirement) for any line long enough to
require horizontal scrolling where wide (double-width) glyphs push the
line's true visual width past the viewport's terminal-cell width — whether
the line is made entirely of wide glyphs or is a mix of ordinary and wide
characters (for example, a Lua comment or string containing CJK text after
enough ASCII columns).

**Unaffected:**

- ordinary ASCII long lines requiring horizontal scroll — cursor stays
  visible (`long_line_scrolling_follows_the_cursor`,
  `tests/editor_foundation_contract.rs`);
- short wide-glyph or combining-mark content that fits within the viewport
  without scrolling — cursor stays visible
  (`combining_marks_keep_cursor_visible_after_focus`,
  `tests/editor_foundation_contract.rs`;
  `unicode_wide_glyphs_and_combining_marks_render_with_a_visible_cursor`,
  `src/console/ui.rs`, at the Controller level);
- exact Unicode source *content* round-tripping, independent of cursor
  visibility or line width
  (`unicode_combining_marks_and_wide_glyphs_round_trip`,
  `exact_source_round_trip_including_empty_and_trailing_newline`,
  `tests/editor_foundation_contract.rs`).

This is accepted for #88/#90 and tracked upstream, independent of and not
blocking Human Exception:
[vipmax/ratatui-code-editor#15](https://github.com/vipmax/ratatui-code-editor/issues/15)
confirms the root cause, and
[vipmax/ratatui-code-editor#16](https://github.com/vipmax/ratatui-code-editor/pull/16)
proposes a fix. The characterization test
`known_limitation_wide_glyph_line_can_leave_cursor_offscreen_after_focus` in
`tests/editor_foundation_contract.rs` asserts today's actual (broken)
behavior on purpose — including that `focus()` leaves `get_offset_x()` at
`0` — so it fails conspicuously, rather than silently, the moment an
upstream fix changes that behavior. No Human Exception-specific cursor or
viewport workaround should be added for this without a separate decision.

Once an upstream release contains the fix, resolve this by:

1. opening a small dependency-upgrade issue;
2. updating `ratatui-code-editor` to the released version containing the fix;
3. replacing the characterization assertion in
   `known_limitation_wide_glyph_line_can_leave_cursor_offscreen_after_focus`
   with the desired visible-cursor regression assertion — both the
   `get_offset_x() == 0` assertion (a fixed `focus()` must scroll, so this
   offset will become nonzero) and the `is_none()` assertion (which becomes
   `is_some()`) need to change together, not just the latter;
4. verifying the behavior through both the foundation contract test and
   Controller `TestBackend` coverage;
5. removing this subsection from this document.

#### Command priority

Controller's editor keys follow the console-wide [Input priority](#input-priority)
order already established in [Pane focus](#pane-focus): confirmation dialogs
first, then global/view-level commands, then `F8` focus movement, then
focused-pane input. Selection, undo/redo, `Tab`, and paste do not change
that order — they are all still focused-pane input, exactly like plain
typing today, and so remain unavailable whenever `Controller source` isn't
the focused pane or Controller isn't the open view.

Restating which Controller commands are pane-local versus view-level (this
extends, and must stay consistent with, the existing
[Pane-local vs. view-level input today](#pane-local-vs-view-level-input-today)
table):

- **View-level** (available whenever Controller is open, regardless of
  which of its two panes is focused): `Ctrl+V`/optional `Ctrl+Enter`
  (validate), `F6` (deploy), `F7` (reset).
- **Pane-local to Controller source:** all cursor movement, selection,
  typed insertion, `Backspace`/`Delete`, `Tab`, undo/redo, and a terminal
  paste.
- **Pane-local to the Lua field reference pane:** none — it remains
  read-only, as today.

#### `Ctrl+V` and the optional `Ctrl+Enter` alias

`Ctrl+V` is the advertised, guaranteed validate binding: an ordinary control
character every terminal reports correctly, regardless of keyboard protocol
support. `Ctrl+Enter` may continue to work as an unadvertised alias on
terminals that can report it distinctly from plain `Enter` (for example via
the Kitty keyboard protocol); it is not the primary path, because most
terminals without that protocol report it identically to plain `Enter`,
which would otherwise insert a newline instead of validating. Whatever
editor foundation #90 selects, its default keymap must not take ownership of
`Ctrl+V` for its own purpose (e.g. a library default of "paste").

#### Bracketed paste

A terminal-initiated paste (the terminal's own paste action, not a bound
key) is accepted only when all of the following hold: Controller is the
open view, `Controller source` is the focused pane, no confirmation dialog
is pending, and the terminal is not below the supported minimum geometry.
Outside that state the paste is silently ignored rather than applied to the
wrong place. Line endings are normalized (CRLF/CR to LF) before insertion.
The pasted text is inserted at the cursor in a single operation, preserving
embedded newlines and whitespace exactly, and counts as one atomic step for
undo purposes — undoing a paste removes the whole block at once, not one
character at a time.

#### `F7` reset — state after confirmed restoration

`F7` restores the starter controller, with confirmation if edits would be
lost (unmodified source resets immediately with no prompt). After a
confirmed reset:

- the source becomes the exact starter controller text;
- the cursor moves to the end of the restored source (there is no stored
  "starter cursor position" to return to instead);
- any active selection is cleared;
- the viewport scrolls so the cursor is visible, as an ordinary consequence
  of the viewport always being cursor-derived (see
  [Minimum editor experience](#minimum-editor-experience));
- undo/redo history is cleared — restoring the starter is an intentional,
  confirmed discard of the prior modified state, not something the player
  can step back through with undo;
- validation reverts to unchecked.

While the reset confirmation is pending, it takes priority over all editor
input — including selection and undo/redo keys — exactly as the existing
[Input priority](#input-priority) rule already requires for every other
Controller key. If the Lua field reference pane happened to be focused when
`F7` was pressed, the confirmation still renders in the source pane (since
that is where the banner renders) without changing which pane is actually
focused; focus itself is restored to whatever the player had once the
dialog resolves.

#### Modified state and validation invalidation

"Modified" means the current source differs from the starter controller
text — it is not a separate dirty flag and is not tied to undo-history
depth, so a sequence of edits that nets back to exactly the starter text is
unmodified again. A prior `Ctrl+V` validation result (`READY`/`INVALID`) is
invalidated back to unchecked only by an edit that actually changes the
source's content. Pure cursor movement, a selection that is created but
never used to mutate anything, and an edit that has no effect (e.g.
`Backspace` at the start of the document) must not silently clear a
`READY`/`INVALID` banner that is still accurate.

#### Deployed-source provenance

`F6` deploy snapshots the exact current working source into the run record
at the moment of deployment. Later edits to the working copy — including
undo/redo — never retroactively change that snapshot, and **Review Run**
always shows the frozen deployed source for that run, never whatever source
currently happens to be in the editor. This must hold regardless of which
editor foundation #90 selects: an editor library must expose, or be wrapped
to expose, an exact-string snapshot of its current content.

#### Interaction-state matrix

| State | `Ctrl+V` validate | `F6` deploy | `F7` reset | Typing / cursor movement | Selection | Undo/redo | Paste |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Source pane focused, no dialog pending | available | available | available | accepted, mutates | accepted | accepted, mutates | accepted, mutates |
| Reference pane focused | available | available | available | inert | inert | inert | inert (silently ignored) |
| Reset / quit / redeploy confirmation pending | inert (dialog swallows all non-dialog keys) | inert | inert (already open) | inert | inert | inert | inert |
| Below minimum geometry (<120×40) | inert (only the geometry warning and `Ctrl+Q` are live) | inert | inert | inert | inert | inert | inert |

`Ctrl+V`/`F6`/`F7` availability in every row follows one authoritative rule —
Controller is the open view, the terminal is not below minimum geometry, and
no confirmation dialog is pending — independent of which pane is focused,
because they are view-level. Typing/selection/undo/paste availability
follows a second, different authoritative rule — the same three conditions,
**plus** `Controller source` specifically being the focused pane — because
they are pane-local. Neither rule is restated per-command; every row above
is a consequence of applying one of those two rules, per
[Command priority](#command-priority).

#### Explicitly out of scope for this contract

LSP, autocomplete/completion, diagnostics beyond load/syntax validation,
formatting, refactoring, configurable keymaps, full Vim/Emacs emulation,
mouse-first editing, multiple buffers or files, and a filesystem browser.
Lua syntax highlighting is desirable but not required.

### Starter controller

The first-play starter must be **useful but intentionally incomplete**.

It should demonstrate:

- the `on_tick(observation)` callback;
- reading at least one observation field in executable code;
- at least one scan;
- persistent Lua state outside the callback;
- a clearly understandable place for the player to change behavior.

The representative source above reads `observation.budget_remaining` so a copied implementation cannot accidentally teach a callback that ignores its input.

It should not be the checked-in reference controller that automatically solves First Contact. The point of the first playable loop is to give the player code worth modifying, not to press Deploy on a finished solution.

## 4. Operation

Operation is the live view of the currently deployed controller.

The compromised satellite feed is visually dominant. Telemetry exists to explain why the player's program behaved as it did.

```text
┌ HUMAN EXCEPTION // RESISTANCE CONSOLE ────────────────────────────────────────────────────────────────────────────┐
│ WORKING SET: FIRST CONTACT   LINK: LIVE   CONTROLLER: modified   STATUS: RUNNING                                  │
├──────────────────────────────────────────────────────────────────────┬────────────────────────────────────────────┤
│ COMPROMISED SATELLITE FEED                                           │ OPERATION TELEMETRY                        │
│                                                                      │                                            │
│              ?   ?   ?   ?   ?                                       │ tick          04                           │
│              .   #   ?   ?   ?                                       │ budget        11 / 15                      │
│              .   #   ?   ?   ?                                       │ last action   north                        │
│              .   .   .   ?   ?                                       │ controller    running                      │
│              ·   #   ?   ?   ?                                       │                                            │
│                                                                      │ RECENT EVENTS                              │
│                   ▲ DRONE                                            │ 04  moved north                            │
│                                                                      │ 03  discovered wall                        │
│ discovered: 9 tiles                                                  │ 02  moved north                            │
│ signal reconstruction: partial                                       │ 01  scan completed                         │
│                                                                      │                                            │
│ ? unknown   · floor   # wall   ~ hazard   U uplink                   │ Space pause                                │
│                                                                      │ Enter step (paused)                        │
├───────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ F1 Help   F2 Signals   F3 Target   F4 Controller   F5 Operation   F6 Redeploy   Space Pause            Ctrl+Q Quit│
└───────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

The sample budget is mechanically valid: four ordinary completed actions from a 15-point budget leave 11.

### Satellite-feed rules

The feed must show only information the player has legitimately discovered through authoritative simulation state.

Never reveal:

- undiscovered terrain;
- the concealed uplink location;
- undiscovered hazard positions;
- other authoritative state unavailable through the established observation rules.

Unknown remains unknown.

Do not show coordinate axes by default. Coordinates exist in the Lua API and may appear in diagnostics or help, but the primary visual should feel like a facility being reconstructed from compromised telemetry, not a matrix debugger.

The glyphs above are illustrative. Implementation may use stronger Unicode terminal graphics, borders, spacing, and color. Required semantic distinctions are:

- unknown;
- traversable floor;
- wall;
- hazard;
- uplink;
- drone.

### Pacing controls

- `Space` pauses/resumes execution and presentation.
- `Enter` advances exactly one tick while paused and remains paused afterward.

Simulation truth must remain deterministic and independent of wall-clock presentation timing.

`F6` during or after a run starts a fresh deployment with the current controller. If a deployment is already active, require confirmation before replacing it.

### Runaway Lua and responsiveness

Player Lua is untrusted input. Valid syntax does not guarantee that evaluation returns.

The interactive console must remain recoverable when Lua runs forever or consumes an unreasonable amount of execution, including:

- an infinite loop during top-level script evaluation;
- an infinite loop inside `on_tick`;
- equivalent non-returning or excessive execution paths.

The product requirement is observable, not architectural:

- each script-load/evaluation phase and each controller callback must have a bounded execution policy or an equivalent cancellation mechanism;
- exceeding that bound ends the deployment with a controller execution-limit/cancelled failure rather than freezing the UI indefinitely;
- the terminal event loop remains responsive enough for the player to reach Controller or quit;
- the player's current editor source remains intact;
- the failure is shown with a concise explanation that the controller exceeded its execution allowance and should be revised;
- the exact instruction-count, hook, thread, process, or other implementation mechanism is left to implementation issues.

Runtime/script failures end the deployment the same as any other terminal outcome: the console transitions to After Action (§5) with a concise error explanation and **Controller** presented as the obvious recovery path. **Review Run** (`F5` from After Action) shows that same explanation alongside the finished run's frozen telemetry. §5's outcome hierarchy and grammar apply to a controller failure exactly as they do to budget exhaustion or success.

### Run records and source provenance

Every deployment creates an immutable run record that includes, directly or by immutable revision identity:

- the exact controller source deployed for that run;
- the scenario/working-set identity;
- authoritative events and final result;
- discovered state needed to review the run.

Editing the current controller after a run must not change the source associated with the recorded run. **Review Run** must therefore be able to answer “which code produced this behavior?” even after the working copy has changed.

The UI may show a compact revision identifier in After Action or Review Run; the storage mechanism is an implementation choice.

## 5. After Action

After Action is an operation state, not a disconnected popup.

The final discovered satellite frame remains visible so the player can connect the result to what the program actually did.

### Outcome hierarchy

Every terminal state — success, budget exhaustion, or controller error — presents these concepts in this order. Higher items must never be sacrificed for lower ones when space is constrained (see [Responsive behavior](#responsive-behavior)):

1. **Outcome** — the headline result (`FOOTHOLD ESTABLISHED`, `OPERATION FAILED`, etc.).
2. **Trigger** — the concrete mechanical event that produced it (the drone reached the uplink; the budget ran out; the controller failed to behave as programmed).
3. **Meaning** — what the trigger means in the resistance's fiction, stated without overclaiming.
4. **Completion** — whether First Contact is complete or incomplete, stated explicitly.
5. **Evidence** — the final satellite frame and the supported run facts that explain the result (§ Evidence: After Action vs. Review Run, below).
6. **Next actions** — Review Run, revise/redeploy the Controller, or return to Signals.
7. **Availability** — a truthful statement that no additional playable operation is currently implemented.

This hierarchy is a presentation contract, not a new state machine: it governs what a given After Action screen says and in what order, not new navigation, new bindings, or new mechanical outcomes.

### Success

A successful run leads with `FOOTHOLD ESTABLISHED` and states `FIRST CONTACT COMPLETE` as the explicit completion line. The meaning line echoes the language already used when the opportunity was first offered in Target (§2): reaching the uplink means resistance access to the facility network was established before the operational window closed. It does **not** mean the facility was captured, owned, brought under resistance control, or made persistently operable — the report must not claim or imply any of those.

The availability line is stated plainly and in-fiction, not as an apologetic developer note: no further operation at this facility is currently available. Returning to Signals is worthwhile because Signals is the wider intelligence network, not because another operation at this target is waiting.

```text
┌ HUMAN EXCEPTION // RESISTANCE CONSOLE ────────────────────────────────────────────────────────────────────────┐
│ WORKING SET: FIRST CONTACT   LINK: RECORDED   CONTROLLER: modified   STATUS: SUCCESS                          │
├──────────────────────────────────────────────────────────────────────┬────────────────────────────────────────┤
│ FINAL SATELLITE FRAME                                                │ AFTER-ACTION REPORT                    │
│                                                                      │                                        │
│              .   .   ?   ?   ?                                       │ FOOTHOLD ESTABLISHED                   │
│              .   #   ?   ?   ?                                       │ The drone reached the facility uplink. │
│              .   #   #   ?   ?                                       │ Resistance access to the facility      │
│              .   .   .   #   ▲                                       │ network was established before the    │
│              ·   #   ?   ?   ?                                       │ access window closed.                 │
│                                                                      │                                        │
│                        ▲ DRONE (AT UPLINK)                            │ FIRST CONTACT COMPLETE                 │
│                                                                      │                                        │
│                                                                      │ ticks executed     11                  │
│                                                                      │ tiles discovered   14                  │
│                                                                      │ hazards entered     0                  │
│                                                                      │ deployed rev       run-08             │
│                                                                      │                                        │
│                                                                      │ No further operation is available at  │
│                                                                      │ this facility. Review the run, redeploy│
│                                                                      │ to try another approach, or return to │
│                                                                      │ Signals for the wider network.        │
├───────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ F2 Signals   F4 Edit Controller   F5 Review Run   F6 Redeploy                                      Ctrl+Q Quit│
└───────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### Failure and controller error

Budget exhaustion and controller errors (an execution-limit breach, an invalid action, a script or runtime error) are all "operation failed" outcomes and follow the same result → cause → completion → evidence → recovery grammar as success, without expanding into a broader redesign of failure systems:

- **Outcome:** `OPERATION FAILED`.
- **Trigger:** a concise, mechanical explanation of what actually happened — budget exhaustion, or the controller failing to behave as programmed — stated without prescribing the exact fix. The explanation names the mechanical reason (e.g. "the controller exceeded its execution allowance") without walking through internal implementation detail.
- **Meaning:** the operational window closed without the drone reaching the uplink, so no facility foothold was established on this attempt. This is the failure counterpart to the success meaning line, not a new mechanic — it states a consequence that already follows from the trigger.
- **Completion:** First Contact is explicitly **incomplete**. This is the direct counterpart to `FIRST CONTACT COMPLETE` and must be no less clear.
- **Evidence/next actions:** identical evidence set and next-action set as success (below), except the primary recovery path is revising the Controller rather than reviewing a clean success.
- **Availability:** the same truthful statement as success — no additional playable operation is currently implemented at this facility, regardless of outcome.

```text
┌ HUMAN EXCEPTION // RESISTANCE CONSOLE ────────────────────────────────────────────────────────────────────────┐
│ WORKING SET: FIRST CONTACT   LINK: RECORDED   CONTROLLER: modified   STATUS: FAILED                           │
├──────────────────────────────────────────────────────────────────────┬────────────────────────────────────────┤
│ FINAL SATELLITE FRAME                                                │ AFTER-ACTION REPORT                    │
│                                                                      │                                        │
│              .   .   ?   ?   ?                                       │ OPERATION FAILED                       │
│              .   #   ?   ?   ?                                       │ Operational budget exhausted.          │
│              .   #   ?   ?   ?                                       │                                        │
│              .   .   .   ▲   ~                                       │ FIRST CONTACT INCOMPLETE               │
│              ·   #   ?   ?   ?                                       │                                        │
│                                                                      │ ticks executed     15                  │
│                   ▲ DRONE                                             │ tiles discovered   12                  │
│                                                                      │ hazards entered     1                  │
│                                                                      │ deployed rev       run-07             │
│                                                                      │                                        │
│                                                                      │ No further operation is available at  │
│                                                                      │ this facility either way. Revise the  │
│                                                                      │ controller and try again, or return   │
│                                                                      │ to Signals.                           │
├───────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ F2 Signals   F4 Edit Controller   F5 Review Run   F6 Redeploy                                      Ctrl+Q Quit│
└───────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

A controller error (e.g. an execution-limit breach) uses the same layout with a headline naming the mechanical failure, for example `OPERATION FAILED: controller execution limit`, followed by `FIRST CONTACT INCOMPLETE` and the same evidence/next-action shape. The exact set of controller-error headlines is an implementation detail; this document only guarantees that every such headline is a specific, mechanical `OPERATION FAILED: ...` variant rather than a generic message, and that it is followed by the same completion/evidence/recovery grammar shown above.

Failure explanations should state the mechanical reason without prescribing the exact solution.

The player's edited working source remains intact. Returning to Controller restores the same document state. Redeploy starts from a clean scenario state.

Returning to Signals does not imply abandoning or failing a formal assignment. It simply means the player has chosen to direct their attention elsewhere.

### Evidence: After Action vs. Review Run

The initial After Action screen carries only the evidence needed to understand the result: final budget/ticks executed, tiles discovered, hazards entered, the run identifier (e.g. `deployed rev run-07`), and — where space allows — the final satellite frame. This evidence set is the same regardless of outcome.

Detailed tick-by-tick telemetry, full event chronology, and the exact deployed-source provenance are **Review Run**'s responsibility (`F5` from After Action), not After Action's. After Action answers "what happened and what does it mean"; Review Run answers "show me exactly what the code did." Neither view exposes hidden authoritative scenario state (§2, [Target information model](#target-information-model)) beyond what the run legitimately discovered.

### Review Run

Review Run is a finished `Operation` — the same two-pane layout (compromised satellite feed, run inspector) the live run used, not a new view. It is reachable the same way live Operation is, via `F5`, once the deployment it shows has concluded.

A single **selected review point** drives both panes at once: the satellite feed renders that point's legitimate discovered snapshot, and the run inspector renders that same point's evidence. The two panes can never describe two different moments in the run. A review point is one of:

- `INITIAL` — the legitimate pre-tick observation, with no action (no action was ever taken to reach it).
- a completed tick — the recorded controller action, the resulting drone position, remaining budget against the starting budget, tiles newly discovered relative to the previous point, and any structured action-cost/hazard events from that tick.
- a terminal failure boundary — a controller/runtime failure that ended the run without completing another tick. It carries the last known position and budget (the preceding point's), never a fabricated action or discovery, and is presented as clearly distinct from the tick that preceded it, naming which tick that was.

The point that is the run's actual terminal boundary additionally carries the terminal evidence for its kind: the success or budget-exhaustion outcome headline, or the controller-failure explanation.

A **zero-tick deployment/load failure** (the controller source itself never loaded) has no completed ticks and no pre-tick observation — nothing was ever legitimately discovered. Review Run states this explicitly (`NO RECORDED SATELLITE EXECUTION STATE`) rather than rendering any frame, including one derived from the fixed scenario's own public starting facts; inventing a frame here would misrepresent what the run actually observed.

The run inspector renders a compact **chronology index** above the selected point's evidence: one row per review point (`INITIAL`, `TICK 01`, `TICK 07 [SUCCESS]`, `FAILURE (after tick 02)`, and so on), with the selected row marked. While the run inspector pane is focused on a finished run:

- `Up` / `Down` — select the previous/next review point, clamped at the first/last point (never wraps).
- `PageUp` / `PageDown` — move backward/forward by one visible chronology page (as many rows as the index currently shows).
- `Home` / `End` — jump straight to the first/terminal review point (a semantic chronology jump, not viewport scrolling).

The chronology index's visible window always includes the selected row, derived purely from the selection and the index's visible-row count — there is no independently scrollable viewport to fall out of sync with it. `F8` pane focus, global `F`-key navigation, and live Operation's `Space`/`Enter` pacing controls are unaffected: chronology navigation only applies once the run inspector is focused and the run has finished.

Browsing the complete deployed source (a separate `SOURCE` mode for the run inspector, reached via `Tab`) is not yet implemented.

### Next actions

From After Action, four next actions are available, using bindings already defined in [Overall navigation](#overall-navigation) — no new bindings are introduced for this contract:

- `F2` — return to Signals.
- `F4` — edit the Controller (current source, including edits from prior sessions this run, is preserved).
- `F5` — Review Run: inspect this run's frozen telemetry and deployed source.
- `F6` — redeploy from a clean scenario state.

**Review Run** displays the immutable source revision and telemetry associated with that recorded run, not whatever source currently happens to be in the editor.

## 6. Help

`F1` opens contextual help as an overlay or dedicated view. It preserves the underlying state and returns to that state when dismissed.

Help has two levels:

1. current-view controls and immediate concepts first;
2. complete Lua contract, signal terminology, and symbol reference available by scrolling.

Do not dump the README into the application.

Help should explain unfamiliar fiction terms in conventional language. The fiction should make the interface more legible, not force the player to decode jargon before they can use it.

When opened during a running deployment, Help temporarily pauses execution as defined in [Navigation while a deployment is active](#navigation-while-a-deployment-is-active).

## Responsive behavior

### 120+ columns

Use the two-pane compositions shown above; both panes always render, and
`F8` moves which one carries the focus marker and input ownership (see
[Pane focus](#pane-focus)). It remains safe in Controller because it does
not insert a normal text character into Lua source.

For After Action specifically, the Report pane is focused by default (§
[Pane focus](#pane-focus)): the player sees outcome, trigger, meaning, and
completion (hierarchy items 1–4) immediately, without needing `F8` to
discover whether they succeeded or what that meant.

### Below 120 columns or 40 rows

Display an in-world geometry warning instead of an unusable compressed interface:

```text
HUMAN EXCEPTION // resistance console
Terminal link degraded.
Minimum console geometry: 120x40
Current geometry: 100x30
Resize the terminal to restore the resistance console.
```

Quitting remains available, subject to the same modified-source confirmation rule.

## State and information rules

Persistent across the application session:

- authored/current Signals state;
- selected signal;
- current working set, if one has been chosen;
- current Lua source and modified/reset state;
- immutable run record for the most recent deployment, including deployed source or revision identity;
- most recent deployment result;
- current run paused/running state;
- navigation target;
- help dismissal return target.

Reset for every deployment:

- simulation state;
- discovery state;
- budget;
- event history for the new run;
- success/failure state.

Leaving a working set for Signals does not erase the controller during this epic. If the player returns to First Contact in the same application session, their edits are preserved. Re-selecting or re-opening the same opportunity must reuse the current source rather than silently loading the starter again.

Persistence across application launches remains out of scope, which is why quit confirmation is required for modified source.

## Visual hierarchy

The current activity determines what dominates the screen.

- **Signals:** the intelligence stream dominates; detail is secondary.
- **Target:** knowns/unknowns and opportunity dominate; provenance is secondary.
- **Controller:** Lua source dominates; API reference is secondary.
- **Operation:** satellite feed dominates; telemetry is secondary.
- **After Action:** final frame and mechanical result share attention.

Do not create a permanent dashboard containing all panes at once. That would dilute the player's current task and make a 120-column terminal feel cramped despite its size.

## Visual tone

The interface should feel **functional, scarce, improvised, and technically credible**.

It may feel military-adjacent because the resistance repurposes hostile infrastructure, but it should not feel like a military chain-of-command system.

Avoid gratuitous cyberpunk decoration:

- random hex dumps;
- Matrix-style rain;
- constant glitch effects;
- fake technical jargon that obscures real gameplay information;
- decorative warning spam.

Fiction should clarify the interface:

- `COMPROMISED SATELLITE FEED` is better than `MAP`;
- `CAPTURED CONTROLLER` is better than `EDITOR`;
- `AFTER-ACTION REPORT` is better than `RESULT DIALOG`;
- `SHARED INTEL` is better than `QUEST FROM NPC`.

But conventional meaning must remain recognizable. A player should never have to decode the fiction just to learn how to operate the console.

## Opportunity design principles beyond First Contact

Future opportunities should reinforce player identity through voluntary specialization.

The Signals model should eventually support players who repeatedly choose to pursue different interests, for example:

- reconnaissance and intelligence gathering;
- infrastructure compromise or capture;
- sabotage;
- resource acquisition;
- helping another resistance cell;
- autonomous machine programming;
- production or deployment opportunities.

The UI must not assume all future opportunities are “missions” with the same structure. Signals are world events and information first; actionable gameplay emerges from what the player decides to exploit.

This epic does **not** implement those future systems. It establishes the product grammar that can accommodate them honestly later.

## Implementation boundary

This specification defines **product behavior, player flow, terminology, safety/recovery expectations, and visual hierarchy**.

It does not dictate:

- the Rust TUI library;
- widget composition;
- render-loop architecture;
- event-dispatch architecture;
- terminal backend;
- exact Unicode glyphs;
- exact colors;
- the internal mechanism used to bound or cancel runaway Lua.

#42 and later issues may make those implementation choices as long as the observable experience remains consistent with this document.

Material deviations from the Signals → Target → Controller → Operation → After Action flow, autonomy model, navigation semantics, safety/recovery behavior, layout hierarchy, or first-play experience are product-design changes and should not be introduced opportunistically during implementation.
