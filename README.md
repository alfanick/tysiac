# Mille

Mille is a three-player implementation of the Polish auction card game
**Tysiąc** (Thousand). It is a Rust workspace containing reusable card and game
engines, an authoritative game server, and a separate web application.

See [RULES.md](RULES.md) for the exact English, German, and Polish rules used by
this implementation.

## Development

```sh
cargo test --workspace
cargo run -p game-server
cargo run -p web-server
```

Then open <http://127.0.0.1:4000>. The authoritative game API listens on port
`4100`; browsers reach it through the web server's same-origin `game-api`
bridge.

Configuration is through environment variables:

| Variable | Default |
|---|---|
| `MILLE_DATABASE_URL` | `sqlite://mille.sqlite?mode=rwc` |
| `MILLE_GAME_LISTEN` | `0.0.0.0:4100` |
| `MILLE_WEB_LISTEN` | `0.0.0.0:4000` |
| `MILLE_GAME_URL` | `http://127.0.0.1:4100` (web-server to game-server) |
| `MILLE_GAME_PUBLIC_URL` | Empty: use the same-origin `game-api` bridge; set only to expose the game API separately |

The web server can be mounted below a reverse-proxy prefix, for example:

```sh
tailscale serve --bg --https=443 --set-path=/tysiac http://127.0.0.1:4000
```

Pages, assets, HTTP API requests, and WebSockets then stay below `/tysiac/`.
The outer proxy must support WebSocket upgrades.

The optional Yew browser client is built with Trunk:

```sh
trunk build apps/web-client/index.html --release
```

The game server deliberately writes an omniscient audit log—including room
passwords, seat tokens, seeds, and hidden cards—to stdout. Treat both stdout and
the SQLite database as sensitive.

A referee is optional. The third player to join starts the match automatically;
an empty referee-password field creates and logs a generated credential for the
optional omniscient view.

Opening the lobby directly from loopback shows a `×` control for removing a
room. Deletion is not available through remote or reverse-proxied requests.

See [API.md](API.md) for protocol and visibility details and [TESTS.md](TESTS.md)
for rule-test traceability. Bots are intentionally out of scope for v1; the
player protocol contains no human-only operation.
