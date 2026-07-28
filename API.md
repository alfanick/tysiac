# Mille API v1

The `game-server` on port `4100` is authoritative. The `web-server` on port
`4000` serves pages, renders the legacy observer fallback, and forwards its
same-origin `/game-api/*` HTTP and WebSocket bridge to the game server. Modern
human clients use that bridge so TLS and reverse-proxy path prefixes are
preserved. Future native bots can talk directly to the game server with the
same protocol.

All bodies are UTF-8 JSON. Cards are structured:

```json
{"rank":"ace","suit":"hearts"}
```

A concealed card is always `{"visibility":"back"}`. A hidden face is never sent
with CSS used to cover it.

## HTTP

| Method and path | Authentication | Purpose |
|---|---|---|
| `GET /health` | none | Liveness |
| `GET /api/rooms` | none | Public room list |
| `POST /api/rooms` | none | Create room, player password, optional referee password/config/seed |
| `DELETE /api/rooms/{room}` | loopback client only | Permanently remove a room and its persisted state |
| `POST /api/rooms/{room}/join` | join password or seat token | Join/reconnect; the third new player starts the match automatically |
| `POST /api/rooms/{room}/leave` | seat token | Leave and compact seats before start |
| `GET /api/rooms/{room}/view` | role-dependent | Full role projection |
| `POST /api/rooms/{room}/action` | seat token | Submit a player action |
| `POST /api/rooms/{room}/admin` | referee password | Optional oversight: pause/resume/abort/rematch |
| `GET /ws/{room}` | query credential | Live snapshots and player commands |

Views use `role=observer`, or `role=player&seat=0&credential=TOKEN`, or
`role=referee&credential=PASSWORD`.

The referee role is never required for play. If room creation omits its
password or sends an empty value, the server generates a referee credential,
returns it to the creator, and writes it to stdout with the other room secrets.

Room deletion is deliberately machine-local. The web server exposes its `×`
control only to direct loopback clients and rejects non-loopback forwarded
addresses before the request reaches the game server. The game server performs
its own loopback check as a second boundary.

## Commands

Every changing command has a client-generated `command_id` and the last observed
`expected_revision`. Duplicate IDs are idempotent. A stale revision returns
HTTP 409 with `revision_conflict` and the current revision.

Player commands carry the same `tysiac::Action` enum for humans and future bots:
bid, pass, proof response/reveal, talon continuation/surrender, two gifts,
contract confirmation, card play, claim, and claim vote. `legal_actions` in a
player view contains concrete currently legal choices. The server still
validates every command in the rules engine.

WebSocket client messages are `authenticate`, `act`, `admin`, and `ping`.
Server messages are `snapshot`, `updated`, `error`, and `pong`. A newer
authenticated player WebSocket becomes the controlling connection; older
connections remain useful for reading snapshots but action attempts return
`superseded_connection`.

## Visibility

- Observer: public scores, phase, card counts, current/last trick, public talon,
  open claim hands, public history, and presentation stage.
- Player: observer view plus own hand and legal actions.
- Referee: complete match state, base seed, all round seed/deal audits, and all
  history including private transfers.

Public match-end history publishes each derived round seed and complete deal
order. The base seed remains referee-only so a future rematch cannot be
predicted.
