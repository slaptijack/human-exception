# Resistance console TUI design

This document defines the product behavior, information hierarchy, and representative layout for the first playable *Human Exception* resistance console.

It is the design contract for Epic #41. Implementation issues may choose appropriate Rust libraries, widgets, rendering primitives, and internal architecture, but should not invent a materially different player flow without an explicit product decision.

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
5. What did that code cause the machine to do?
6. What do I want to try next?

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

Primary design target: **120×40**.

Supported minimum: **80×24**.

At 100+ columns, views may use a primary pane plus a contextual secondary pane. At 80–99 columns, secondary information becomes a switchable subview rather than compressing the primary content into an unreadable layout.

Below 80 columns or 24 rows, do not silently truncate critical information. Show an in-world resize notice and preserve the ability to quit.

Color may reinforce state, but no information may depend on color alone. Unicode box drawing and symbols are welcome where supported; ASCII equivalents should remain possible. The identity is the resistance console and compromised telemetry, not any particular glyph set.

## Overall navigation

The major states are:

**Signals → Target → Controller → Operation → After Action**

From After Action, the player may return to **Controller** to iterate or **Signals** to disengage and look elsewhere.

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

## Persistent frame

Every major view uses the same outer frame, but the header reflects whether the player has merely observed an opportunity or has chosen to work it.

Before selecting a target:

```text
┌ HUMAN EXCEPTION // RESISTANCE CONSOLE ─────────────────────────────────────────────────────────────────────────────┐
│ MESH: DEGRADED        SATLINK: COMPROMISED        SIGNALS: 04        WORKING SET: none                              │
├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                                                     │
│                                            ACTIVE WORKSPACE                                                         │
│                                                                                                                     │
├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ F1 Help   F2 Signals   F3 Target   F4 Controller   F5 Operation   F6 Deploy                           Ctrl+Q Quit    │
└─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

After the player chooses an opportunity:

```text
┌ HUMAN EXCEPTION // RESISTANCE CONSOLE ─────────────────────────────────────────────────────────────────────────────┐
│ MESH: DEGRADED   WORKING SET: FIRST CONTACT   LINK: COMPROMISED   CONTROLLER: modified   STATUS: READY              │
├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                                                     │
│                                            ACTIVE WORKSPACE                                                         │
│                                                                                                                     │
├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ F1 Help   F2 Signals   F3 Target   F4 Controller   F5 Operation   F6 Deploy                           Ctrl+Q Quit    │
└─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
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
┌ HUMAN EXCEPTION // RESISTANCE CONSOLE ─────────────────────────────────────────────────────────────────────────────┐
│ MESH: DEGRADED        SATLINK: COMPROMISED        SIGNALS: 04        WORKING SET: none                              │
├───────────────────────────────────────────────────────────────┬─────────────────────────────────────────────────────┤
│ SIGNALS                                                       │ SELECTED SIGNAL                                     │
│                                                               │                                                     │
│ > 11:42  MACHINE INTERCEPT                                    │ MACHINE INTERCEPT // sector 7                       │
│   Fabricator node 31B resumed local control after mesh loss.  │ confidence: HIGH                                    │
│   auth state inconsistent.                              [OPEN] │                                                     │
│                                                               │ A maintenance unit inside an automated facility     │
│   11:35  rook@pacific // SHARED INTEL                         │ appears to have fallen back to local control.        │
│   “Lost my relay before I could trace the uplink.             │ Authentication traffic is incomplete.               │
│    Dumping what I saw in case somebody is closer.”            │                                                     │
│                                                               │ Correlated fragments suggest a temporary access     │
│   11:18  CELL/MARE-4 // REQUEST                               │ window through the captured maintenance drone.      │
│   Looking for anyone who can identify convoy routing          │                                                     │
│   changes near old I-5. No clean telemetry yet.               │ ACTIONABLE: FIRST CONTACT                            │
│                                                               │ Enter  inspect opportunity                           │
│   10:57  PASSIVE SENSOR // ANOMALY                             │                                                     │
│   Burst traffic from an offline municipal control cluster.    │                                                     │
│                                                               │                                                     │
├───────────────────────────────────────────────────────────────┴─────────────────────────────────────────────────────┤
│ ↑↓ Select   Enter Inspect   F1 Help   F2 Signals                                             Ctrl+Q Quit           │
└─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
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

## 2. Target

Target is the dossier for an opportunity the player is considering or has chosen to work.

It should answer: what is known, how do we know it, what is uncertain, what access is available, and what could be gained by acting?

Representative layout:

