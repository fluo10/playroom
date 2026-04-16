# playroom

A peer-to-peer session library for multiplayer tabletop games, built on [iroh](https://github.com/n0-computer/iroh). Designed so that humans and AI agents can play the same game together, including across machines.

## Motivation

The starting point is simple: I want to play tabletop games with AI agents. Not just local ones — agents running on a VPS (e.g. Openclaw-style setups) should be able to join too. playroom provides the session layer that makes those remote multiplayer sessions possible. [playtable](crates/playtable) is the reference application built on top of it.

## Design

### Session model

- Self-hosted server/client, in the spirit of Minecraft.
- A **Room** is the equivalent of a Minecraft "world": the unit of a session.
- Any node can host Rooms. Hosted Rooms become visible and joinable to trusted nodes.
- Rooms are fully owned by a single host. There is no host migration and no cross-host replication — if the host is offline, the Room is offline.
- Headless server deployments are a supported use case. Packaging/distribution of such servers is left to the application.

### Trust model

- Trusted peers are registered by their iroh `EndpointId`.
- When two nodes have mutually registered each other, they can see and join each other's Rooms.
- Trust is the only access control. playroom assumes communication happens between already-trusted devices, so there is no user authentication and no impersonation protection beyond that.
- Within a single Room, duplicate usernames are rejected so that users remain distinguishable.
  - A lightweight tripcode-style hash (à la 5ch) may be added later as a soft deduplication hint, but nothing stronger is planned.
- `EndpointId` exchange is manual for now (copy/paste, or QR on mobile). `EndpointId`s are always shared directly — no invite-link indirection.

### Clients: humans and AI

- **Humans** connect through a GUI client.
- **AI agents** connect through an MCP server exposed over **stdio only** (local process).
- The two UIs are asymmetric by nature, but the intent is that both surfaces expose the same capabilities — including Room management (create/join/leave/etc.), which AI agents can drive through MCP just like a human through the GUI.
- HTTP MCP is explicitly out of scope: it would require an additional auth layer that conflicts with the "trusted endpoints only" model. Remote AI agents are expected to run a local stdio MCP server on their own host and connect to playroom from there.

### Persistence

- playroom persists the **trusted `EndpointId` list** and whatever Room metadata it needs to operate.
- Game save data is *not* playroom's concern — it belongs to the application. (playtable, being a simple tabletop app, may not even have save data.)
- If save data replication between hosts is ever wanted, that is an application-level decision, not playroom's.

### Scope (current)

playroom today is deliberately minimal: transport, Room lifecycle, and the trust list. Notably *out of scope* for now:

- **Game state synchronization.** Applications are responsible for syncing their own state over the session. Once patterns emerge from building playtable (and hopefully other apps), common pieces may be promoted into playroom.
- **Chat.** Not implemented yet — for now, use Discord or similar out-of-band. Text chat at the playroom layer is a stretch goal once the core session work is stable.
- **User authentication / anti-impersonation.**
- **HTTP MCP.**
- **Host migration, save replication, backups.**

## Wire format and versioning

The plan is to try a fast zero-copy format like [rkyv](https://github.com/rkyv/rkyv) rather than chasing schema-level backward compatibility. Compatibility is instead handled at the session level: every connection carries an explicit `protocol_version`, and mismatched peers are rejected at handshake time. Trusted peers are expected to keep their playroom versions reasonably aligned.

Note that playroom targets turn-based / event-driven tabletop games, where ordinary request/response + event broadcast over a reliable stream is sufficient. Realtime-game techniques (client-side prediction, rollback, lockstep, unreliable channels, etc.) are not in scope.
