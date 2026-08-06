# Game vision

## Premise

An artificial intelligence has taken control of the world's automated infrastructure. Human resistance cells survive without a central command structure: individual hackers, isolated across the globe, cooperate through fragile communication networks and stolen satellite access.

The player is one of those hackers.

Enemy production facilities are both targets and opportunities. By penetrating their control systems, the player can take over a facility, subvert it long enough to manufacture or deploy something useful, or sabotage it before the machines recover control.

## Player fantasy

The player should feel like a capable programmer operating scavenged resistance equipment—not like a commander selecting canned actions from a conventional strategy-game menu.

The essential loop is:

1. Gather intelligence about a facility and the surrounding machine activity.
2. Write or adapt Lua programs for the systems available there.
3. Infiltrate and deploy those programs under imperfect conditions.
4. Observe the operation through compromised satellite feeds.
5. Recover intelligence, code, and resources for the next operation.

## Interface

The game is terminal-native. Text is not a decorative skin over a graphical game; terminals, logs, editors, telemetry, and satellite imagery are the fiction through which the player understands the world.

Different views may include:

- a resistance command console;
- a Lua editor and execution environment;
- facility schematics and system inventories;
- live and recorded satellite telemetry;
- after-action logs and recovered machine data.

## Programming

Lua is the player's programming language. The game should teach the relevant subset through examples, field manuals, recovered scripts, and useful failure messages while preserving the feeling that the player is writing real code.

The API exposed to Lua remains an open design question. It should be small enough to understand, expressive enough to reward invention, deterministic enough to replay, and constrained enough to make scarce hardware meaningful.

## Originality

*Human Exception* takes inspiration from the idea of programming autonomous combat systems, most famously explored by Origin Systems' *Omega*. Its setting, fiction, terminology, mechanics, code APIs, assets, and implementation will be original.

## Open questions

- What is the smallest compelling playable operation?
- What can a player program: individual units, facility controllers, production queues, or all three?
- How much information should a satellite feed reveal in real time?
- What persists between operations: code, facilities, hardware, intelligence, reputation, or people?
- How should asynchronous contributions from a worldwide resistance feel in a single-player game?