```text
┌ HUMAN EXCEPTION // RESISTANCE CONSOLE ─────────────────────────────────────────────────────────────────────────────┐
│ MESH: DEGRADED        TARGET: FIRST CONTACT        CONFIDENCE: MED/HIGH        WORKING SET: none                    │
├───────────────────────────────────────────────────────────────┬─────────────────────────────────────────────────────┤
│ TARGET INTELLIGENCE                                           │ PROVENANCE / ACCESS                                 │
│                                                               │                                                     │
│ FIRST CONTACT                                                 │ SOURCE                                              │
│ Automated production facility // sector 7                     │ machine intercept + shared fragment                 │
│                                                               │                                                     │
│ KNOWN                                                         │ ACCESS                                              │
│ • one maintenance drone responds to our control channel       │ captured maintenance controller                    │
│ • facility map is incomplete                                  │ compromised satellite feed                         │
│ • a local network uplink exists somewhere inside              │                                                     │
│ • drone endurance is limited                                  │ CONFIDENCE                                          │
│                                                               │ maintenance access      HIGH                       │
│ UNKNOWN                                                       │ facility layout         LOW                        │
│ • uplink location                                             │ uplink location         UNKNOWN                    │
│ • complete floor plan                                         │ hazards                 UNKNOWN                    │
│ • hazard locations                                            │                                                     │
│                                                               │ OPPORTUNITY                                         │
│ If the drone reaches the uplink, we may obtain a foothold     │ No one is waiting for us to do this.                │
│ in the facility network before the access window closes.      │ The window may not last.                            │
│                                                               │                                                     │
│ Enter  work this opportunity                                  │ Esc  back to signals                                │
├───────────────────────────────────────────────────────────────┴─────────────────────────────────────────────────────┤
│ F1 Help   F2 Signals   Enter Work Opportunity                                                   Ctrl+Q Quit        │
└─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
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

Pressing `Enter` on **work this opportunity** creates the current working set and loads the starter controller.

This is the point at which the UI transitions from passive intelligence gathering to active preparation.

The player may always return to Signals later. Choosing an opportunity is not accepting an irrevocable quest.

## 3. Controller

The Controller view is available after the player chooses to work an opportunity.

The editor owns most of the screen. API help is secondary.

```text
┌ HUMAN EXCEPTION // RESISTANCE CONSOLE ─────────────────────────────────────────────────────────────────────────────┐
│ WORKING SET: FIRST CONTACT   LINK: COMPROMISED   CONTROLLER: modified   STATUS: READY                              │
├────────────────────────────────────────────────────────────────────────────┬────────────────────────────────────────┤
│ CAPTURED CONTROLLER // first_contact.lua                                  │ LUA FIELD REFERENCE                    │
│                                                                            │                                        │
│  1  local scanned = false                                                  │ on_tick(observation)                   │
│  2                                                                         │                                        │
│  3  function on_tick(observation)                                          │ observation.drone.x / .y               │
│  4    if not scanned then                                                  │ observation.budget_remaining           │
│  5      scanned = true                                                     │ observation.discovered                 │
│  6      return "scan"                                                      │                                        │
│  7    end                                                                  │ return:                                │
│  8                                                                         │ north south east west                  │
│  9    -- choose what the drone should do next                              │ wait scan                              │
│ 10    return "wait"                                                        │                                        │
│ 11  end                                                                    │ F1 opens complete reference            │
│                                                                            │                                        │
├────────────────────────────────────────────────────────────────────────────┴────────────────────────────────────────┤
│ F1 Help   F2 Signals   F3 Target   F4 Controller   F5 Operation   F6 Deploy   F7 Reset                 Ctrl+Q Quit  │
└─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

Product requirements:

- the editor is focused when this view opens;
- line numbers are visible but not part of the source;
- source survives navigation between views;
- modified state is visible in the persistent header;
- `F7` restores the starter controller, with confirmation if edits would be lost;
- Lua syntax/load validation errors appear adjacent to the source with useful line information where available;
- the sidebar is a cheat sheet, not a complete manual;
- no mouse is required.

### Starter controller

The first-play starter must be **useful but intentionally incomplete**.

It should demonstrate:

- the `on_tick(observation)` callback;
- access to observations;
- at least one scan;
- persistent Lua state outside the callback;
- a clearly understandable place for the player to change behavior.

It should not be the checked-in reference controller that automatically solves First Contact. The point of the first playable loop is to give the player code worth modifying, not to press Deploy on a finished solution.

## 4. Operation

Operation is the live view of the currently deployed controller.

The compromised satellite feed is visually dominant. Telemetry exists to explain why the player's program behaved as it did.

