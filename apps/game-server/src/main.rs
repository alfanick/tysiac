use std::{
    collections::{BTreeMap, VecDeque},
    env,
    net::SocketAddr,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use axum::{
    Json, Router,
    extract::{
        ConnectInfo, Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use futures_util::{SinkExt, StreamExt};
use mille_protocol::{
    AdminAction, ApiError, ClientMessage, Lifecycle, PresentationView, PublicPlayer, PublicView,
    RoundAudit, ServerMessage, View, project_player, project_public,
};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool, sqlite::SqlitePoolOptions};
use tokio::sync::{Mutex, RwLock, broadcast};
use tower_http::cors::{Any, CorsLayer};
use tracing::error;
use tysiac::{Action, Config, Match, MatchPhase, Seat};
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    db: SqlitePool,
    rooms: Arc<RwLock<BTreeMap<String, Arc<Mutex<Room>>>>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Room {
    name: String,
    player_password: String,
    referee_password: String,
    base_seed: u64,
    match_index: u64,
    config: Config,
    seats: Vec<SeatIdentity>,
    lifecycle: Lifecycle,
    revision: u64,
    game: Option<Match>,
    #[serde(default)]
    round_audits: Vec<RoundAudit>,
    presentation: Presentation,
    recent_commands: VecDeque<String>,
    #[serde(skip, default)]
    connections: [usize; 3],
    #[serde(skip, default)]
    connection_generation: [u64; 3],
    #[serde(skip, default)]
    deleted: bool,
    #[serde(skip, default = "broadcast_channel")]
    updates: broadcast::Sender<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SeatIdentity {
    name: String,
    token: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Presentation {
    stage: PresentationStage,
    visible_deal_cards: usize,
    gate_until_ms: u64,
    deal_started_ms: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PresentationStage {
    #[default]
    Ready,
    Shuffling,
    Dealing,
}

const INITIAL_SHUFFLE_PRESENTATION_MS: u64 = 2_000;
const DEAL_PRESENTATION_MS: u64 = 6_000;
const FOUR_NINES_RESHUFFLE_PRESENTATION_MS: u64 = 2_000;
const MAX_PRESENTED_FOUR_NINES_RESHUFFLES: u64 = 10;

fn broadcast_channel() -> broadcast::Sender<u64> {
    broadcast::channel(64).0
}

#[derive(Deserialize)]
struct CreateRoom {
    name: String,
    player_password: String,
    #[serde(default)]
    referee_password: String,
    seed: Option<String>,
    config: Option<Config>,
}

#[derive(Serialize)]
struct CreatedRoom {
    name: String,
    player_password: String,
    referee_password: String,
    base_seed: u64,
    observer_url: String,
    referee_url: String,
}

#[derive(Deserialize)]
struct JoinRoom {
    name: String,
    password_or_token: String,
}

#[derive(Serialize)]
struct JoinedRoom {
    seat: Seat,
    name: String,
    token: String,
    player_url: String,
}

#[derive(Deserialize)]
struct LeaveRoom {
    token: String,
}

#[derive(Deserialize)]
struct ActionRequest {
    seat: Seat,
    token: String,
    command_id: String,
    expected_revision: u64,
    action: Action,
}

#[derive(Deserialize)]
struct AdminRequest {
    referee_password: String,
    command_id: String,
    expected_revision: u64,
    action: AdminAction,
}

#[derive(Deserialize)]
struct ViewQuery {
    role: Option<String>,
    seat: Option<u8>,
    credential: Option<String>,
}

#[derive(Deserialize)]
struct WsQuery {
    role: Option<String>,
    seat: Option<u8>,
    credential: Option<String>,
}

#[derive(Serialize)]
struct RoomSummary {
    name: String,
    seats: Vec<String>,
    lifecycle: Lifecycle,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let database_url =
        env::var("MILLE_DATABASE_URL").unwrap_or_else(|_| "sqlite://mille.sqlite?mode=rwc".into());
    let listen = env::var("MILLE_GAME_LISTEN").unwrap_or_else(|_| "0.0.0.0:4100".into());
    let db = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .with_context(|| format!("opening {database_url}"))?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS rooms (
            name TEXT PRIMARY KEY NOT NULL,
            state_json TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
    )
    .execute(&db)
    .await?;
    let rooms = restore_rooms(&db).await?;
    let state = AppState {
        db,
        rooms: Arc::new(RwLock::new(rooms)),
    };

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/rooms", get(list_rooms).post(create_room))
        .route("/api/rooms/{room}", delete(delete_room))
        .route("/api/rooms/{room}/join", post(join_room))
        .route("/api/rooms/{room}/leave", post(leave_room))
        .route("/api/rooms/{room}/view", get(room_view))
        .route("/api/rooms/{room}/action", post(room_action))
        .route("/api/rooms/{room}/admin", post(room_admin))
        .route("/ws/{room}", get(room_ws))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&listen).await?;
    eprintln!("game-server listening on http://{listen}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

async fn restore_rooms(db: &SqlitePool) -> anyhow::Result<BTreeMap<String, Arc<Mutex<Room>>>> {
    let mut rooms = BTreeMap::new();
    for row in sqlx::query("SELECT state_json FROM rooms ORDER BY name")
        .fetch_all(db)
        .await?
    {
        let json: String = row.try_get("state_json")?;
        let mut room: Room = serde_json::from_str(&json)?;
        room.updates = broadcast_channel();
        stdout_secrets(&room, "RESTORED");
        stdout_full(&room, "RESTORED_STATE");
        rooms.insert(room.name.clone(), Arc::new(Mutex::new(room)));
    }
    Ok(rooms)
}

async fn list_rooms(State(state): State<AppState>) -> Json<Vec<RoomSummary>> {
    let rooms = state.rooms.read().await;
    let values = rooms.values().cloned().collect::<Vec<_>>();
    drop(rooms);
    let mut summaries = Vec::new();
    for room in values {
        let room = room.lock().await;
        if room.deleted {
            continue;
        }
        summaries.push(RoomSummary {
            name: room.name.clone(),
            seats: room.seats.iter().map(|seat| seat.name.clone()).collect(),
            lifecycle: room.lifecycle,
        });
    }
    Json(summaries)
}

async fn create_room(
    State(state): State<AppState>,
    Json(input): Json<CreateRoom>,
) -> ApiResult<Json<CreatedRoom>> {
    validate_identifier(&input.name, "room name")?;
    validate_secret(&input.player_password, "player password")?;
    let referee_password = if input.referee_password.is_empty() {
        Uuid::new_v4().simple().to_string()
    } else {
        validate_secret(&input.referee_password, "referee password")?;
        input.referee_password
    };
    let config = input.config.unwrap_or_default();
    if config.target_score < 100
        || config.target_score % 10 != 0
        || config.lock_score < 0
        || config.lock_score >= config.target_score
        || config.lock_score % 10 != 0
    {
        return Err(ApiFailure::bad_request(
            "invalid_config",
            "target and lock must be multiples of ten, with 0 <= lock < target",
        ));
    }
    let seed = input
        .seed
        .as_deref()
        .map(parse_seed)
        .transpose()?
        .unwrap_or_else(time_seed);
    let room = Room {
        name: input.name.clone(),
        player_password: input.player_password,
        referee_password,
        base_seed: seed,
        match_index: 0,
        config,
        seats: Vec::new(),
        lifecycle: Lifecycle::Lobby,
        revision: 0,
        game: None,
        round_audits: Vec::new(),
        presentation: Presentation::default(),
        recent_commands: VecDeque::new(),
        connections: [0; 3],
        connection_generation: [0; 3],
        deleted: false,
        updates: broadcast_channel(),
    };
    let mut rooms = state.rooms.write().await;
    if rooms.contains_key(&input.name) {
        return Err(ApiFailure::conflict(
            "room_exists",
            "room name is already in use",
            None,
        ));
    }
    persist(&state.db, &room).await?;
    stdout_secrets(&room, "CREATED");
    let response = CreatedRoom {
        name: room.name.clone(),
        player_password: room.player_password.clone(),
        referee_password: room.referee_password.clone(),
        base_seed: room.base_seed,
        observer_url: format!("/room/{}", room.name),
        referee_url: format!("/room/{}/referee", room.name),
    };
    rooms.insert(room.name.clone(), Arc::new(Mutex::new(room)));
    Ok(Json(response))
}

async fn delete_room(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    Path(room_name): Path<String>,
) -> ApiResult<StatusCode> {
    if !peer.ip().is_loopback() {
        return Err(ApiFailure::forbidden(
            "rooms can only be deleted from the local machine",
        ));
    }

    let mut rooms = state.rooms.write().await;
    let handle = rooms.get(&room_name).cloned().ok_or_else(room_not_found)?;
    let mut room = handle.lock().await;
    sqlx::query("DELETE FROM rooms WHERE name = ?1")
        .bind(&room_name)
        .execute(&state.db)
        .await
        .map_err(|error| ApiFailure::internal(format!("deleting room: {error}")))?;

    room.deleted = true;
    let player_connections = room.connections.iter().sum::<usize>();
    let listeners = room.updates.receiver_count();
    room.updates.send(room.revision).ok();
    let deleted_name = room.name.clone();
    drop(room);
    rooms.remove(&room_name);
    drop(rooms);

    stdout_line(
        &deleted_name,
        "DELETED",
        format!(
            "deleted by local peer {peer}; notified {listeners} listener(s), including \
             {player_connections} active player connection(s)"
        ),
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn join_room(
    State(state): State<AppState>,
    Path(room_name): Path<String>,
    Json(input): Json<JoinRoom>,
) -> ApiResult<Json<JoinedRoom>> {
    validate_identifier(&input.name, "player name")?;
    let handle = find_room(&state, &room_name).await?;
    let mut room = handle.lock().await;
    ensure_room_active(&room)?;
    if let Some((index, seat)) = room
        .seats
        .iter()
        .enumerate()
        .find(|(_, seat)| seat.token == input.password_or_token)
    {
        return Ok(Json(join_response(&room.name, index, seat)));
    }
    if room.lifecycle != Lifecycle::Lobby {
        return Err(ApiFailure::conflict(
            "match_started",
            "new players cannot join after start",
            Some(room.revision),
        ));
    }
    if input.password_or_token != room.player_password {
        return Err(ApiFailure::unauthorized(
            "wrong player password or reconnect token",
        ));
    }
    if room.seats.len() == 3 {
        return Err(ApiFailure::conflict(
            "room_full",
            "all three seats are occupied",
            Some(room.revision),
        ));
    }
    if room.seats.iter().any(|seat| seat.name == input.name) {
        return Err(ApiFailure::conflict(
            "name_in_use",
            "player names must be unique",
            Some(room.revision),
        ));
    }
    let seat = SeatIdentity {
        name: input.name,
        token: Uuid::new_v4().simple().to_string(),
    };
    room.seats.push(seat);
    let started = room.seats.len() == 3;
    if started && let Err(error) = start_match(&mut room) {
        room.seats.pop();
        return Err(error);
    }
    room.revision += 1;
    let index = room.seats.len() - 1;
    persist(&state.db, &room).await?;
    stdout_line(
        &room.name,
        "JOIN",
        format!(
            "seat={} name={} token={}",
            index + 1,
            escaped(&room.seats[index].name),
            room.seats[index].token
        ),
    );
    room.updates.send(room.revision).ok();
    let response = join_response(&room.name, index, &room.seats[index]);
    if started {
        schedule_presentation(state, room.name.clone(), room.revision);
    }
    Ok(Json(response))
}

async fn leave_room(
    State(state): State<AppState>,
    Path(room_name): Path<String>,
    Json(input): Json<LeaveRoom>,
) -> ApiResult<StatusCode> {
    let handle = find_room(&state, &room_name).await?;
    let mut room = handle.lock().await;
    ensure_room_active(&room)?;
    if room.lifecycle != Lifecycle::Lobby {
        return Err(ApiFailure::conflict(
            "match_started",
            "seats are fixed after start",
            Some(room.revision),
        ));
    }
    let index = room
        .seats
        .iter()
        .position(|seat| seat.token == input.token)
        .ok_or_else(|| ApiFailure::unauthorized("invalid reconnect token"))?;
    let departed = room.seats.remove(index);
    room.revision += 1;
    persist(&state.db, &room).await?;
    stdout_line(
        &room.name,
        "LEAVE",
        format!("old_seat={} name={}", index + 1, escaped(&departed.name)),
    );
    room.updates.send(room.revision).ok();
    Ok(StatusCode::NO_CONTENT)
}

async fn room_view(
    State(state): State<AppState>,
    Path(room_name): Path<String>,
    Query(query): Query<ViewQuery>,
) -> ApiResult<Json<View>> {
    let handle = find_room(&state, &room_name).await?;
    let room = handle.lock().await;
    ensure_room_active(&room)?;
    Ok(Json(make_view(
        &room,
        query.role.as_deref(),
        query.seat,
        query.credential.as_deref(),
    )?))
}

async fn room_action(
    State(state): State<AppState>,
    Path(room_name): Path<String>,
    Json(input): Json<ActionRequest>,
) -> ApiResult<Json<ServerMessage>> {
    let handle = find_room(&state, &room_name).await?;
    let mut room = handle.lock().await;
    ensure_room_active(&room)?;
    authenticate_seat(&room, input.seat, &input.token)?;
    if room.connections[input.seat.index()] > 0 {
        return Err(ApiFailure::conflict(
            "use_controlling_socket",
            "this seat has a live controlling WebSocket; submit the action there",
            Some(room.revision),
        ));
    }
    apply_player_action(
        &state,
        &mut room,
        input.seat,
        input.command_id,
        input.expected_revision,
        input.action,
    )
    .await
}

async fn room_admin(
    State(state): State<AppState>,
    Path(room_name): Path<String>,
    Json(input): Json<AdminRequest>,
) -> ApiResult<Json<ServerMessage>> {
    let handle = find_room(&state, &room_name).await?;
    let mut room = handle.lock().await;
    ensure_room_active(&room)?;
    if room.referee_password != input.referee_password {
        return Err(ApiFailure::unauthorized("wrong referee password"));
    }
    check_command(&room, &input.command_id, input.expected_revision)?;
    if room.recent_commands.contains(&input.command_id) {
        return Ok(Json(ServerMessage::Snapshot(make_referee_view(&room))));
    }
    match input.action {
        AdminAction::Start if room.lifecycle == Lifecycle::Lobby => start_match(&mut room)?,
        AdminAction::Pause if room.lifecycle == Lifecycle::Running => {
            room.lifecycle = Lifecycle::Paused;
        }
        AdminAction::Resume if room.lifecycle == Lifecycle::Paused => {
            room.lifecycle = Lifecycle::Running;
        }
        AdminAction::Abort if matches!(room.lifecycle, Lifecycle::Running | Lifecycle::Paused) => {
            room.lifecycle = Lifecycle::Aborted;
        }
        AdminAction::Rematch
            if matches!(room.lifecycle, Lifecycle::Finished | Lifecycle::Aborted) =>
        {
            room.match_index += 1;
            start_match(&mut room)?;
        }
        AdminAction::AdvancePresentation => {
            room.presentation = Presentation::default();
        }
        _ => {
            return Err(ApiFailure::conflict(
                "illegal_admin_action",
                "admin action is not legal now",
                Some(room.revision),
            ));
        }
    }
    remember_command(&mut room, input.command_id);
    room.revision += 1;
    persist(&state.db, &room).await?;
    stdout_line(
        &room.name,
        "ADMIN",
        format!("{:?} revision={}", input.action, room.revision),
    );
    stdout_full(&room, "STATE");
    room.updates.send(room.revision).ok();
    if matches!(
        input.action,
        AdminAction::Start | AdminAction::Rematch | AdminAction::Resume
    ) {
        schedule_presentation(state.clone(), room.name.clone(), room.revision);
    }
    if input.action == AdminAction::Resume && room.game.as_ref().is_some_and(round_finished) {
        schedule_next_round(state.clone(), room.name.clone(), room.revision);
    }
    Ok(Json(ServerMessage::Updated {
        revision: room.revision,
        events: Vec::new(),
        view: make_referee_view(&room),
    }))
}

fn start_match(room: &mut Room) -> ApiResult<()> {
    if room.seats.len() != 3 {
        return Err(ApiFailure::conflict(
            "seats_missing",
            "three players are required",
            Some(room.revision),
        ));
    }
    let seed = Match::derive_round_seed(room.base_seed, room.match_index, 0);
    let dealer = Seat((seed % 3) as u8);
    let names: [String; 3] = room
        .seats
        .iter()
        .map(|seat| seat.name.clone())
        .collect::<Vec<_>>()
        .try_into()
        .expect("three seats");
    let mut game = Match::new(names, room.config.clone(), dealer);
    game.match_index = room.match_index;
    let deal = game
        .deal_seeded_with_report(seed)
        .map_err(|error| rule_failure(&error))?;
    let audit = RoundAudit {
        match_index: room.match_index,
        round_index: 0,
        derived_seed: seed,
        deal_order: deal.order,
        four_nines_reshuffles: deal.four_nines_reshuffles,
    };
    let shuffle_detail = shuffle_audit_detail(&audit);
    room.round_audits.clear();
    room.presentation = deal_presentation(now_ms(), audit.four_nines_reshuffles);
    room.round_audits.push(audit);
    room.game = Some(game);
    room.lifecycle = Lifecycle::Running;
    stdout_line(&room.name, "SHUFFLE", shuffle_detail);
    Ok(())
}

fn deal_presentation(now: u64, four_nines_reshuffles: u64) -> Presentation {
    let reshuffle_delay = four_nines_reshuffles
        .min(MAX_PRESENTED_FOUR_NINES_RESHUFFLES)
        .saturating_mul(FOUR_NINES_RESHUFFLE_PRESENTATION_MS);
    let deal_started_ms = now
        .saturating_add(INITIAL_SHUFFLE_PRESENTATION_MS)
        .saturating_add(reshuffle_delay);
    Presentation {
        stage: PresentationStage::Shuffling,
        visible_deal_cards: 0,
        gate_until_ms: deal_started_ms.saturating_add(DEAL_PRESENTATION_MS),
        deal_started_ms,
    }
}

fn shuffle_audit_detail(audit: &RoundAudit) -> String {
    let reshuffle_reason = if audit.four_nines_reshuffles == 0 {
        "none"
    } else {
        "initial_hand_contained_all_four_nines"
    };
    format!(
        "match={} round={} derived_seed={} four_nines_reshuffles={} \
         reshuffle_reason={reshuffle_reason} accepted_deal_order={}",
        audit.match_index,
        audit.round_index,
        audit.derived_seed,
        audit.four_nines_reshuffles,
        audit
            .deal_order
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    )
}

fn published_round_audit_message(audit: &RoundAudit) -> String {
    let reshuffle_summary = match audit.four_nines_reshuffles {
        0 => "no all-four-nines reshuffle was required".to_owned(),
        1 => "1 deal was reshuffled because an initial hand contained all four nines".to_owned(),
        count => format!(
            "{count} deals were reshuffled because an initial hand contained all four nines"
        ),
    };
    format!(
        "published match {} round {}: seed {}, {reshuffle_summary}; accepted deal {}",
        audit.match_index,
        audit.round_index,
        audit.derived_seed,
        audit
            .deal_order
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    )
}

fn round_finished(game: &Match) -> bool {
    matches!(
        &game.phase,
        MatchPhase::Round(round)
            if matches!(round.phase, tysiac::RoundPhase::RoundFinished(_))
    )
}

async fn apply_player_action(
    state: &AppState,
    room: &mut Room,
    actor: Seat,
    command_id: String,
    expected_revision: u64,
    action: Action,
) -> ApiResult<Json<ServerMessage>> {
    check_command(room, &command_id, expected_revision)?;
    if room.recent_commands.contains(&command_id) {
        let view = player_view(room, actor);
        return Ok(Json(ServerMessage::Snapshot(view)));
    }
    if room.lifecycle != Lifecycle::Running {
        return Err(ApiFailure::conflict(
            "not_running",
            "match is not running",
            Some(room.revision),
        ));
    }
    update_presentation(room);
    if now_ms() < room.presentation.gate_until_ms {
        return Err(ApiFailure::conflict(
            "presentation_wait",
            "the table animation has not completed",
            Some(room.revision),
        ));
    }
    let outcome = room
        .game
        .as_mut()
        .ok_or_else(|| ApiFailure::conflict("no_match", "no match exists", Some(room.revision)))?
        .apply(actor, action.clone())
        .map_err(|error| rule_failure(&error))?;
    if matches!(
        room.game.as_ref().map(|game| &game.phase),
        Some(MatchPhase::MatchFinished { .. })
    ) {
        room.lifecycle = Lifecycle::Finished;
        publish_round_audits(room);
    }
    room.presentation.gate_until_ms = now_ms() + presentation_delay(&action, &outcome.events);
    remember_command(room, command_id);
    room.revision += 1;
    persist(&state.db, room).await?;
    stdout_line(
        &room.name,
        "ACTION",
        format!(
            "seat={} action={} revision={}",
            actor.0 + 1,
            escaped(&serde_json::to_string(&action).unwrap_or_default()),
            room.revision
        ),
    );
    for event in &outcome.events {
        stdout_line(&room.name, "EVENT", event.message.clone());
    }
    stdout_full(room, "STATE");
    room.updates.send(room.revision).ok();
    let view = player_view(room, actor);
    let should_advance = room.game.as_ref().is_some_and(round_finished);
    if should_advance {
        schedule_next_round(state.clone(), room.name.clone(), room.revision);
    }
    Ok(Json(ServerMessage::Updated {
        revision: room.revision,
        events: outcome.events,
        view,
    }))
}

async fn room_ws(
    State(state): State<AppState>,
    Path(room_name): Path<String>,
    Query(query): Query<WsQuery>,
    upgrade: WebSocketUpgrade,
) -> ApiResult<Response> {
    let handle = find_room(&state, &room_name).await?;
    let role = query.role.unwrap_or_else(|| "observer".into());
    let seat = query.seat.map(Seat);
    let credential = query.credential;
    {
        let room = handle.lock().await;
        ensure_room_active(&room)?;
        make_view(&room, Some(&role), query.seat, credential.as_deref())?;
    }
    Ok(upgrade.on_upgrade(move |socket| websocket(socket, state, handle, role, seat, credential)))
}

async fn websocket(
    socket: WebSocket,
    state: AppState,
    handle: Arc<Mutex<Room>>,
    role: String,
    seat: Option<Seat>,
    credential: Option<String>,
) {
    let (mut sender, mut receiver) = socket.split();
    let (mut updates, generation) = {
        let mut room = handle.lock().await;
        if room.deleted {
            return;
        }
        let generation = if role == "player" {
            let Some(seat) = seat else { return };
            room.connections[seat.index()] += 1;
            room.connection_generation[seat.index()] += 1;
            room.connection_generation[seat.index()]
        } else {
            0
        };
        let Ok(view) = make_view(
            &room,
            Some(&role),
            seat.map(|seat| seat.0),
            credential.as_deref(),
        ) else {
            return;
        };
        if sender
            .send(Message::Text(
                serde_json::to_string(&ServerMessage::Snapshot(view))
                    .unwrap()
                    .into(),
            ))
            .await
            .is_err()
        {
            return;
        }
        (room.updates.subscribe(), generation)
    };

    loop {
        tokio::select! {
            update = updates.recv() => {
                match update {
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
                let room = handle.lock().await;
                if room.deleted {
                    drop(room);
                    let _ = sender.send(Message::Close(None)).await;
                    break;
                }
                let Ok(view) = make_view(&room, Some(&role), seat.map(|seat| seat.0), credential.as_deref()) else { break };
                if sender.send(Message::Text(serde_json::to_string(&ServerMessage::Snapshot(view)).unwrap().into())).await.is_err() {
                    break;
                }
            }
            message = receiver.next() => {
                let Some(Ok(Message::Text(text))) = message else { break };
                let parsed = serde_json::from_str::<ClientMessage>(&text);
                let response = match parsed {
                    Ok(ClientMessage::Ping) => ServerMessage::Pong,
                    Ok(ClientMessage::Act { command_id, expected_revision, action }) if role == "player" => {
                        let Some(actor) = seat else { break };
                        let mut room = handle.lock().await;
                        if room.deleted {
                            break;
                        } else if room.connection_generation[actor.index()] != generation {
                            ServerMessage::Error(ApiError { code: "superseded_connection".into(), message: "a newer player connection controls this seat".into(), current_revision: Some(room.revision) })
                        } else if let Some(token) = credential.as_deref() {
                            match authenticate_seat(&room, actor, token) {
                                Ok(()) => match apply_player_action(&state, &mut room, actor, command_id, expected_revision, action).await {
                                    Ok(Json(message)) => message,
                                    Err(error) => ServerMessage::Error(error.body),
                                },
                                Err(error) => ServerMessage::Error(error.body),
                            }
                        } else {
                            ServerMessage::Error(ApiError { code: "unauthorized".into(), message: "missing token".into(), current_revision: Some(room.revision) })
                        }
                    }
                    _ => ServerMessage::Error(ApiError { code: "invalid_message".into(), message: "message is not valid for this connection".into(), current_revision: None }),
                };
                if sender.send(Message::Text(serde_json::to_string(&response).unwrap().into())).await.is_err() {
                    break;
                }
            }
        }
    }
    if role == "player"
        && let Some(seat) = seat
    {
        let mut room = handle.lock().await;
        room.connections[seat.index()] = room.connections[seat.index()].saturating_sub(1);
        if !room.deleted {
            room.updates.send(room.revision).ok();
        }
    }
}

fn make_view(
    room: &Room,
    role: Option<&str>,
    seat: Option<u8>,
    credential: Option<&str>,
) -> ApiResult<View> {
    match role.unwrap_or("observer") {
        "observer" => Ok(View::Observer(public_view(room))),
        "player" => {
            let seat = Seat(seat.ok_or_else(|| {
                ApiFailure::bad_request("seat_required", "player view needs a seat")
            })?);
            authenticate_seat(room, seat, credential.unwrap_or_default())?;
            Ok(player_view(room, seat))
        }
        "referee" => {
            if credential != Some(room.referee_password.as_str()) {
                return Err(ApiFailure::unauthorized("wrong referee password"));
            }
            Ok(make_referee_view(room))
        }
        _ => Err(ApiFailure::bad_request(
            "unknown_role",
            "role must be observer, player, or referee",
        )),
    }
}

fn make_referee_view(room: &Room) -> View {
    let public = public_view(room);
    View::Referee {
        state: room.game.clone().unwrap_or_else(|| {
            Match::new(
                ["Seat 1", "Seat 2", "Seat 3"].map(String::from),
                room.config.clone(),
                Seat::ONE,
            )
        }),
        public,
        base_seed: room.base_seed,
        round_audits: room.round_audits.clone(),
    }
}

fn schedule_next_round(state: AppState, room_name: String, expected_revision: u64) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(2_500)).await;
        let Ok(handle) = find_room(&state, &room_name).await else {
            return;
        };
        let mut room = handle.lock().await;
        if room.deleted
            || room.revision != expected_revision
            || room.lifecycle != Lifecycle::Running
        {
            return;
        }
        let base_seed = room.base_seed;
        let match_index = room.match_index;
        let Some(game) = room.game.as_mut() else {
            return;
        };
        if game.acknowledge_round().is_err() {
            return;
        }
        let round_index = game.round_index;
        let seed = Match::derive_round_seed(base_seed, match_index, round_index);
        let Ok(deal) = game.deal_seeded_with_report(seed) else {
            return;
        };
        let audit = RoundAudit {
            match_index,
            round_index,
            derived_seed: seed,
            deal_order: deal.order,
            four_nines_reshuffles: deal.four_nines_reshuffles,
        };
        let shuffle_detail = shuffle_audit_detail(&audit);
        room.presentation = deal_presentation(now_ms(), audit.four_nines_reshuffles);
        room.round_audits.push(audit);
        room.revision += 1;
        if persist(&state.db, &room).await.is_err() {
            error!("failed to persist automatic round");
            return;
        }
        stdout_line(&room.name, "SHUFFLE", shuffle_detail);
        stdout_full(&room, "STATE");
        room.updates.send(room.revision).ok();
        schedule_presentation(state.clone(), room.name.clone(), room.revision);
    });
}

fn schedule_presentation(state: AppState, room_name: String, expected_revision: u64) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            let Ok(handle) = find_room(&state, &room_name).await else {
                return;
            };
            let mut room = handle.lock().await;
            if room.deleted || room.revision != expected_revision {
                return;
            }
            update_presentation(&mut room);
            room.updates.send(room.revision).ok();
            if room.presentation.stage == PresentationStage::Ready {
                let _ = persist(&state.db, &room).await;
                return;
            }
        }
    });
}

fn publish_round_audits(room: &mut Room) {
    let Some(game) = room.game.as_mut() else {
        return;
    };
    for audit in &room.round_audits {
        game.events.push(tysiac::GameEvent {
            kind: tysiac::EventKind::Deal,
            message: published_round_audit_message(audit),
        });
    }
}

fn public_view(room: &Room) -> PublicView {
    let mut view = project_public(
        room.name.clone(),
        room.revision,
        room.lifecycle,
        room.game.as_ref(),
        &connected(&room.connections),
    );
    if room.game.is_none() {
        view.players = room
            .seats
            .iter()
            .zip(Seat::ALL)
            .map(|(identity, seat)| PublicPlayer {
                seat,
                name: identity.name.clone(),
                score: 0,
                connected: room.connections[seat.index()] > 0,
                card_count: 0,
            })
            .collect();
    }
    let now = now_ms();
    let (stage, visible_deal_cards) = if room.presentation.stage == PresentationStage::Ready
        || now >= room.presentation.gate_until_ms
    {
        ("ready", 24)
    } else if now < room.presentation.deal_started_ms {
        ("shuffling", 0)
    } else {
        (
            "dealing",
            usize::try_from((now - room.presentation.deal_started_ms) / 250)
                .unwrap_or(24)
                .min(24),
        )
    };
    view.presentation = PresentationView {
        stage: stage.into(),
        visible_deal_cards,
        input_blocked: now < room.presentation.gate_until_ms,
    };
    view
}

fn player_view(room: &Room, seat: Seat) -> View {
    let projected = project_player(
        room.name.clone(),
        room.revision,
        room.lifecycle,
        room.game.as_ref(),
        &connected(&room.connections),
        seat,
    );
    match projected {
        View::Player {
            seat,
            own_hand,
            legal_actions,
            ..
        } => View::Player {
            seat,
            own_hand,
            public: public_view(room),
            legal_actions,
        },
        other => other,
    }
}

fn authenticate_seat(room: &Room, seat: Seat, token: &str) -> ApiResult<()> {
    if !seat.valid() {
        return Err(ApiFailure::bad_request(
            "invalid_seat",
            "seat must be 0, 1, or 2",
        ));
    }
    if room
        .seats
        .get(seat.index())
        .is_some_and(|identity| identity.token == token)
    {
        Ok(())
    } else {
        Err(ApiFailure::unauthorized("invalid seat token"))
    }
}

fn check_command(room: &Room, id: &str, expected_revision: u64) -> ApiResult<()> {
    if id.is_empty() || id.len() > 128 {
        return Err(ApiFailure::bad_request(
            "invalid_command_id",
            "command id must contain 1 to 128 characters",
        ));
    }
    if !room.recent_commands.contains(&id.to_owned()) && expected_revision != room.revision {
        return Err(ApiFailure::conflict(
            "revision_conflict",
            "state changed; refresh and try again",
            Some(room.revision),
        ));
    }
    Ok(())
}

fn remember_command(room: &mut Room, id: String) {
    room.recent_commands.push_back(id);
    while room.recent_commands.len() > 256 {
        room.recent_commands.pop_front();
    }
}

fn presentation_delay(action: &Action, events: &[tysiac::GameEvent]) -> u64 {
    if events
        .iter()
        .any(|event| event.kind == tysiac::EventKind::Trick)
    {
        1_200
    } else {
        match action {
            Action::PlayCard { .. } => 700,
            Action::Pass | Action::Bid { .. } => 350,
            Action::ContinueAfterTalon | Action::Transfer { .. } => 1_000,
            _ => 300,
        }
    }
}

fn update_presentation(room: &mut Room) {
    if room.presentation.stage == PresentationStage::Ready {
        return;
    }
    let now = now_ms();
    if now < room.presentation.deal_started_ms {
        room.presentation.stage = PresentationStage::Shuffling;
    } else {
        room.presentation.stage = PresentationStage::Dealing;
        room.presentation.visible_deal_cards =
            usize::try_from((now - room.presentation.deal_started_ms) / 250)
                .unwrap_or(24)
                .min(24);
    }
    if now >= room.presentation.gate_until_ms {
        room.presentation.stage = PresentationStage::Ready;
        room.presentation.visible_deal_cards = 24;
    }
}

async fn persist(db: &SqlitePool, room: &Room) -> ApiResult<()> {
    ensure_room_active(room)?;
    let json = serde_json::to_string(room)
        .map_err(|error| ApiFailure::internal(format!("serializing room: {error}")))?;
    sqlx::query(
        "INSERT INTO rooms(name, state_json, updated_at) VALUES(?1, ?2, ?3)
         ON CONFLICT(name) DO UPDATE SET state_json=excluded.state_json, updated_at=excluded.updated_at",
    )
    .bind(&room.name)
    .bind(json)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(db)
    .await
    .map_err(|error| ApiFailure::internal(format!("saving room: {error}")))?;
    Ok(())
}

async fn find_room(state: &AppState, name: &str) -> ApiResult<Arc<Mutex<Room>>> {
    state
        .rooms
        .read()
        .await
        .get(name)
        .cloned()
        .ok_or_else(room_not_found)
}

fn ensure_room_active(room: &Room) -> ApiResult<()> {
    if room.deleted {
        Err(room_not_found())
    } else {
        Ok(())
    }
}

fn room_not_found() -> ApiFailure {
    ApiFailure::not_found("room_not_found", "room does not exist")
}

fn join_response(room: &str, index: usize, seat: &SeatIdentity) -> JoinedRoom {
    JoinedRoom {
        seat: Seat::ALL[index],
        name: seat.name.clone(),
        token: seat.token.clone(),
        player_url: format!(
            "/room/{room}/player/{}/{}",
            index + 1,
            urlencoding::encode(&seat.name)
        ),
    }
}

fn validate_identifier(value: &str, field: &str) -> ApiResult<()> {
    if value.trim().is_empty()
        || value.len() > 48
        || value.chars().any(char::is_control)
        || value
            .chars()
            .any(|character| matches!(character, '/' | '?' | '#' | '%'))
    {
        Err(ApiFailure::bad_request(
            "invalid_identifier",
            format!("{field} must contain 1 to 48 printable characters"),
        ))
    } else {
        Ok(())
    }
}

fn validate_secret(value: &str, field: &str) -> ApiResult<()> {
    if value.is_empty()
        || value.len() > 128
        || value
            .chars()
            .any(|character| matches!(character, '\r' | '\n' | '\0'))
    {
        Err(ApiFailure::bad_request(
            "invalid_secret",
            format!("{field} must contain 1 to 128 single-line characters"),
        ))
    } else {
        Ok(())
    }
}

fn parse_seed(value: &str) -> ApiResult<u64> {
    let parsed = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map_or_else(|| value.parse::<u64>(), |hex| u64::from_str_radix(hex, 16))
        .map_err(|_| {
            ApiFailure::bad_request(
                "invalid_seed",
                "seed must be an unsigned decimal or 0x-prefixed hexadecimal integer",
            )
        })?;
    Ok(parsed)
}

fn time_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_secs().rotate_left(32) ^ u64::from(duration.subsec_nanos())
        })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration
                .as_secs()
                .saturating_mul(1_000)
                .saturating_add(u64::from(duration.subsec_millis()))
        })
}

