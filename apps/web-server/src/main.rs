use std::{
    env,
    fmt::Write as _,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{
        ConnectInfo, OriginalUri, Path, State,
        ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, Method, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{any, get},
};
use futures_util::{SinkExt, StreamExt};
use mille_protocol::{PublicCard, PublicView, View};
use serde::Deserialize;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite};
use tower_http::services::ServeDir;

#[derive(Clone)]
struct AppState {
    game_internal: String,
    game_public: String,
    client: reqwest::Client,
}

#[derive(Deserialize)]
struct RoomSummary {
    name: String,
    seats: Vec<String>,
    lifecycle: mille_protocol::Lifecycle,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    let listen = env::var("MILLE_WEB_LISTEN").unwrap_or_else(|_| "0.0.0.0:4000".into());
    let state = Arc::new(AppState {
        game_internal: env::var("MILLE_GAME_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:4100".into()),
        // An empty public URL makes browsers use this server's same-origin
        // /game-api bridge. That works on LANs, behind TLS, and below a
        // reverse-proxy path prefix.
        game_public: env::var("MILLE_GAME_PUBLIC_URL").unwrap_or_default(),
        client: reqwest::Client::new(),
    });
    let app = Router::new()
        .route("/", get(lobby))
        .route("/room/{room}", get(observer))
        .route("/room/{room}/referee", get(referee))
        .route("/room/{room}/player/{seat}/{name}", get(player))
        .route("/game-api/ws/{room}", get(proxy_websocket))
        .route("/game-api/{*path}", any(proxy_http))
        .nest_service("/static", ServeDir::new("apps/web-server/static"))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    eprintln!("web-server listening on http://{listen}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

async fn proxy_http(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(path) = uri.path().strip_prefix("/game-api") else {
        return proxy_error("Invalid game API proxy path");
    };
    if is_room_delete(&method, path) && !request_is_local(peer, &headers) {
        return local_only_error();
    }
    let mut upstream_url = format!("{}{path}", state.game_internal.trim_end_matches('/'));
    if let Some(query) = uri.query() {
        upstream_url.push('?');
        upstream_url.push_str(query);
    }

    let mut request = state.client.request(method, upstream_url);
    for (name, value) in &headers {
        if !is_hop_by_hop(name.as_str()) {
            request = request.header(name, value);
        }
    }
    let upstream = match request.body(body).send().await {
        Ok(response) => response,
        Err(error) => {
            eprintln!("game API proxy request failed: {error}");
            return proxy_error("Game server unavailable");
        }
    };

    let status = upstream.status();
    let response_headers = upstream.headers().clone();
    let bytes = match upstream.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("game API proxy response failed: {error}");
            return proxy_error("Invalid game server response");
        }
    };
    let mut response = Response::builder().status(status);
    if let Some(headers) = response.headers_mut() {
        for (name, value) in &response_headers {
            if !is_hop_by_hop(name.as_str()) {
                headers.append(name, value.clone());
            }
        }
    }
    match response.body(Body::from(bytes)) {
        Ok(response) => response,
        Err(error) => {
            eprintln!("game API proxy could not build response: {error}");
            proxy_error("Invalid game server response")
        }
    }
}

async fn proxy_websocket(
    State(state): State<Arc<AppState>>,
    Path(room): Path<String>,
    OriginalUri(uri): OriginalUri,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Some(websocket_base) = websocket_base(&state.game_internal) else {
        return proxy_error("Game server URL must use HTTP or HTTPS");
    };
    let mut upstream_url = format!(
        "{}/ws/{}",
        websocket_base.trim_end_matches('/'),
        urlencoding::encode(&room)
    );
    if let Some(query) = uri.query() {
        upstream_url.push('?');
        upstream_url.push_str(query);
    }
    let (upstream, _) = match connect_async(&upstream_url).await {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!("game WebSocket proxy connection failed: {error}");
            return proxy_error("Game server WebSocket unavailable");
        }
    };
    upgrade
        .on_upgrade(move |downstream| bridge_websockets(downstream, upstream))
        .into_response()
}

