//! Wire types shared by the game server and every client.

use cards::Card;
use serde::{Deserialize, Serialize};
use tysiac::{Action, Config, GameEvent, Seat};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Authenticate {
        credential: String,
    },
    Act {
        command_id: String,
        expected_revision: u64,
        action: Action,
    },
    Admin {
        command_id: String,
        expected_revision: u64,
        action: AdminAction,
    },
    Ping,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminAction {
    Start,
    Pause,
    Resume,
    Abort,
    Rematch,
    AdvancePresentation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Snapshot(View),
    Updated {
        revision: u64,
        events: Vec<GameEvent>,
        view: View,
    },
    Error(ApiError),
    Pong,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub current_revision: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum View {
    Observer(PublicView),
    Player {
        seat: Seat,
        own_hand: Vec<Card>,
        public: PublicView,
        legal_actions: Vec<Action>,
    },
    Referee {
        state: tysiac::Match,
        public: PublicView,
        base_seed: u64,
        round_audits: Vec<RoundAudit>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PublicView {
    pub room_name: String,
    pub revision: u64,
    pub lifecycle: Lifecycle,
    pub config: Config,
    pub players: Vec<PublicPlayer>,
    pub game: Option<PublicGame>,
    pub presentation: PresentationView,
    pub history: Vec<GameEvent>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PresentationView {
    pub stage: String,
    pub visible_deal_cards: usize,
    pub input_blocked: bool,
}

impl Default for PresentationView {
    fn default() -> Self {
        Self {
            stage: "ready".into(),
            visible_deal_cards: 24,
            input_blocked: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Lobby,
    Running,
    Paused,
    Finished,
    Aborted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PublicPlayer {
    pub seat: Seat,
    pub name: String,
    pub score: i32,
    pub connected: bool,
    pub card_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PublicGame {
    pub phase: String,
    pub dealer: Seat,
    pub turn: Option<Seat>,
    pub contractor: Option<Seat>,
    pub bid_or_contract: Option<u16>,
    pub trump: Option<cards::Suit>,
    pub current_trick: Vec<PublicPlayed>,
    pub last_trick: Vec<PublicPlayed>,
    pub open_hands: Vec<(Seat, Vec<Card>)>,
    pub talon: Vec<PublicCard>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PublicPlayed {
    pub seat: Seat,
    pub card: Card,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RoundAudit {
    pub match_index: u64,
    pub round_index: u64,
    pub derived_seed: u64,
    pub deal_order: Vec<Card>,
    #[serde(default)]
    pub four_nines_reshuffles: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "visibility", content = "card", rename_all = "snake_case")]
pub enum PublicCard {
    Face(Card),
    Back,
}

#[must_use]
pub fn project_public(
    room_name: String,
    revision: u64,
    lifecycle: Lifecycle,
    game: Option<&tysiac::Match>,
    connected: &[bool; 3],
) -> PublicView {
    let config = game.map_or_else(Config::default, |game| game.config.clone());
    let players = game.map_or_else(Vec::new, |game| {
        game.players
            .iter()
            .zip(Seat::ALL)
            .map(|(player, seat)| PublicPlayer {
                seat,
                name: player.name.clone(),
                score: player.score,
                connected: connected[seat.index()],
                card_count: match &game.phase {
                    tysiac::MatchPhase::Round(round) => round.hands[seat.index()].len(),
                    _ => 0,
                },
            })
            .collect()
    });
    PublicView {
        room_name,
        revision,
        lifecycle,
        config,
        players,
        game: game.and_then(project_game),
        presentation: PresentationView::default(),
        history: game.map_or_else(Vec::new, |game| {
            game.events
                .iter()
                .filter(|event| event.kind != tysiac::EventKind::Transfer)
                .cloned()
                .collect()
        }),
    }
}

#[must_use]
pub fn project_player(
    room_name: String,
    revision: u64,
    lifecycle: Lifecycle,
    game: Option<&tysiac::Match>,
    connected: &[bool; 3],
    seat: Seat,
) -> View {
    let public = project_public(room_name, revision, lifecycle, game, connected);
    let own_hand = game
        .and_then(|game| match &game.phase {
            tysiac::MatchPhase::Round(round) => Some(round.hands[seat.index()].clone()),
            _ => None,
        })
        .unwrap_or_default();
    let legal_actions = if lifecycle == Lifecycle::Running {
        game.map_or_else(Vec::new, |game| tysiac::legal_actions(game, seat))
    } else {
        Vec::new()
    };
    View::Player {
        seat,
        own_hand,
        public,
        legal_actions,
    }
}

fn project_game(game: &tysiac::Match) -> Option<PublicGame> {
    let tysiac::MatchPhase::Round(round) = &game.phase else {
        return None;
    };
    let mut projected = PublicGame {
        phase: phase_name(&round.phase).to_owned(),
        dealer: round.dealer,
        turn: None,
        contractor: None,
        bid_or_contract: None,
        trump: None,
        current_trick: Vec::new(),
        last_trick: Vec::new(),
        open_hands: Vec::new(),
        talon: round
            .talon
            .iter()
            .map(|card| {
                if round.publicly_revealed.contains(card) {
                    PublicCard::Face(*card)
                } else {
                    PublicCard::Back
                }
            })
            .collect(),
    };
    match &round.phase {
        tysiac::RoundPhase::Auction(auction) => {
            projected.turn = auction
                .proof
                .as_ref()
                .and_then(|proof| {
                    proof
                        .responder()
                        .or(proof.reveal_required.then_some(proof.bidder))
                })
                .or(Some(auction.turn));
            projected.contractor = Some(auction.highest_bidder);
            projected.bid_or_contract = Some(auction.highest_bid);
        }
        tysiac::RoundPhase::Talon(talon) => {
            projected.turn = Some(talon.contractor);
            projected.contractor = Some(talon.contractor);
            projected.bid_or_contract = Some(talon.auction_bid);
            if talon.public {
                projected.talon = round.talon.iter().copied().map(PublicCard::Face).collect();
            }
        }
        tysiac::RoundPhase::Transfer(transfer) => {
            projected.turn = Some(transfer.contractor);
            projected.contractor = Some(transfer.contractor);
            projected.bid_or_contract = Some(transfer.auction_bid);
        }
        tysiac::RoundPhase::Contract(contract) => {
            projected.turn = contract
                .proof
                .as_ref()
                .and_then(|proof| {
                    proof
                        .responder()
                        .or(proof.reveal_required.then_some(proof.bidder))
                })
                .or(Some(contract.contractor));
            projected.contractor = Some(contract.contractor);
            projected.bid_or_contract = Some(contract.points);
        }
        tysiac::RoundPhase::Play(play) => project_play(round, play, &mut projected),
        tysiac::RoundPhase::ClaimVote(vote) => project_claim_vote(vote, &mut projected),
        tysiac::RoundPhase::RoundFinished(result) => {
            projected.contractor = Some(result.contractor);
            projected.bid_or_contract = Some(result.contract);
        }
    }
    Some(projected)
}

fn project_play(round: &tysiac::Round, play: &tysiac::Play, projected: &mut PublicGame) {
    projected.turn = Some(play.turn);
    projected.contractor = Some(play.contractor);
    projected.bid_or_contract = Some(play.contract);
    projected.trump = play.trump;
    projected.current_trick = public_plays(&play.trick);
    projected.last_trick = public_plays(&play.last_trick);
    projected.open_hands = Seat::ALL
        .into_iter()
        .filter(|seat| play.open_hands[seat.index()])
        .map(|seat| (seat, round.hands[seat.index()].clone()))
        .collect();
}

fn project_claim_vote(vote: &tysiac::ClaimVote, projected: &mut PublicGame) {
    projected.turn = vote.voters.get(vote.voter_index).copied();
    projected.contractor = Some(vote.play.contractor);
    projected.bid_or_contract = Some(vote.play.contract);
    projected.trump = vote.play.trump;
    projected.last_trick = public_plays(&vote.play.last_trick);
}

fn public_plays(plays: &[cards::PlayedCard<Seat>]) -> Vec<PublicPlayed> {
    plays
        .iter()
        .map(|played| PublicPlayed {
            seat: played.player,
            card: played.card,
        })
        .collect()
}

const fn phase_name(phase: &tysiac::RoundPhase) -> &'static str {
    match phase {
        tysiac::RoundPhase::Auction(_) => "auction",
        tysiac::RoundPhase::Talon(_) => "talon",
        tysiac::RoundPhase::Transfer(_) => "transfer",
        tysiac::RoundPhase::Contract(_) => "contract",
        tysiac::RoundPhase::Play(_) => "play",
        tysiac::RoundPhase::ClaimVote(_) => "claim_vote",
        tysiac::RoundPhase::RoundFinished(_) => "round_finished",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> tysiac::Match {
        let mut game = tysiac::Match::new(
            ["Ada", "Bert", "Celina"].map(String::from),
            Config::default(),
            Seat::ONE,
        );
        game.deal_ordered(12, tysiac::game_deck()).unwrap();
        game
    }

    #[test]
    fn observer_json_contains_backs_but_no_hands_or_seed() {
        let game = game();
        let view = View::Observer(project_public(
            "table".into(),
            1,
            Lifecycle::Running,
            Some(&game),
            &[true; 3],
        ));
        let json = serde_json::to_string(&view).unwrap();
        assert!(json.contains("\"visibility\":\"back\""));
        assert!(!json.contains("own_hand"));
        assert!(!json.contains("\"seed\":12"));
        assert!(!json.contains("\"hands\""));
    }

    #[test]
    fn player_projection_contains_only_their_hand() {
        let game = game();
        let View::Player {
            own_hand, public, ..
        } = project_player(
            "table".into(),
            1,
            Lifecycle::Running,
            Some(&game),
            &[true; 3],
            Seat::TWO,
        )
        else {
            panic!()
        };
        let tysiac::MatchPhase::Round(round) = &game.phase else {
            panic!()
        };
        assert_eq!(own_hand, round.hands[1]);
        let json = serde_json::to_string(&public).unwrap();
        for card in &round.hands[0] {
            assert!(!json.contains(&format!(
                "\"rank\":\"{}\",\"suit\":\"{}\"",
                rank_name(card.rank),
                suit_name(card.suit)
            )));
        }
    }

    #[test]
    fn private_transfer_events_are_not_in_public_history() {
        let mut game = game();
        game.events.push(GameEvent {
            kind: tysiac::EventKind::Transfer,
            message: "seat 1 gives A♥ to seat 2".into(),
        });
        let view = project_public(
            "table".into(),
            1,
            Lifecycle::Running,
            Some(&game),
            &[false; 3],
        );
        assert!(
            view.history
                .iter()
                .all(|event| event.kind != tysiac::EventKind::Transfer)
        );
        assert!(
            game.events
                .iter()
                .any(|event| event.kind == tysiac::EventKind::Transfer)
        );
    }

    #[test]
    fn old_round_audit_json_defaults_four_nines_reshuffles_to_zero() {
        let audit: RoundAudit = serde_json::from_value(serde_json::json!({
            "match_index": 2,
            "round_index": 3,
            "derived_seed": 42,
            "deal_order": []
        }))
        .unwrap();

        assert_eq!(audit.match_index, 2);
        assert_eq!(audit.round_index, 3);
        assert_eq!(audit.derived_seed, 42);
        assert!(audit.deal_order.is_empty());
        assert_eq!(audit.four_nines_reshuffles, 0);
    }

    fn rank_name(rank: cards::Rank) -> &'static str {
        match rank {
            cards::Rank::Nine => "nine",
            cards::Rank::Ten => "ten",
            cards::Rank::Jack => "jack",
            cards::Rank::Queen => "queen",
            cards::Rank::King => "king",
            cards::Rank::Ace => "ace",
            _ => "unused",
        }
    }

    fn suit_name(suit: cards::Suit) -> &'static str {
        match suit {
            cards::Suit::Clubs => "clubs",
            cards::Suit::Diamonds => "diamonds",
            cards::Suit::Hearts => "hearts",
            cards::Suit::Spades => "spades",
        }
    }
}