```text
┌ HUMAN EXCEPTION // RESISTANCE CONSOLE ─────────────────────────────────────────────────────────────────────────────┐
│ WORKING SET: FIRST CONTACT   LINK: LIVE   CONTROLLER: modified   STATUS: RUNNING                                   │
├──────────────────────────────────────────────────────────────────────┬──────────────────────────────────────────────┤
│ COMPROMISED SATELLITE FEED                                          │ OPERATION TELEMETRY                          │
│                                                                      │                                              │
│              ?   ?   ?   ?   ?                                     │ tick          04                            │
│              .   #   ?   ?   ?                                     │ budget        10 / 15                       │
│              .   #   ?   ?   ?                                     │ last action   north                         │
│              .   .   .   ?   ?                                     │ controller    running                       │
│              ·   #   ?   ?   ?                                     │                                              │
│                                                                      │ RECENT EVENTS                                │
│                   ▲ DRONE                                            │ 04  moved north                              │
│                                                                      │ 03  discovered wall                          │
│ discovered: 9 tiles                                                  │ 02  moved north                              │
│ signal reconstruction: partial                                      │ 01  scan completed                           │
│                                                                      │                                              │
│ ? unknown   · floor   # wall   ~ hazard   U uplink                   │ Space pause                                 │
│                                                                      │ Enter step (paused)                         │
├──────────────────────────────────────────────────────────────────────┴──────────────────────────────────────────────┤
│ F1 Help   F2 Signals   F3 Target   F4 Controller   F5 Operation   F6 Redeploy   Space Pause            Ctrl+Q Quit │
└─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

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

- `Space` pauses/resumes presentation.
- `Enter` advances one tick while paused.

Simulation truth must remain deterministic and independent of wall-clock presentation timing.

`F6` during or after a run starts a fresh deployment with the current controller. If a deployment is already active, require a simple confirmation before replacing it.

Runtime/script failures remain in the Operation view, with the telemetry pane becoming an error explanation and **Controller** presented as the obvious recovery path.

## 5. After Action

After Action is an operation state, not a disconnected popup.

The final discovered satellite frame remains visible so the player can connect the result to what the program actually did.

```text
┌ HUMAN EXCEPTION // RESISTANCE CONSOLE ─────────────────────────────────────────────────────────────────────────────┐
│ WORKING SET: FIRST CONTACT   LINK: RECORDED   CONTROLLER: modified   STATUS: FAILED                                │
├──────────────────────────────────────────────────────────────────────┬──────────────────────────────────────────────┤
│ FINAL SATELLITE FRAME                                                │ AFTER-ACTION REPORT                          │
│                                                                      │                                              │
│              .   .   ?   ?   ?                                     │ OPERATION FAILED                             │
│              .   #   ?   ?   ?                                     │ Operational budget exhausted.                │
│              .   #   ?   ?   ?                                     │                                              │
│              .   .   .   .   ~                                     │ ticks executed     15                        │
│              ·   #   ?   ?   ?                                     │ tiles discovered   12                        │
│                                                                      │ hazards entered     1                         │
│                                                                      │                                              │
│                                                                      │ The controller is unchanged.                │
│                                                                      │ Revise it and try again, or return to        │
│                                                                      │ Signals and work on something else.          │
├──────────────────────────────────────────────────────────────────────┴──────────────────────────────────────────────┤
│ F2 Signals   F4 Edit Controller   F5 Review Run   F6 Redeploy                                      Ctrl+Q Quit     │
└─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

Success uses the same structure with a clear `OPERATION SUCCESSFUL` result and relevant statistics.

Failure explanations should state the mechanical reason without prescribing the exact solution.

The player's edited source remains intact. Returning to Controller restores the same document state. Redeploy starts from a clean scenario state.

Returning to Signals does not imply abandoning or failing a formal assignment. It simply means the player has chosen to direct their attention elsewhere.

## 6. Help

`F1` opens contextual help as an overlay or dedicated view. It preserves the underlying state and returns to that state when dismissed.

Help has two levels:

1. current-view controls and immediate concepts first;
2. complete Lua contract, signal terminology, and symbol reference available by scrolling.

Do not dump the README into the application.

Help should explain unfamiliar fiction terms in conventional language. The fiction should make the interface more legible, not force the player to decode jargon before they can use it.

## Responsive behavior

### 100+ columns

Use the two-pane compositions shown above.

### 80–99 columns

Use one primary pane. Secondary information becomes a toggled subview:

- Signals ↔ selected-signal detail;
- Target intelligence ↔ provenance/access;
- Controller ↔ Lua reference;
- Satellite feed ↔ telemetry;
- final satellite frame ↔ after-action report.

The footer shows the key used to toggle the secondary view.

### Below 80 columns or 24 rows

Display an in-world geometry warning instead of an unusable compressed interface:

```text
HUMAN EXCEPTION // resistance console
Terminal link degraded.
Minimum console geometry: 80x24
Current geometry: 72x20
Resize the terminal to restore the resistance console.
```

Quitting remains available.

## State and information rules

Persistent across the application session:

- authored/current Signals state;
- selected signal;
- current working set, if one has been chosen;
- current Lua source and modified/reset state;
- most recent deployment result;
- navigation target;
- help dismissal return target.

Reset for every deployment:

- simulation state;
- discovery state;
- budget;
- event history for the new run;
- success/failure state.

Leaving a working set for Signals does not need to erase the controller during this epic. If the player returns to First Contact in the same application session, preserving their edits is preferable to surprising data loss. Persistence across application launches remains out of scope.

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

This specification defines **product behavior, player flow, terminology, and visual hierarchy**.

It does not dictate:

- the Rust TUI library;
- widget composition;
- render-loop architecture;
- event-dispatch architecture;
- terminal backend;
- exact Unicode glyphs;
- exact colors.

#42 and later issues may make those implementation choices as long as the observable experience remains consistent with this document.

Material deviations from the Signals → Target → Controller → Operation → After Action flow, autonomy model, layout hierarchy, or first-play experience are product-design changes and should not be introduced opportunistically during implementation.