async fn bridge_websockets(
    downstream: WebSocket,
    upstream: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
) {
    let (mut downstream_sender, mut downstream_receiver) = downstream.split();
    let (mut upstream_sender, mut upstream_receiver) = upstream.split();

    loop {
        tokio::select! {
            from_browser = downstream_receiver.next() => {
                let Some(Ok(message)) = from_browser else {
                    break;
                };
                let Some(message) = to_tungstenite_message(message) else {
                    break;
                };
                if upstream_sender.send(message).await.is_err() {
                    break;
                }
            }
            from_game = upstream_receiver.next() => {
                let Some(Ok(message)) = from_game else {
                    break;
                };
                let Some(message) = to_axum_message(message) else {
                    break;
                };
                if downstream_sender.send(message).await.is_err() {
                    break;
                }
            }
        }
    }

    let _ = upstream_sender.close().await;
    let _ = downstream_sender.close().await;
}

fn to_tungstenite_message(message: AxumWsMessage) -> Option<tungstenite::Message> {
    match message {
        AxumWsMessage::Text(text) => Some(tungstenite::Message::Text(text.to_string().into())),
        AxumWsMessage::Binary(bytes) => Some(tungstenite::Message::Binary(bytes.to_vec().into())),
        AxumWsMessage::Ping(bytes) => Some(tungstenite::Message::Ping(bytes.to_vec().into())),
        AxumWsMessage::Pong(bytes) => Some(tungstenite::Message::Pong(bytes.to_vec().into())),
        AxumWsMessage::Close(_) => None,
    }
}

fn to_axum_message(message: tungstenite::Message) -> Option<AxumWsMessage> {
    match message {
        tungstenite::Message::Text(text) => Some(AxumWsMessage::Text(text.to_string().into())),
        tungstenite::Message::Binary(bytes) => Some(AxumWsMessage::Binary(bytes.to_vec().into())),
        tungstenite::Message::Ping(bytes) => Some(AxumWsMessage::Ping(bytes.to_vec().into())),
        tungstenite::Message::Pong(bytes) => Some(AxumWsMessage::Pong(bytes.to_vec().into())),
        tungstenite::Message::Close(_) | tungstenite::Message::Frame(_) => None,
    }
}

fn websocket_base(http_base: &str) -> Option<String> {
    http_base
        .strip_prefix("https://")
        .map(|rest| format!("wss://{rest}"))
        .or_else(|| {
            http_base
                .strip_prefix("http://")
                .map(|rest| format!("ws://{rest}"))
        })
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "host"
            | "content-length"
    )
}

fn proxy_error(message: &str) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        serde_json::json!({
            "type": "error",
            "code": "proxy_unavailable",
            "message": message,
        })
        .to_string(),
    )
        .into_response()
}

fn local_only_error() -> Response {
    (
        StatusCode::FORBIDDEN,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        serde_json::json!({
            "type": "error",
            "code": "local_only",
            "message": "Room removal is only available from localhost",
        })
        .to_string(),
    )
        .into_response()
}

fn is_room_delete(method: &Method, path: &str) -> bool {
    if method != Method::DELETE {
        return false;
    }
    path.strip_prefix("/api/rooms/")
        .is_some_and(|room| !room.is_empty() && !room.contains('/'))
}

fn request_is_local(peer: SocketAddr, headers: &HeaderMap) -> bool {
    address_is_loopback(peer.ip())
        && plain_forwarded_header_is_local(headers, "x-forwarded-for")
        && plain_forwarded_header_is_local(headers, "x-real-ip")
        && forwarded_header_is_local(headers)
}

fn plain_forwarded_header_is_local(headers: &HeaderMap, name: &str) -> bool {
    headers.get_all(name).iter().all(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .all(|part| parse_forwarded_address(part).is_some_and(address_is_loopback))
        })
    })
}

fn forwarded_header_is_local(headers: &HeaderMap) -> bool {
    headers.get_all("forwarded").iter().all(|value| {
        value.to_str().is_ok_and(|value| {
            value.split(',').all(|element| {
                element.split(';').all(|parameter| {
                    let Some((name, value)) = parameter.split_once('=') else {
                        return true;
                    };
                    !name.trim().eq_ignore_ascii_case("for")
                        || parse_forwarded_address(value).is_some_and(address_is_loopback)
                })
            })
        })
    })
}

fn parse_forwarded_address(value: &str) -> Option<IpAddr> {
    let value = value.trim();
    let value = if value.starts_with('"') || value.ends_with('"') {
        value.strip_prefix('"')?.strip_suffix('"')?
    } else {
        value
    };
    if let Some(value) = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    {
        return value.parse().ok();
    }
    value
        .parse::<IpAddr>()
        .ok()
        .or_else(|| value.parse::<SocketAddr>().ok().map(|address| address.ip()))
}