fn connected(counts: &[usize; 3]) -> [bool; 3] {
    counts.map(|count| count > 0)
}

fn stdout_secrets(room: &Room, verb: &str) {
    stdout_line(
        &room.name,
        verb,
        format!(
            "player_password={} referee_password={} base_seed={}",
            escaped(&room.player_password),
            escaped(&room.referee_password),
            room.base_seed
        ),
    );
    for (index, seat) in room.seats.iter().enumerate() {
        stdout_line(
            &room.name,
            verb,
            format!(
                "seat={} name={} token={}",
                index + 1,
                escaped(&seat.name),
                seat.token
            ),
        );
    }
}

fn stdout_full(room: &Room, verb: &str) {
    match serde_json::to_string(room) {
        Ok(json) => stdout_line(&room.name, verb, json),
        Err(error) => error!(%error, "failed to serialize omniscient log"),
    }
}

fn stdout_line(room: &str, verb: &str, detail: impl AsRef<str>) {
    println!(
        "room={} {} {}",
        escaped(room),
        verb,
        escaped(detail.as_ref())
    );
}

fn escaped(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn rule_failure(error: &tysiac::RuleError) -> ApiFailure {
    ApiFailure::conflict("rule_violation", error.to_string(), None)
}

type ApiResult<T> = Result<T, ApiFailure>;

#[derive(Debug)]
struct ApiFailure {
    status: StatusCode,
    body: ApiError,
}

impl ApiFailure {
    fn new(
        status: StatusCode,
        code: impl Into<String>,
        message: impl Into<String>,
        revision: Option<u64>,
    ) -> Self {
        Self {
            status,
            body: ApiError {
                code: code.into(),
                message: message.into(),
                current_revision: revision,
            },
        }
    }

    fn bad_request(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message, None)
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message, None)
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message, None)
    }

    fn not_found(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, code, message, None)
    }

    fn conflict(
        code: impl Into<String>,
        message: impl Into<String>,
        revision: Option<u64>,
    ) -> Self {
        Self::new(StatusCode::CONFLICT, code, message, revision)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message, None)
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        (self.status, Json(ServerMessage::Error(self.body))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_state() -> AppState {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE rooms (
                name TEXT PRIMARY KEY NOT NULL,
                state_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
        )
        .execute(&db)
        .await
        .unwrap();
        AppState {
            db,
            rooms: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    fn room() -> Room {
        Room {
            name: "test-table".into(),
            player_password: "players".into(),
            referee_password: "referee".into(),
            base_seed: 0x1234,
            match_index: 0,
            config: Config::default(),
            seats: ["Ada", "Bert", "Celina"]
                .map(|name| SeatIdentity {
                    name: name.into(),
                    token: format!("token-{name}"),
                })
                .to_vec(),
            lifecycle: Lifecycle::Lobby,
            revision: 0,
            game: None,
            round_audits: Vec::new(),
            presentation: Presentation::default(),
            recent_commands: VecDeque::new(),
            connections: [0; 3],
            connection_generation: [0; 3],
            deleted: false,
            updates: broadcast_channel(),
        }
    }

    async fn lobby_state() -> AppState {
        let state = test_state().await;
        let mut room = room();
        room.seats.clear();
        persist(&state.db, &room).await.unwrap();
        state
            .rooms
            .write()
            .await
            .insert(room.name.clone(), Arc::new(Mutex::new(room)));
        state
    }

    async fn join(state: &AppState, name: &str) -> JoinedRoom {
        let Json(joined) = join_room(
            State(state.clone()),
            Path("test-table".into()),
            Json(JoinRoom {
                name: name.into(),
                password_or_token: "players".into(),
            }),
        )
        .await
        .unwrap();
        joined
    }

    #[tokio::test]
    async fn room_deletion_accepts_ipv4_and_ipv6_loopback_and_removes_all_state() {
        for peer in ["127.0.0.1:12345", "[::1]:12345"] {
            let state = lobby_state().await;
            let handle = find_room(&state, "test-table").await.unwrap();
            let mut updates = handle.lock().await.updates.subscribe();

            let status = delete_room(
                ConnectInfo(peer.parse().unwrap()),
                State(state.clone()),
                Path("test-table".into()),
            )
            .await
            .unwrap();

            assert_eq!(status, StatusCode::NO_CONTENT);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(100), updates.recv())
                    .await
                    .is_ok()
            );
            assert!(handle.lock().await.deleted);
            assert!(state.rooms.read().await.get("test-table").is_none());
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rooms WHERE name = ?1")
                .bind("test-table")
                .fetch_one(&state.db)
                .await
                .unwrap();
            assert_eq!(count, 0);
        }
    }

    #[tokio::test]
    async fn room_deletion_rejects_non_loopback_peers_without_changing_state() {
        let state = lobby_state().await;

        let error = delete_room(
            ConnectInfo("192.0.2.1:12345".parse().unwrap()),
            State(state.clone()),
            Path("test-table".into()),
        )
        .await
        .unwrap_err();

        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert_eq!(error.body.code, "forbidden");
        assert!(state.rooms.read().await.contains_key("test-table"));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rooms WHERE name = ?1")
            .bind("test-table")
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn deleting_a_missing_room_returns_not_found() {
        let state = test_state().await;

        let error = delete_room(
            ConnectInfo("127.0.0.1:12345".parse().unwrap()),
            State(state),
            Path("missing".into()),
        )
        .await
        .unwrap_err();

        assert_eq!(error.status, StatusCode::NOT_FOUND);
        assert_eq!(error.body.code, "room_not_found");
    }

    #[test]
    fn parses_decimal_and_hex_seeds_without_guessing() {
        assert_eq!(parse_seed("42").ok(), Some(42));
        assert_eq!(parse_seed("0x2a").ok(), Some(42));
        assert!(parse_seed("-1").is_err());
        assert!(parse_seed("2a").is_err());
    }

    #[test]
    fn start_creates_reproducible_audit_and_an_eight_second_gate() {
        let mut first = room();
        let mut second = room();
        start_match(&mut first).unwrap();
        start_match(&mut second).unwrap();
        assert_eq!(first.round_audits, second.round_audits);
        assert_eq!(first.round_audits[0].deal_order.len(), 24);
        assert_eq!(first.round_audits[0].four_nines_reshuffles, 0);
        assert_eq!(first.presentation.stage, PresentationStage::Shuffling);
        assert!(first.presentation.gate_until_ms >= first.presentation.deal_started_ms + 6_000);
        let zero_retry = deal_presentation(100, 0);
        assert_eq!(zero_retry.deal_started_ms, 2_100);
        assert_eq!(zero_retry.gate_until_ms, 8_100);
        let MatchPhase::Round(round) = &first.game.as_ref().unwrap().phase else {
            panic!()
        };
        assert_eq!(round.hands.each_ref().map(Vec::len), [7, 7, 7]);
        assert_eq!(round.talon.len(), 3);
    }

    #[test]
    fn retry_is_audited_published_and_given_extra_presentation_time() {
        let mut room = room();
        room.base_seed = 218;
        start_match(&mut room).unwrap();

        let audit = room.round_audits[0].clone();
        assert_eq!(audit.derived_seed, 8_934_752_883_078_774_011);
        assert_eq!(audit.four_nines_reshuffles, 1);
        assert_eq!(audit.deal_order.len(), 24);
        assert_eq!(
            room.game.as_ref().unwrap().events[0].message,
            "1 invalid all-four-nines deal was reshuffled"
        );

        let one_retry = deal_presentation(100, audit.four_nines_reshuffles);
        assert_eq!(one_retry.deal_started_ms, 4_100);
        assert_eq!(one_retry.gate_until_ms, 10_100);
        let saturated = deal_presentation(u64::MAX - 1_000, u64::MAX);
        assert_eq!(saturated.deal_started_ms, u64::MAX);
        assert_eq!(saturated.gate_until_ms, u64::MAX);

        let stdout = shuffle_audit_detail(&audit);
        assert!(stdout.contains("four_nines_reshuffles=1"));
        assert!(stdout.contains("reshuffle_reason=initial_hand_contained_all_four_nines"));
        assert!(stdout.contains("accepted_deal_order="));

        let published = published_round_audit_message(&audit);
        publish_round_audits(&mut room);
        assert_eq!(
            room.game.as_ref().unwrap().events.last().unwrap().message,
            published
        );
        assert!(
            published
                .contains("1 deal was reshuffled because an initial hand contained all four nines")
        );
        assert!(published.contains("accepted deal"));
    }

    #[tokio::test]
    async fn match_does_not_start_before_three_players_join() {
        let state = lobby_state().await;
        join(&state, "Ada").await;
        join(&state, "Bert").await;

        let handle = find_room(&state, "test-table").await.unwrap();
        let room = handle.lock().await;
        assert_eq!(room.seats.len(), 2);
        assert_eq!(room.lifecycle, Lifecycle::Lobby);
        assert!(room.game.is_none());
        assert_eq!(room.revision, 2);
    }

    #[tokio::test]
    async fn third_join_atomically_starts_and_persists_the_match() {
        let state = lobby_state().await;
        join(&state, "Ada").await;
        join(&state, "Bert").await;
        let handle = find_room(&state, "test-table").await.unwrap();
        let mut updates = handle.lock().await.updates.subscribe();

        let joined = join(&state, "Celina").await;

        assert_eq!(joined.seat, Seat::THREE);
        assert_eq!(joined.name, "Celina");
        assert!(!joined.token.is_empty());
        assert_eq!(updates.recv().await.unwrap(), 3);

        let room = handle.lock().await;
        assert_eq!(room.seats.len(), 3);
        assert_eq!(room.lifecycle, Lifecycle::Running);
        assert!(room.game.is_some());
        assert_eq!(room.revision, 3);
        assert_eq!(room.round_audits.len(), 1);
        assert_eq!(room.seats[2].token, joined.token);
        drop(room);

        let json: String = sqlx::query("SELECT state_json FROM rooms WHERE name = ?1")
            .bind("test-table")
            .fetch_one(&state.db)
            .await
            .unwrap()
            .try_get("state_json")
            .unwrap();
        let persisted: Room = serde_json::from_str(&json).unwrap();
        assert_eq!(persisted.lifecycle, Lifecycle::Running);
        assert_eq!(persisted.revision, 3);
        assert_eq!(persisted.seats.len(), 3);
        assert!(persisted.game.is_some());
    }

    #[tokio::test]
    async fn referee_credential_is_generated_when_absent_or_empty_and_honors_explicit_input() {
        let state = test_state().await;
        let omitted: CreateRoom = serde_json::from_value(serde_json::json!({
            "name": "omitted",
            "player_password": "players"
        }))
        .unwrap();
        let empty: CreateRoom = serde_json::from_value(serde_json::json!({
            "name": "empty",
            "player_password": "players",
            "referee_password": ""
        }))
        .unwrap();
        let explicit: CreateRoom = serde_json::from_value(serde_json::json!({
            "name": "explicit",
            "player_password": "players",
            "referee_password": "chosen-referee-secret"
        }))
        .unwrap();

        let Json(omitted) = create_room(State(state.clone()), Json(omitted))
            .await
            .unwrap();
        let Json(empty) = create_room(State(state.clone()), Json(empty))
            .await
            .unwrap();
        let Json(explicit) = create_room(State(state), Json(explicit)).await.unwrap();

        for generated in [&omitted.referee_password, &empty.referee_password] {
            assert_eq!(generated.len(), 32);
            assert!(
                generated
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
            );
        }
        assert_ne!(omitted.referee_password, empty.referee_password);
        assert_eq!(explicit.referee_password, "chosen-referee-secret");
    }

    #[test]
    fn observer_projection_omits_base_seed_and_hidden_hands() {
        let mut room = room();
        start_match(&mut room).unwrap();
        let json = serde_json::to_string(&View::Observer(public_view(&room))).unwrap();
        assert!(!json.contains("\"base_seed\""));
        assert!(!json.contains("\"hands\""));
        assert!(json.contains("\"input_blocked\":true"));
        let referee = serde_json::to_string(&make_referee_view(&room)).unwrap();
        assert!(referee.contains("\"base_seed\":4660"));
        assert!(referee.contains("\"hands\""));
    }

    #[test]
    fn revision_and_command_id_make_retries_idempotent() {
        let mut room = room();
        room.revision = 7;
        assert!(check_command(&room, "abc", 6).is_err());
        assert!(check_command(&room, "abc", 7).is_ok());
        remember_command(&mut room, "abc".into());
        assert!(check_command(&room, "abc", 1).is_ok());
    }

    #[tokio::test]
    async fn sqlite_round_trip_restores_credentials_tokens_and_state() {
        let state = test_state().await;
        let mut original = room();
        start_match(&mut original).unwrap();
        persist(&state.db, &original).await.unwrap();
        let restored = restore_rooms(&state.db).await.unwrap();
        let restored = restored["test-table"].lock().await;
        assert_eq!(restored.player_password, "players");
        assert_eq!(restored.referee_password, "referee");
        assert_eq!(restored.seats[1].token, "token-Bert");
        assert_eq!(restored.round_audits, original.round_audits);
    }

    #[test]
    fn one_line_log_escaping_prevents_injected_lines() {
        assert_eq!(escaped("a\nb\rc\td\\e"), "a\\nb\\rc\\td\\\\e");
    }
}