fn address_is_loopback(address: IpAddr) -> bool {
    address.is_loopback()
        || match address {
            IpAddr::V6(address) => address
                .to_ipv4_mapped()
                .is_some_and(|address| address.is_loopback()),
            IpAddr::V4(_) => false,
        }
}

async fn lobby(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Html<String> {
    let rooms = match state
        .client
        .get(format!("{}/api/rooms", state.game_internal))
        .send()
        .await
    {
        Ok(response) => response
            .json::<Vec<RoomSummary>>()
            .await
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let room_rows = room_rows_markup(&rooms, request_is_local(peer, &headers));
    Html(format!(
        "{head}<body data-page=\"lobby\" data-game-api=\"{api}\">
        <main class=\"lobby\"><h1>♠ Mille · Tysiąc · Tausend ♥</h1>
        <label>Language <select id=\"locale\"><option value=\"en\">EN</option><option value=\"de\">DE</option><option value=\"pl\">PL</option></select></label>
        <section class=\"panel\"><h2 data-i18n=\"rooms\">Rooms</h2><ul>{room_rows}</ul></section>
        <section class=\"panel\"><h2 data-i18n=\"create\">Create a room</h2>
        <form id=\"create-room\">
        <label>Room name <input name=\"name\" maxlength=\"48\" required></label>
        <label>Player password <input name=\"player_password\" type=\"password\" maxlength=\"128\" required></label>
        <label>Referee password (optional) <input name=\"referee_password\" type=\"password\" maxlength=\"128\" placeholder=\"Generated if empty\"></label>
        <label>Seed (optional decimal or 0x…) <input name=\"seed\" inputmode=\"numeric\"></label>
        <label>Target <input name=\"target_score\" type=\"number\" value=\"1000\" min=\"100\" step=\"10\"></label>
        <label>Lock <input name=\"lock_score\" type=\"number\" value=\"900\" min=\"0\" step=\"10\"></label>
        <label>Talon <select name=\"talon_visibility\"><option value=\"always_public\">Always public</option><option value=\"hide_at_one_hundred\">Hide at bid 100</option></select></label>
        <button type=\"submit\">Create room</button>
        </form><p id=\"status\"></p></section></main>{scripts}</body></html>",
        head = modern_head("Mille", "./"),
        api = escape_attr(&state.game_public),
        scripts = scripts(),
    ))
}

fn room_rows_markup(rooms: &[RoomSummary], allow_removal: bool) -> String {
    let mut room_rows = String::new();
    for room in rooms {
        let remove = if allow_removal {
            format!(
                " <button type=\"button\" data-delete-room data-room=\"{room}\" aria-label=\"Remove room {room}\" title=\"Remove room\">×</button>",
                room = escape_attr(&room.name),
            )
        } else {
            String::new()
        };
        write!(
            room_rows,
            "<li><a href=\"room/{url}\">{name}</a> — {count}/3 — {lifecycle:?}{remove}</li>",
            url = urlencoding::encode(&room.name),
            name = escape(&room.name),
            count = room.seats.len(),
            lifecycle = room.lifecycle,
            remove = remove,
        )
        .expect("writing HTML to a String cannot fail");
    }
    room_rows
}

async fn observer(State(state): State<Arc<AppState>>, Path(room): Path<String>) -> Response {
    let fetched = state
        .client
        .get(format!(
            "{}/api/rooms/{}/view?role=observer",
            state.game_internal,
            urlencoding::encode(&room)
        ))
        .send()
        .await;
    let Ok(response) = fetched else {
        return error_page(StatusCode::BAD_GATEWAY, "Game server unavailable");
    };
    if !response.status().is_success() {
        return error_page(StatusCode::NOT_FOUND, "Room not found");
    }
    let Ok(View::Observer(view)) = response.json::<View>().await else {
        return error_page(StatusCode::BAD_GATEWAY, "Invalid game response");
    };
    let body = observer_markup(&view);
    Html(format!(
        "<!DOCTYPE html PUBLIC \"-//W3C//DTD HTML 4.01 Transitional//EN\">
        <html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\">
        <noscript><meta http-equiv=\"refresh\" content=\"2\"></noscript><title>{title}</title><base href=\"../\"><link rel=\"icon\" href=\"static/favicon.svg\" type=\"image/svg+xml\">
        <link rel=\"stylesheet\" href=\"static/observer.css\" type=\"text/css\"></head>
        <body data-page=\"observer\" data-room=\"{room}\" data-game-api=\"{api}\">
        <div id=\"table\">{body}</div>
        <div class=\"join\"><form id=\"join-room\"><strong>Join this table:</strong>
        <label>Name <input name=\"name\" maxlength=\"48\"></label>
        <label>Password or token <input name=\"password_or_token\" type=\"password\" maxlength=\"128\"></label>
        <input type=\"submit\" value=\"Join\"><span id=\"join-status\"></span></form></div>
        <script type=\"text/javascript\" src=\"static/observer.js\"></script></body></html>",
        title = escape(&format!("{} — Mille", view.room_name)),
        room = escape_attr(&room),
        api = escape_attr(&state.game_public),
    ))
    .into_response()
}

async fn referee(State(state): State<Arc<AppState>>, Path(room): Path<String>) -> Html<String> {
    Html(format!(
        "{head}<body data-page=\"referee\" data-room=\"{room}\" data-game-api=\"{api}\">
        <main class=\"game-shell\">
        <header><a href=\"room/{room_url}\">← observer</a><h1>Referee · {room_name}</h1>
        <select id=\"locale\"><option value=\"en\">EN</option><option value=\"de\">DE</option><option value=\"pl\">PL</option></select></header>
        <section id=\"auth\" class=\"panel\"><label>Referee password <input id=\"credential\" type=\"password\"></label><button id=\"connect\">Open table</button></section>
        <section id=\"admin\" class=\"toolbar hidden\"><button data-admin=\"pause\">Pause</button><button data-admin=\"resume\">Resume</button><button data-admin=\"abort\">Abort</button><button data-admin=\"rematch\">Rematch</button><button data-admin=\"advance_presentation\">Skip animation</button></section>
        <section id=\"view\" class=\"felt\"></section><details><summary>Omniscient state and history</summary><pre id=\"raw\"></pre></details>
        <p id=\"status\"></p></main>{scripts}</body></html>",
        head = modern_head(&format!("Referee — {room}"), "../../"),
        room = escape_attr(&room),
        room_url = urlencoding::encode(&room),
        room_name = escape(&room),
        api = escape_attr(&state.game_public),
        scripts = scripts(),
    ))
}

async fn player(
    State(state): State<Arc<AppState>>,
    Path((room, seat, name)): Path<(String, u8, String)>,
) -> Response {
    if !(1..=3).contains(&seat) {
        return error_page(StatusCode::NOT_FOUND, "Seat not found");
    }
    Html(format!(
        "{head}<body data-page=\"player\" data-room=\"{room}\" data-seat=\"{seat_zero}\" data-name=\"{name}\" data-game-api=\"{api}\">
        <main class=\"game-shell\"><header><a href=\"room/{room_url}\">← observer</a>
        <h1>{name_text} · seat {seat}</h1><select id=\"locale\"><option value=\"en\">EN</option><option value=\"de\">DE</option><option value=\"pl\">PL</option></select></header>
        <section id=\"auth\" class=\"panel\"><label>Password or seat token <input id=\"credential\" type=\"password\"></label><button id=\"connect\">Sit down</button></section>
        <section id=\"view\" class=\"felt\"></section><section id=\"hand\" class=\"hand\"></section><section id=\"actions\" class=\"toolbar\"></section>
        <p id=\"status\"></p></main>{scripts}</body></html>",
        head = modern_head(&format!("{name} — {room}"), "../../../../"),
        room = escape_attr(&room),
        room_url = urlencoding::encode(&room),
        seat_zero = seat - 1,
        name = escape_attr(&name),
        name_text = escape(&name),
        api = escape_attr(&state.game_public),
        scripts = scripts(),
    ))
    .into_response()
}

fn observer_markup(view: &PublicView) -> String {
    let mut scores = String::new();
    for player in &view.players {
        write!(
            scores,
            "<div class=\"player seat{seat}\"><strong>{name}</strong><br>{score} points<br>{cards} cards</div>",
            seat = player.seat.0 + 1,
            name = escape(&player.name),
            score = player.score,
            cards = player.card_count,
        )
        .expect("writing HTML to a String cannot fail");
    }
    let (phase, trump, current, last, talon) = if let Some(game) = &view.game {
        (
            escape(&game.phase),
            game.trump
                .map_or_else(|| "—".to_owned(), |suit| suit_symbol(suit).to_owned()),
            game.current_trick
                .iter()
                .map(|played| card_markup(PublicCard::Face(played.card)))
                .collect::<String>(),
            game.last_trick
                .iter()
                .map(|played| card_markup(PublicCard::Face(played.card)))
                .collect::<String>(),
            game.talon
                .iter()
                .copied()
                .map(card_markup)
                .collect::<String>(),
        )
    } else {
        (
            "lobby".to_owned(),
            "—".to_owned(),
            String::new(),
            String::new(),
            String::new(),
        )
    };
    let mut history = String::new();
    for event in view.history.iter().rev().take(20) {
        write!(history, "<li>{}</li>", escape(&event.message))
            .expect("writing HTML to a String cannot fail");
    }
    let presentation_cards = if view.presentation.stage == "ready" {
        String::new()
    } else {
        (0..view.presentation.visible_deal_cards)
            .map(|_| card_markup(PublicCard::Back))
            .collect::<String>()
    };
    format!(
        "<h1>{room}</h1><p class=\"status\">{lifecycle:?} · {phase} · trump {trump}</p>
        <p class=\"status\">Presentation: {presentation_stage} {visible}/24</p>
        <div class=\"cards\">{presentation_cards}</div>
        <div class=\"scoreboard\">{scores}</div>
        <div class=\"center\"><h2>Current trick</h2><div class=\"cards\">{current}</div>
        <h2>Last trick</h2><div class=\"cards\">{last}</div>
        <h2>Talon</h2><div class=\"cards\">{talon}</div></div>
        <div class=\"history\"><h2>Public history</h2><ol>{history}</ol></div>",
        room = escape(&view.room_name),
        lifecycle = view.lifecycle,
        presentation_stage = escape(&view.presentation.stage),
        visible = view.presentation.visible_deal_cards,
    )
}

fn card_markup(card: PublicCard) -> String {
    match card {
        PublicCard::Back => "<span class=\"card back\"><span>♠</span></span>".into(),
        PublicCard::Face(card) => {
            let red = matches!(
                card.suit,
                mille_cards::Suit::Diamonds | mille_cards::Suit::Hearts
            );
            format!(
                "<span class=\"card {colour}\"><b>{rank}</b><i>{suit}</i></span>",
                colour = if red { "red" } else { "black" },
                rank = card.rank.label(),
                suit = card.suit.symbol(),
            )
        }
    }
}

fn suit_symbol(suit: mille_cards::Suit) -> &'static str {
    match suit {
        mille_cards::Suit::Clubs => "♣",
        mille_cards::Suit::Diamonds => "♦",
        mille_cards::Suit::Hearts => "♥",
        mille_cards::Suit::Spades => "♠",
    }
}

fn modern_head(title: &str, base_href: &str) -> String {
    let lobby_base_fix = if base_href == "./" {
        "<script>(function(){var b=document.getElementById('app-base'),p=window.location.pathname;if(b&&p.charAt(p.length-1)!=='/'){b.href=p+'/';}}());</script>"
    } else {
        ""
    };
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1,viewport-fit=cover\"><title>{}</title><base id=\"app-base\" href=\"{}\">{lobby_base_fix}<link rel=\"icon\" href=\"static/favicon.svg\" type=\"image/svg+xml\"><link rel=\"stylesheet\" href=\"static/app.css\"></head>",
        escape(title),
        escape_attr(base_href),
    )
}

fn scripts() -> &'static str {
    "<script src=\"static/app.js\"></script>"
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn escape_attr(value: &str) -> String {
    escape(value)
}

fn error_page(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        format!(
            "<html><body><h1>{}</h1><p><a href=\"/\">Lobby</a></p></body></html>",
            escape(message)
        ),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mille_protocol::{Lifecycle, PresentationView, PublicGame, PublicPlayer};
    use tysiac::{Config, Seat};

    #[test]
    fn classic_observer_renders_backs_scores_and_escapes_names() {
        let view = PublicView {
            room_name: "<table>".into(),
            revision: 1,
            lifecycle: Lifecycle::Running,
            config: Config::default(),
            players: vec![PublicPlayer {
                seat: Seat::ONE,
                name: "Ada & Bert".into(),
                score: 44,
                connected: true,
                card_count: 7,
            }],
            game: Some(PublicGame {
                phase: "auction".into(),
                dealer: Seat::ONE,
                turn: Some(Seat::TWO),
                contractor: Some(Seat::TWO),
                bid_or_contract: Some(100),
                trump: None,
                current_trick: Vec::new(),
                last_trick: Vec::new(),
                open_hands: Vec::new(),
                talon: vec![PublicCard::Back; 3],
            }),
            presentation: PresentationView {
                stage: "dealing".into(),
                visible_deal_cards: 4,
                input_blocked: true,
            },
            history: Vec::new(),
        };
        let html = observer_markup(&view);
        assert!(html.contains("&lt;table&gt;"));
        assert!(html.contains("Ada &amp; Bert"));
        assert_eq!(html.matches("card back").count(), 7);
    }

    #[test]
    fn html_escaping_covers_all_markup_characters() {
        assert_eq!(escape("<&>\"'"), "&lt;&amp;&gt;&quot;&#39;");
    }

    #[test]
    fn pages_use_relative_assets_below_a_proxy_prefix() {
        let lobby = modern_head("Mille", "./");
        assert!(lobby.contains("<base id=\"app-base\" href=\"./\">"));
        assert!(lobby.contains("b.href=p+'/'"));
        assert!(lobby.contains("href=\"static/app.css\""));
        assert!(!lobby.contains("href=\"/static/"));

        let player = modern_head("Player", "../../../../");
        assert!(player.contains("<base id=\"app-base\" href=\"../../../../\">"));
        assert!(!player.contains("b.href=p+'/'"));
        assert_eq!(scripts(), "<script src=\"static/app.js\"></script>");
    }

    #[test]
    fn websocket_proxy_preserves_transport_security() {
        assert_eq!(
            websocket_base("https://game.example/engine"),
            Some("wss://game.example/engine".to_owned())
        );
        assert_eq!(
            websocket_base("http://127.0.0.1:4100"),
            Some("ws://127.0.0.1:4100".to_owned())
        );
        assert_eq!(websocket_base("ftp://game.example"), None);
    }

    #[test]
    fn local_request_classification_checks_peer_and_forwarded_clients() {
        let local: SocketAddr = "127.0.0.1:1234".parse().unwrap();
        let local_v6: SocketAddr = "[::1]:1234".parse().unwrap();
        let remote: SocketAddr = "192.0.2.10:1234".parse().unwrap();
        let mut headers = HeaderMap::new();

        assert!(request_is_local(local, &headers));
        assert!(request_is_local(local_v6, &headers));
        assert!(!request_is_local(remote, &headers));

        headers.insert("x-forwarded-for", "127.0.0.1, ::1".parse().unwrap());
        headers.insert("x-real-ip", "127.0.0.1:4567".parse().unwrap());
        headers.insert(
            "forwarded",
            "for=\"[::1]\";proto=https, for=127.0.0.1".parse().unwrap(),
        );
        assert!(request_is_local(local, &headers));
        assert!(!request_is_local(remote, &headers));

        headers.insert("x-forwarded-for", "203.0.113.8".parse().unwrap());
        assert!(!request_is_local(local, &headers));

        headers.insert("x-forwarded-for", "127.0.0.1".parse().unwrap());
        headers.insert("forwarded", "for=198.51.100.9".parse().unwrap());
        assert!(!request_is_local(local, &headers));

        headers.insert("forwarded", "for=unknown".parse().unwrap());
        assert!(!request_is_local(local, &headers));
    }

    #[test]
    fn room_delete_detection_does_not_affect_other_proxy_requests() {
        assert!(is_room_delete(&Method::DELETE, "/api/rooms/table"));
        assert!(is_room_delete(&Method::DELETE, "/api/rooms/table%20one"));
        assert!(!is_room_delete(&Method::POST, "/api/rooms/table"));
        assert!(!is_room_delete(&Method::DELETE, "/api/rooms"));
        assert!(!is_room_delete(&Method::DELETE, "/api/rooms/table/join"));
        assert!(!is_room_delete(&Method::GET, "/api/rooms/table/view"));
    }

    #[test]
    fn lobby_room_controls_are_rendered_only_for_local_requests() {
        let rooms = vec![RoomSummary {
            name: "Ada's <table>".into(),
            seats: vec!["Ada".into()],
            lifecycle: Lifecycle::Lobby,
        }];
        let headers = HeaderMap::new();
        let local: SocketAddr = "127.0.0.1:1234".parse().unwrap();
        let remote: SocketAddr = "192.0.2.10:1234".parse().unwrap();

        let local_rows = room_rows_markup(&rooms, request_is_local(local, &headers));
        assert!(local_rows.contains("data-delete-room"));
        assert!(local_rows.contains("data-room=\"Ada&#39;s &lt;table&gt;\""));

        let remote_rows = room_rows_markup(&rooms, request_is_local(remote, &headers));
        assert!(!remote_rows.contains("data-delete-room"));
        assert!(remote_rows.contains("Ada&#39;s &lt;table&gt;"));
    }
}
