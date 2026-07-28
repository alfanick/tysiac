//! Authoritative rules engine for three-player Tysiąc.
//!
//! The engine contains no networking, clocks, animation, or authentication. Every
//! client, including future bots, submits the same [`Action`] values.

use std::collections::BTreeSet;

use cards::{Card, PlayedCard, Rank, Suit, trick_winner};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PLAYER_COUNT: usize = 3;
pub const HAND_SIZE: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Seat(pub u8);

impl Seat {
    pub const ONE: Self = Self(0);
    pub const TWO: Self = Self(1);
    pub const THREE: Self = Self(2);
    pub const ALL: [Self; PLAYER_COUNT] = [Self::ONE, Self::TWO, Self::THREE];

    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    #[must_use]
    pub const fn next(self) -> Self {
        Self((self.0 + 1) % 3)
    }

    #[must_use]
    pub const fn valid(self) -> bool {
        self.0 < 3
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TalonVisibility {
    AlwaysPublic,
    HideAtOneHundred,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub target_score: i32,
    pub lock_score: i32,
    pub talon_visibility: TalonVisibility,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            target_score: 1_000,
            lock_score: 900,
            talon_visibility: TalonVisibility::AlwaysPublic,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Player {
    pub name: String,
    pub score: i32,
    pub free_surrender_used: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Match {
    pub config: Config,
    pub players: [Player; PLAYER_COUNT],
    pub dealer: Seat,
    pub match_index: u64,
    pub round_index: u64,
    pub phase: MatchPhase,
    pub events: Vec<GameEvent>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "match_phase", rename_all = "snake_case")]
pub enum MatchPhase {
    WaitingForDeal,
    Round(Box<Round>),
    MatchFinished { winner: Seat },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Round {
    pub seed: u64,
    pub dealer: Seat,
    pub locked_at_start: [bool; PLAYER_COUNT],
    pub hands: [Vec<Card>; PLAYER_COUNT],
    pub talon: Vec<Card>,
    pub publicly_revealed: BTreeSet<Card>,
    pub phase: RoundPhase,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "name", rename_all = "snake_case")]
pub enum RoundPhase {
    Auction(Auction),
    Talon(Talon),
    Transfer(Transfer),
    Contract(Contract),
    Play(Play),
    ClaimVote(ClaimVote),
    RoundFinished(RoundResult),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Auction {
    pub opener: Seat,
    pub turn: Seat,
    pub highest_bidder: Seat,
    pub highest_bid: u16,
    pub active: [bool; PLAYER_COUNT],
    pub proof: Option<Proof>,
    pub revealed_marriages: [BTreeSet<Suit>; PLAYER_COUNT],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofPurpose {
    Auction,
    Contract,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Proof {
    pub purpose: ProofPurpose,
    pub bidder: Seat,
    pub points: u16,
    pub responders: [Seat; 2],
    pub responder_index: usize,
    pub reveal_required: bool,
}

impl Proof {
    #[must_use]
    pub fn responder(&self) -> Option<Seat> {
        (!self.reveal_required && self.responder_index < self.responders.len())
            .then(|| self.responders[self.responder_index])
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Talon {
    pub contractor: Seat,
    pub auction_bid: u16,
    pub public: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Transfer {
    pub contractor: Seat,
    pub auction_bid: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Contract {
    pub contractor: Seat,
    pub auction_bid: u16,
    pub points: u16,
    pub proof: Option<Proof>,
    pub revealed_marriages: BTreeSet<Suit>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Play {
    pub contractor: Seat,
    pub contract: u16,
    pub leader: Seat,
    pub turn: Seat,
    pub trump: Option<Suit>,
    pub trick: Vec<PlayedCard<Seat>>,
    pub last_trick: Vec<PlayedCard<Seat>>,
    pub captured: [Vec<Card>; PLAYER_COUNT],
    pub marriage_points: [u16; PLAYER_COUNT],
    pub declared_marriages: [BTreeSet<Suit>; PLAYER_COUNT],
    pub open_hands: [bool; PLAYER_COUNT],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClaimVote {
    pub play: Play,
    pub claimant: Seat,
    pub voters: [Seat; 2],
    pub voter_index: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RoundResult {
    pub contractor: Seat,
    pub contract: u16,
    pub raw_points: [u16; PLAYER_COUNT],
    pub deltas: [i32; PLAYER_COUNT],
    pub scores: [i32; PLAYER_COUNT],
    pub surrendered: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Bid { points: u16 },
    Pass,
    RespondToProof { reveal: bool },
    RevealProof { suits: Vec<Suit> },
    ContinueAfterTalon,
    Surrender,
    Transfer { gifts: Vec<Gift> },
    ConfirmContract { points: u16 },
    PlayCard { card: Card },
    ClaimRemaining,
    VoteOnClaim { accept: bool },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Gift {
    pub recipient: Seat,
    pub card: Card,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GameEvent {
    pub kind: EventKind,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Deal,
    Auction,
    Proof,
    Talon,
    Transfer,
    Contract,
    Marriage,
    Play,
    Trick,
    Claim,
    Score,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum RuleError {
    #[error("seat does not exist")]
    InvalidSeat,
    #[error("that action is not legal in the current phase")]
    WrongPhase,
    #[error("it is not that player's turn")]
    NotYourTurn,
    #[error("bid must be exactly ten above the current bid")]
    InvalidBidStep,
    #[error("bid or contract is above the marriages in hand")]
    UnsupportedPoints,
    #[error("player has already passed")]
    AlreadyPassed,
    #[error("proof response is not expected from this player")]
    NotProofResponder,
    #[error("proof reveal is not expected from this player")]
    NotProofBidder,
    #[error("proof is not a smallest sufficient set of held marriages")]
    InvalidProof,
    #[error("contract must be a multiple of ten and at least the auction bid")]
    InvalidContract,
    #[error("gift list must give one distinct held card to each opponent")]
    InvalidTransfer,
    #[error("card is not in the player's hand")]
    CardNotHeld,
    #[error("card does not satisfy follow-suit and beat-if-possible rules")]
    IllegalPlay,
    #[error("a claim can be made only by the leader before a trick starts")]
    CannotClaim,
    #[error("claim vote is not expected from this player")]
    NotClaimVoter,
    #[error("deck must contain each of the 24 game cards exactly once")]
    InvalidDeck,
    #[error("a round is already active or the match has ended")]
    CannotDeal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplyOutcome {
    pub events: Vec<GameEvent>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeededDeal {
    pub order: Vec<Card>,
    pub four_nines_reshuffles: u64,
}

impl Match {
    #[must_use]
    pub fn new(names: [String; PLAYER_COUNT], config: Config, dealer: Seat) -> Self {
        Self {
            config,
            players: names.map(|name| Player {
                name,
                score: 0,
                free_surrender_used: false,
            }),
            dealer,
            match_index: 0,
            round_index: 0,
            phase: MatchPhase::WaitingForDeal,
            events: Vec::new(),
        }
    }

    /// Derives an independent seed without exposing the room's base seed.
    #[must_use]
    pub fn derive_round_seed(base_seed: u64, match_index: u64, round_index: u64) -> u64 {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"mille/round-seed/v1");
        hasher.update(&base_seed.to_le_bytes());
        hasher.update(&match_index.to_le_bytes());
        hasher.update(&round_index.to_le_bytes());
        let bytes = hasher.finalize();
        let [
            first,
            second,
            third,
            fourth,
            fifth,
            sixth,
            seventh,
            eighth,
            ..,
        ] = *bytes.as_bytes();
        u64::from_le_bytes([first, second, third, fourth, fifth, sixth, seventh, eighth])
    }

    /// Shuffles and deals the next round from a stable seed, returning the
    /// accepted card order.
    ///
    /// A shuffle that gives one player all four nines in their initial hand is
    /// discarded and retried from a deterministic, domain-separated seed. The
    /// returned order is always the accepted shuffle, while [`Round::seed`]
    /// remains the supplied round seed.
    ///
    /// # Errors
    ///
    /// Returns [`RuleError::CannotDeal`] unless the match is waiting for a deal.
    pub fn deal_seeded(&mut self, seed: u64) -> Result<Vec<Card>, RuleError> {
        self.deal_seeded_with_report(seed).map(|deal| deal.order)
    }

    /// Shuffles and deals the next round, reporting any all-four-nines
    /// reshuffles.
    ///
    /// Rejected shuffles do not mutate the match or emit events. When at least
    /// one shuffle is rejected, one [`EventKind::Deal`] event reports the count
    /// immediately before the accepted deal and auction events.
    ///
    /// # Errors
    ///
    /// Returns [`RuleError::CannotDeal`] unless the match is waiting for a deal,
    /// or in the unreachable-in-practice event that the retry counter is
    /// exhausted.
    pub fn deal_seeded_with_report(&mut self, seed: u64) -> Result<SeededDeal, RuleError> {
        if !matches!(self.phase, MatchPhase::WaitingForDeal) {
            return Err(RuleError::CannotDeal);
        }

        let mut four_nines_reshuffles = 0_u64;
        loop {
            let mut deck =
                cards::Deck::from_cards(game_deck()).map_err(|_| RuleError::InvalidDeck)?;
            deck.shuffle_with_seed(deal_shuffle_seed(seed, four_nines_reshuffles));
            let order = deck.cards().to_vec();
            let (hands, _) = distribute_deal(&order);
            if hands.iter().any(|hand| has_all_nines(hand)) {
                four_nines_reshuffles = four_nines_reshuffles
                    .checked_add(1)
                    .ok_or(RuleError::CannotDeal)?;
                continue;
            }
            if four_nines_reshuffles > 0 {
                let deal_word = if four_nines_reshuffles == 1 {
                    "deal was"
                } else {
                    "deals were"
                };
                self.push(
                    EventKind::Deal,
                    format!(
                        "{four_nines_reshuffles} invalid all-four-nines {deal_word} reshuffled"
                    ),
                );
            }
            self.deal_ordered(seed, order.clone())?;
            return Ok(SeededDeal {
                order,
                four_nines_reshuffles,
            });
        }
    }

    /// Deals an explicitly ordered 24-card game deck.
    ///
    /// This is a setup and replay escape hatch: the supplied order is dealt
    /// exactly as provided, without applying the seeded all-four-nines
    /// reshuffle policy.
    ///
    /// # Errors
    ///
    /// Returns [`RuleError::CannotDeal`] when a round is active and
    /// [`RuleError::InvalidDeck`] when the supplied cards are not the exact game
    /// deck.
    pub fn deal_ordered(&mut self, seed: u64, deck: impl AsRef<[Card]>) -> Result<(), RuleError> {
        if !matches!(self.phase, MatchPhase::WaitingForDeal) {
            return Err(RuleError::CannotDeal);
        }
        let deck = deck.as_ref();
        validate_game_deck(deck)?;
        let (mut hands, talon) = distribute_deal(deck);
        for hand in &mut hands {
            sort_hand(hand);
        }
        let opener = self.dealer.next();
        let round = Round {
            seed,
            dealer: self.dealer,
            locked_at_start: std::array::from_fn(|index| {
                let score = self.players[index].score;
                score >= self.config.lock_score && score < self.config.target_score
            }),
            hands,
            talon,
            publicly_revealed: BTreeSet::new(),
            phase: RoundPhase::Auction(Auction {
                opener,
                turn: opener.next(),
                highest_bidder: opener,
                highest_bid: 100,
                active: [true; PLAYER_COUNT],
                proof: None,
                revealed_marriages: std::array::from_fn(|_| BTreeSet::new()),
            }),
        };
        self.phase = MatchPhase::Round(Box::new(round));
        self.push(
            EventKind::Deal,
            format!("round dealt; dealer is seat {}", self.dealer.0 + 1),
        );
        self.push(
            EventKind::Auction,
            format!("seat {} opens at 100", opener.0 + 1),
        );
        Ok(())
    }

    /// Applies one authoritative player action.
    ///
    /// # Errors
    ///
    /// Returns a [`RuleError`] when the actor, phase, turn, or move violates the
    /// game rules.
    pub fn apply(&mut self, actor: Seat, action: Action) -> Result<ApplyOutcome, RuleError> {
        if !actor.valid() {
            return Err(RuleError::InvalidSeat);
        }
        let start = self.events.len();
        let MatchPhase::Round(mut round) =
            std::mem::replace(&mut self.phase, MatchPhase::WaitingForDeal)
        else {
            return Err(RuleError::WrongPhase);
        };
        let result = self.apply_to_round(actor, action, &mut round);
        if result.is_err() {
            self.phase = MatchPhase::Round(round);
            return result.map(|()| unreachable!());
        }
        if matches!(round.phase, RoundPhase::RoundFinished(_)) {
            self.finish_round(&round);
            if !matches!(self.phase, MatchPhase::MatchFinished { .. }) {
                self.phase = MatchPhase::Round(round);
            }
        } else {
            self.phase = MatchPhase::Round(round);
        }
        Ok(ApplyOutcome {
            events: self.events[start..].to_vec(),
        })
    }

    fn apply_to_round(
        &mut self,
        actor: Seat,
        move_to_apply: Action,
        round: &mut Round,
    ) -> Result<(), RuleError> {
        match move_to_apply {
            Action::Bid { points } => self.apply_bid(actor, points, round),
            Action::Pass => self.apply_pass(actor, round),
            Action::RespondToProof { reveal } => self.apply_proof_response(actor, reveal, round),
            Action::RevealProof { suits } => self.apply_proof_reveal(actor, &suits, round),
            Action::ContinueAfterTalon => Self::continue_after_talon(actor, round),
            Action::Surrender => self.apply_surrender(actor, round),
            Action::Transfer { gifts } => self.apply_transfer(actor, gifts, round),
            Action::ConfirmContract { points } => self.apply_contract(actor, points, round),
            Action::PlayCard { card } => self.apply_card_play(actor, card, round),
            Action::ClaimRemaining => self.apply_claim(actor, round),
            Action::VoteOnClaim { accept } => self.apply_claim_vote(actor, accept, round),
        }
    }

    fn apply_bid(&mut self, actor: Seat, points: u16, round: &mut Round) -> Result<(), RuleError> {
        let RoundPhase::Auction(auction) = &mut round.phase else {
            return Err(RuleError::WrongPhase);
        };
        if auction.proof.is_some() {
            return Err(RuleError::WrongPhase);
        }
        if actor != auction.turn {
            return Err(RuleError::NotYourTurn);
        }
        if !auction.active[actor.index()] {
            return Err(RuleError::AlreadyPassed);
        }
        if points != auction.highest_bid + 10 {
            return Err(RuleError::InvalidBidStep);
        }
        ensure_supported(&round.hands[actor.index()], points)?;
        auction.highest_bid = points;
        auction.highest_bidder = actor;
        auction.turn = next_active(actor, auction.active).expect("bidder is active");
        self.push(
            EventKind::Auction,
            format!("seat {} bids {points}", actor.0 + 1),
        );
        if points > revealed_ceiling(&auction.revealed_marriages[actor.index()]) {
            auction.proof = Some(new_proof(ProofPurpose::Auction, actor, points));
            self.push(
                EventKind::Proof,
                format!("seat {} may be challenged for bid {points}", actor.0 + 1),
            );
        }
        Ok(())
    }

    fn apply_pass(&mut self, actor: Seat, round: &mut Round) -> Result<(), RuleError> {
        let RoundPhase::Auction(auction) = &mut round.phase else {
            return Err(RuleError::WrongPhase);
        };
        if auction.proof.is_some() {
            return Err(RuleError::WrongPhase);
        }
        if actor != auction.turn {
            return Err(RuleError::NotYourTurn);
        }
        if !auction.active[actor.index()] {
            return Err(RuleError::AlreadyPassed);
        }
        auction.active[actor.index()] = false;
        self.push(EventKind::Auction, format!("seat {} passes", actor.0 + 1));
        if auction.active.into_iter().filter(|active| *active).count() > 1 {
            auction.turn = next_active(actor, auction.active).expect("two active bidders remain");
            return Ok(());
        }
        let contractor = auction.highest_bidder;
        let auction_bid = auction.highest_bid;
        round.hands[contractor.index()].extend(round.talon.iter().copied());
        sort_hand(&mut round.hands[contractor.index()]);
        let public =
            self.config.talon_visibility == TalonVisibility::AlwaysPublic || auction_bid != 100;
        if public {
            round.publicly_revealed.extend(round.talon.iter().copied());
        }
        round.phase = RoundPhase::Talon(Talon {
            contractor,
            auction_bid,
            public,
        });
        self.push(
            EventKind::Talon,
            format!(
                "seat {} wins the auction at {auction_bid}; talon is {}",
                contractor.0 + 1,
                if public { "public" } else { "hidden" }
            ),
        );
        Ok(())
    }

    fn apply_proof_response(
        &mut self,
        actor: Seat,
        reveal: bool,
        round: &mut Round,
    ) -> Result<(), RuleError> {
        let (proof, contract_details) = match &mut round.phase {
            RoundPhase::Auction(auction) => (&mut auction.proof, None),
            RoundPhase::Contract(contract) => (
                &mut contract.proof,
                Some((contract.contractor, contract.points)),
            ),
            _ => return Err(RuleError::WrongPhase),
        };
        handle_proof_response(actor, proof.as_mut(), reveal)?;
        self.push(
            EventKind::Proof,
            format!(
                "seat {} {} proof",
                actor.0 + 1,
                if reveal { "requests" } else { "waives" }
            ),
        );
        if proof.as_ref().is_some_and(proof_complete_without_reveal) {
            if let Some((contractor, points)) = contract_details {
                round.phase = RoundPhase::Play(new_play(contractor, points));
            } else {
                *proof = None;
            }
        }
        Ok(())
    }

    fn apply_proof_reveal(
        &mut self,
        actor: Seat,
        suits: &[Suit],
        round: &mut Round,
    ) -> Result<(), RuleError> {
        let proof = match &round.phase {
            RoundPhase::Auction(auction) => auction.proof.as_ref(),
            RoundPhase::Contract(contract) => contract.proof.as_ref(),
            _ => None,
        }
        .ok_or(RuleError::WrongPhase)?;
        if actor != proof.bidder || !proof.reveal_required {
            return Err(RuleError::NotProofBidder);
        }
        validate_proof(&round.hands[actor.index()], proof.points, suits)?;
        for &suit in suits {
            round.publicly_revealed.insert(Card {
                rank: Rank::Queen,
                suit,
            });
            round.publicly_revealed.insert(Card {
                rank: Rank::King,
                suit,
            });
        }
        match &mut round.phase {
            RoundPhase::Auction(auction) => {
                auction.revealed_marriages[actor.index()].extend(suits.iter().copied());
                auction.proof = None;
            }
            RoundPhase::Contract(contract) => {
                contract.revealed_marriages.extend(suits.iter().copied());
                let play = new_play(contract.contractor, contract.points);
                round.phase = RoundPhase::Play(play);
            }
            _ => unreachable!("phase checked before reveal"),
        }
        self.push(
            EventKind::Proof,
            format!("seat {} proves support with {suits:?}", actor.0 + 1),
        );
        Ok(())
    }

    fn continue_after_talon(actor: Seat, round: &mut Round) -> Result<(), RuleError> {
        let RoundPhase::Talon(talon) = &round.phase else {
            return Err(RuleError::WrongPhase);
        };
        if actor != talon.contractor {
            return Err(RuleError::NotYourTurn);
        }
        round.phase = RoundPhase::Transfer(Transfer {
            contractor: talon.contractor,
            auction_bid: talon.auction_bid,
        });
        Ok(())
    }

    fn apply_surrender(&mut self, actor: Seat, round: &mut Round) -> Result<(), RuleError> {
        let RoundPhase::Talon(talon) = &round.phase else {
            return Err(RuleError::WrongPhase);
        };
        if actor != talon.contractor {
            return Err(RuleError::NotYourTurn);
        }
        let contractor = talon.contractor;
        let auction_bid = talon.auction_bid;
        let mut deltas = [60_i32; PLAYER_COUNT];
        if self.players[contractor.index()].free_surrender_used {
            deltas[contractor.index()] = -i32::from(auction_bid);
        } else {
            deltas[contractor.index()] = 0;
            self.players[contractor.index()].free_surrender_used = true;
        }
        suppress_locked_defenders(&mut deltas, round.locked_at_start, contractor);
        round.phase = RoundPhase::RoundFinished(self.apply_scores(
            contractor,
            auction_bid,
            [0; PLAYER_COUNT],
            deltas,
            true,
        ));
        self.push(
            EventKind::Score,
            format!("seat {} surrenders at {auction_bid}", contractor.0 + 1),
        );
        Ok(())
    }

    fn apply_transfer(
        &mut self,
        actor: Seat,
        gifts: Vec<Gift>,
        round: &mut Round,
    ) -> Result<(), RuleError> {
        let RoundPhase::Transfer(transfer) = &round.phase else {
            return Err(RuleError::WrongPhase);
        };
        if actor != transfer.contractor {
            return Err(RuleError::NotYourTurn);
        }
        validate_transfer(actor, &round.hands[actor.index()], &gifts)?;
        for gift in gifts {
            remove_card(&mut round.hands[actor.index()], gift.card)?;
            round.hands[gift.recipient.index()].push(gift.card);
            sort_hand(&mut round.hands[gift.recipient.index()]);
            self.push(
                EventKind::Transfer,
                format!(
                    "seat {} gives {} to seat {}",
                    actor.0 + 1,
                    gift.card,
                    gift.recipient.0 + 1
                ),
            );
        }
        round.phase = RoundPhase::Contract(Contract {
            contractor: actor,
            auction_bid: transfer.auction_bid,
            points: transfer.auction_bid,
            proof: None,
            revealed_marriages: BTreeSet::new(),
        });
        Ok(())
    }

    fn apply_contract(
        &mut self,
        actor: Seat,
        points: u16,
        round: &mut Round,
    ) -> Result<(), RuleError> {
        let RoundPhase::Contract(contract) = &mut round.phase else {
            return Err(RuleError::WrongPhase);
        };
        if contract.proof.is_some() {
            return Err(RuleError::WrongPhase);
        }
        if actor != contract.contractor {
            return Err(RuleError::NotYourTurn);
        }
        if points < contract.auction_bid || !points.is_multiple_of(10) {
            return Err(RuleError::InvalidContract);
        }
        ensure_supported(&round.hands[actor.index()], points)?;
        contract.points = points;
        self.push(
            EventKind::Contract,
            format!("seat {} declares contract {points}", actor.0 + 1),
        );
        if points > revealed_ceiling(&contract.revealed_marriages) {
            contract.proof = Some(new_proof(ProofPurpose::Contract, actor, points));
        } else {
            round.phase = RoundPhase::Play(new_play(actor, points));
        }
        Ok(())
    }

    fn apply_card_play(
        &mut self,
        actor: Seat,
        card: Card,
        round: &mut Round,
    ) -> Result<(), RuleError> {
        let RoundPhase::Play(play) = &mut round.phase else {
            return Err(RuleError::WrongPhase);
        };
        if actor != play.turn {
            return Err(RuleError::NotYourTurn);
        }
        if !round.hands[actor.index()].contains(&card) {
            return Err(RuleError::CardNotHeld);
        }
        if !legal_cards(&round.hands[actor.index()], &play.trick, play.trump).contains(&card) {
            return Err(RuleError::IllegalPlay);
        }
        if play.trick.is_empty()
            && is_marriage_card(card)
            && round.hands[actor.index()].contains(&mate(card))
        {
            play.trump = Some(card.suit);
            play.declared_marriages[actor.index()].insert(card.suit);
            play.marriage_points[actor.index()] += marriage_value(card.suit);
            self.push(
                EventKind::Marriage,
                format!(
                    "seat {} declares {:?} for {} points; trump changes",
                    actor.0 + 1,
                    card.suit,
                    marriage_value(card.suit)
                ),
            );
        }
        remove_card(&mut round.hands[actor.index()], card)?;
        play.trick.push(PlayedCard {
            player: actor,
            card,
        });
        self.push(
            EventKind::Play,
            format!("seat {} plays {card}", actor.0 + 1),
        );
        if play.trick.len() < PLAYER_COUNT {
            play.turn = actor.next();
            return Ok(());
        }
        let winner = trick_winner(&play.trick, play.trump, game_rank_strength)
            .expect("three-card trick has a winner")
            .player;
        let completed = std::mem::take(&mut play.trick);
        play.captured[winner.index()].extend(completed.iter().map(|played| played.card));
        play.last_trick = completed;
        play.leader = winner;
        play.turn = winner;
        self.push(
            EventKind::Trick,
            format!("seat {} takes the trick", winner.0 + 1),
        );
        if round.hands.iter().all(Vec::is_empty) {
            round.phase = RoundPhase::RoundFinished(self.score_play(round.locked_at_start, play));
        }
        Ok(())
    }

    fn apply_claim(&mut self, actor: Seat, round: &mut Round) -> Result<(), RuleError> {
        let RoundPhase::Play(play) = &round.phase else {
            return Err(RuleError::WrongPhase);
        };
        if actor != play.leader || actor != play.turn || !play.trick.is_empty() {
            return Err(RuleError::CannotClaim);
        }
        round.phase = RoundPhase::ClaimVote(ClaimVote {
            play: play.clone(),
            claimant: actor,
            voters: opponents_in_order(actor),
            voter_index: 0,
        });
        self.push(
            EventKind::Claim,
            format!("seat {} claims all remaining tricks", actor.0 + 1),
        );
        Ok(())
    }

    fn apply_claim_vote(
        &mut self,
        actor: Seat,
        accept: bool,
        round: &mut Round,
    ) -> Result<(), RuleError> {
        let RoundPhase::ClaimVote(vote) = &mut round.phase else {
            return Err(RuleError::WrongPhase);
        };
        let expected = vote
            .voters
            .get(vote.voter_index)
            .copied()
            .ok_or(RuleError::WrongPhase)?;
        if actor != expected {
            return Err(RuleError::NotClaimVoter);
        }
        self.push(
            EventKind::Claim,
            format!(
                "seat {} {} the claim",
                actor.0 + 1,
                if accept { "accepts" } else { "rejects" }
            ),
        );
        if !accept {
            vote.play.open_hands[vote.claimant.index()] = true;
            round.phase = RoundPhase::Play(vote.play.clone());
        } else if vote.voter_index + 1 == vote.voters.len() {
            for hand in &mut round.hands {
                vote.play.captured[vote.claimant.index()].append(hand);
            }
            round.phase =
                RoundPhase::RoundFinished(self.score_play(round.locked_at_start, &vote.play));
        } else {
            vote.voter_index += 1;
        }
        Ok(())
    }

    fn score_play(&mut self, locked_at_start: [bool; PLAYER_COUNT], play: &Play) -> RoundResult {
        let raw = std::array::from_fn(|index| {
            play.captured[index]
                .iter()
                .map(|card| card_points(*card))
                .sum::<u16>()
                + play.marriage_points[index]
        });
        let contractor_made = raw[play.contractor.index()] >= play.contract;
        let mut deltas = std::array::from_fn(|index| {
            if index == play.contractor.index() {
                if contractor_made {
                    i32::from(play.contract)
                } else {
                    -i32::from(play.contract)
                }
            } else {
                round_to_ten(i32::from(raw[index]))
            }
        });
        suppress_locked_defenders(&mut deltas, locked_at_start, play.contractor);
        let result = self.apply_scores(play.contractor, play.contract, raw, deltas, false);
        self.push(
            EventKind::Score,
            format!("round scores raw {raw:?}, deltas {deltas:?}"),
        );
        result
    }

    fn apply_scores(
        &mut self,
        contractor: Seat,
        contract: u16,
        raw_points: [u16; PLAYER_COUNT],
        deltas: [i32; PLAYER_COUNT],
        surrendered: bool,
    ) -> RoundResult {
        for (player, delta) in self.players.iter_mut().zip(deltas) {
            player.score += delta;
        }
        RoundResult {
            contractor,
            contract,
            raw_points,
            deltas,
            scores: self.players.each_ref().map(|player| player.score),
            surrendered,
        }
    }

    fn finish_round(&mut self, round: &Round) {
        let RoundPhase::RoundFinished(result) = &round.phase else {
            return;
        };
        let contractor_success = !result.surrendered
            && result.deltas[result.contractor.index()] > 0
            && result.scores[result.contractor.index()] >= self.config.target_score;
        let winner = if contractor_success {
            Some(result.contractor)
        } else {
            let mut eligible = Seat::ALL
                .into_iter()
                .filter(|seat| result.scores[seat.index()] >= self.config.target_score)
                .collect::<Vec<_>>();
            eligible.sort_by_key(|seat| std::cmp::Reverse(result.scores[seat.index()]));
            match eligible.as_slice() {
                [first, second, ..]
                    if result.scores[first.index()] == result.scores[second.index()] =>
                {
                    None
                }
                [first, ..] => Some(*first),
                [] => None,
            }
        };
        if let Some(winner) = winner {
            self.phase = MatchPhase::MatchFinished { winner };
        }
    }

    /// Advances after a displayed round result. The dealer always rotates.
    ///
    /// # Errors
    ///
    /// Returns [`RuleError::WrongPhase`] before the round has finished.
    pub fn acknowledge_round(&mut self) -> Result<(), RuleError> {
        let round_is_finished = matches!(
            &self.phase,
            MatchPhase::Round(round) if matches!(round.phase, RoundPhase::RoundFinished(_))
        );
        if !round_is_finished {
            return Err(RuleError::WrongPhase);
        }
        self.dealer = self.dealer.next();
        self.round_index += 1;
        self.phase = MatchPhase::WaitingForDeal;
        Ok(())
    }

    fn push(&mut self, kind: EventKind, message: String) {
        self.events.push(GameEvent { kind, message });
    }
}

/// Returns concrete actions the authoritative engine will currently accept from
/// a seat. Transfer choices are enumerated so humans and future bots receive the
/// same information.
#[must_use]
pub fn legal_actions(game: &Match, actor: Seat) -> Vec<Action> {
    if !actor.valid() {
        return Vec::new();
    }
    let MatchPhase::Round(round) = &game.phase else {
        return Vec::new();
    };
    match &round.phase {
        RoundPhase::Auction(auction) => {
            if let Some(proof) = &auction.proof {
                return legal_proof_actions(proof, &round.hands[actor.index()], actor);
            }
            if auction.turn != actor || !auction.active[actor.index()] {
                return Vec::new();
            }
            let mut actions = vec![Action::Pass];
            let points = auction.highest_bid + 10;
            if points <= support_ceiling(&round.hands[actor.index()]) {
                actions.insert(0, Action::Bid { points });
            }
            actions
        }
        RoundPhase::Talon(talon) if talon.contractor == actor => {
            vec![Action::ContinueAfterTalon, Action::Surrender]
        }
        RoundPhase::Transfer(transfer) if transfer.contractor == actor => {
            let opponents = opponents_in_order(actor);
            let hand = &round.hands[actor.index()];
            let mut actions = Vec::new();
            for (first_index, &first) in hand.iter().enumerate() {
                for (second_index, &second) in hand.iter().enumerate() {
                    if first_index != second_index {
                        actions.push(Action::Transfer {
                            gifts: vec![
                                Gift {
                                    recipient: opponents[0],
                                    card: first,
                                },
                                Gift {
                                    recipient: opponents[1],
                                    card: second,
                                },
                            ],
                        });
                    }
                }
            }
            actions
        }
        RoundPhase::Contract(contract) => {
            if let Some(proof) = &contract.proof {
                return legal_proof_actions(proof, &round.hands[actor.index()], actor);
            }
            if contract.contractor != actor {
                return Vec::new();
            }
            (contract.auction_bid..=support_ceiling(&round.hands[actor.index()]))
                .step_by(10)
                .map(|points| Action::ConfirmContract { points })
                .collect()
        }
        RoundPhase::Play(play) if play.turn == actor => {
            let mut actions = legal_cards(&round.hands[actor.index()], &play.trick, play.trump)
                .into_iter()
                .map(|card| Action::PlayCard { card })
                .collect::<Vec<_>>();
            if play.trick.is_empty() && play.leader == actor {
                actions.push(Action::ClaimRemaining);
            }
            actions
        }
        RoundPhase::ClaimVote(vote)
            if vote.voters.get(vote.voter_index).copied() == Some(actor) =>
        {
            vec![
                Action::VoteOnClaim { accept: false },
                Action::VoteOnClaim { accept: true },
            ]
        }
        _ => Vec::new(),
    }
}

fn legal_proof_actions(proof: &Proof, hand: &[Card], actor: Seat) -> Vec<Action> {
    if proof.reveal_required && proof.bidder == actor {
        smallest_proofs(hand, proof.points)
            .into_iter()
            .map(|suits| Action::RevealProof { suits })
            .collect()
    } else if proof.responder() == Some(actor) {
        vec![
            Action::RespondToProof { reveal: false },
            Action::RespondToProof { reveal: true },
        ]
    } else {
        Vec::new()
    }
}

#[must_use]
pub fn game_deck() -> Vec<Card> {
    Suit::ALL
        .into_iter()
        .flat_map(|suit| {
            [
                Rank::Nine,
                Rank::Jack,
                Rank::Queen,
                Rank::King,
                Rank::Ten,
                Rank::Ace,
            ]
            .map(move |rank| Card { rank, suit })
        })
        .collect()
}

/// Validates the exact 24 unique cards used by this game.
///
/// # Errors
///
/// Returns [`RuleError::InvalidDeck`] for a missing, duplicate, or foreign card.
pub fn validate_game_deck(deck: &[Card]) -> Result<(), RuleError> {
    let expected = game_deck().into_iter().collect::<BTreeSet<_>>();
    let actual = deck.iter().copied().collect::<BTreeSet<_>>();
    if deck.len() == 24 && actual == expected {
        Ok(())
    } else {
        Err(RuleError::InvalidDeck)
    }
}

#[must_use]
pub const fn card_points(card: Card) -> u16 {
    match card.rank {
        Rank::Nine
        | Rank::Two
        | Rank::Three
        | Rank::Four
        | Rank::Five
        | Rank::Six
        | Rank::Seven
        | Rank::Eight => 0,
        Rank::Jack => 2,
        Rank::Queen => 3,
        Rank::King => 4,
        Rank::Ten => 10,
        Rank::Ace => 11,
    }
}

#[must_use]
pub const fn marriage_value(suit: Suit) -> u16 {
    match suit {
        Suit::Spades => 40,
        Suit::Clubs => 60,
        Suit::Diamonds => 80,
        Suit::Hearts => 100,
    }
}

#[must_use]
pub fn marriages(hand: &[Card]) -> BTreeSet<Suit> {
    Suit::ALL
        .into_iter()
        .filter(|&suit| {
            hand.contains(&Card {
                rank: Rank::Queen,
                suit,
            }) && hand.contains(&Card {
                rank: Rank::King,
                suit,
            })
        })
        .collect()
}

#[must_use]
pub fn support_ceiling(hand: &[Card]) -> u16 {
    120 + marriages(hand).into_iter().map(marriage_value).sum::<u16>()
}

#[must_use]
pub const fn round_to_ten(points: i32) -> i32 {
    if points >= 0 {
        ((points + 5) / 10) * 10
    } else {
        ((points - 5) / 10) * 10
    }
}

#[must_use]
pub fn legal_cards(hand: &[Card], trick: &[PlayedCard<Seat>], trump: Option<Suit>) -> Vec<Card> {
    let Some(lead) = trick.first().map(|played| played.card.suit) else {
        return hand.to_vec();
    };
    let led_cards = hand
        .iter()
        .copied()
        .filter(|card| card.suit == lead)
        .collect::<Vec<_>>();
    if led_cards.is_empty() {
        return hand.to_vec();
    }
    let Some(winner) = trick_winner(trick, trump, game_rank_strength).map(|play| play.card) else {
        return hand.to_vec();
    };
    if winner.suit != lead {
        return led_cards;
    }
    let higher = led_cards
        .iter()
        .copied()
        .filter(|card| game_rank_strength(card.rank) > game_rank_strength(winner.rank))
        .collect::<Vec<_>>();
    if higher.is_empty() { led_cards } else { higher }
}

#[must_use]
pub const fn game_rank_strength(rank: Rank) -> u8 {
    match rank {
        Rank::Nine
        | Rank::Two
        | Rank::Three
        | Rank::Four
        | Rank::Five
        | Rank::Six
        | Rank::Seven
        | Rank::Eight => 0,
        Rank::Jack => 1,
        Rank::Queen => 2,
        Rank::King => 3,
        Rank::Ten => 4,
        Rank::Ace => 5,
    }
}

fn ensure_supported(hand: &[Card], points: u16) -> Result<(), RuleError> {
    if points <= support_ceiling(hand) && points >= 100 && points.is_multiple_of(10) {
        Ok(())
    } else {
        Err(RuleError::UnsupportedPoints)
    }
}

fn revealed_ceiling(suits: &BTreeSet<Suit>) -> u16 {
    120 + suits.iter().copied().map(marriage_value).sum::<u16>()
}

fn new_proof(purpose: ProofPurpose, bidder: Seat, points: u16) -> Proof {
    Proof {
        purpose,
        bidder,
        points,
        responders: opponents_in_order(bidder),
        responder_index: 0,
        reveal_required: false,
    }
}

fn opponents_in_order(seat: Seat) -> [Seat; 2] {
    [seat.next(), seat.next().next()]
}

fn handle_proof_response(
    actor: Seat,
    proof: Option<&mut Proof>,
    reveal: bool,
) -> Result<(), RuleError> {
    let proof = proof.ok_or(RuleError::WrongPhase)?;
    if proof.reveal_required || proof.responder() != Some(actor) {
        return Err(RuleError::NotProofResponder);
    }
    if reveal {
        proof.reveal_required = true;
    } else {
        proof.responder_index += 1;
    }
    Ok(())
}

fn proof_complete_without_reveal(proof: &Proof) -> bool {
    !proof.reveal_required && proof.responder_index == proof.responders.len()
}

fn validate_proof(hand: &[Card], points: u16, suits: &[Suit]) -> Result<(), RuleError> {
    let held = marriages(hand);
    let unique = suits.iter().copied().collect::<BTreeSet<_>>();
    if unique.len() != suits.len() || !unique.is_subset(&held) {
        return Err(RuleError::InvalidProof);
    }
    let sufficient = 120 + unique.iter().copied().map(marriage_value).sum::<u16>() >= points;
    let held = held.into_iter().collect::<Vec<_>>();
    let smaller_exists = (0_u8..(1 << held.len()))
        .filter(|mask| {
            usize::try_from(mask.count_ones()).expect("four-bit count fits usize") < unique.len()
        })
        .any(|mask| {
            let value = held
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1 << index) != 0)
                .map(|(_, suit)| marriage_value(*suit))
                .sum::<u16>();
            120 + value >= points
        });
    if sufficient && !smaller_exists {
        Ok(())
    } else {
        Err(RuleError::InvalidProof)
    }
}

fn smallest_proofs(hand: &[Card], points: u16) -> Vec<Vec<Suit>> {
    let held = marriages(hand).into_iter().collect::<Vec<_>>();
    let mut valid = (0_u8..(1 << held.len()))
        .filter_map(|mask| {
            let suits = held
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1 << index) != 0)
                .map(|(_, suit)| *suit)
                .collect::<Vec<_>>();
            (120 + suits.iter().copied().map(marriage_value).sum::<u16>() >= points)
                .then_some(suits)
        })
        .collect::<Vec<_>>();
    let minimum = valid.iter().map(Vec::len).min().unwrap_or(usize::MAX);
    valid.retain(|suits| suits.len() == minimum);
    valid
}

fn validate_transfer(actor: Seat, hand: &[Card], gifts: &[Gift]) -> Result<(), RuleError> {
    if gifts.len() != 2 {
        return Err(RuleError::InvalidTransfer);
    }
    let recipients = gifts
        .iter()
        .map(|gift| gift.recipient)
        .collect::<BTreeSet<_>>();
    let cards = gifts.iter().map(|gift| gift.card).collect::<BTreeSet<_>>();
    let expected = Seat::ALL
        .into_iter()
        .filter(|seat| *seat != actor)
        .collect::<BTreeSet<_>>();
    if recipients == expected
        && cards.len() == 2
        && gifts.iter().all(|gift| hand.contains(&gift.card))
    {
        Ok(())
    } else {
        Err(RuleError::InvalidTransfer)
    }
}

fn new_play(contractor: Seat, contract: u16) -> Play {
    Play {
        contractor,
        contract,
        leader: contractor,
        turn: contractor,
        trump: None,
        trick: Vec::new(),
        last_trick: Vec::new(),
        captured: std::array::from_fn(|_| Vec::new()),
        marriage_points: [0; PLAYER_COUNT],
        declared_marriages: std::array::from_fn(|_| BTreeSet::new()),
        open_hands: [false; PLAYER_COUNT],
    }
}

fn suppress_locked_defenders(
    deltas: &mut [i32; PLAYER_COUNT],
    locked: [bool; PLAYER_COUNT],
    contractor: Seat,
) {
    for seat in Seat::ALL {
        if seat != contractor && locked[seat.index()] && deltas[seat.index()] > 0 {
            deltas[seat.index()] = 0;
        }
    }
}

fn next_active(after: Seat, active: [bool; PLAYER_COUNT]) -> Option<Seat> {
    let mut seat = after;
    for _ in 0..PLAYER_COUNT {
        seat = seat.next();
        if active[seat.index()] {
            return Some(seat);
        }
    }
    None
}

fn is_marriage_card(card: Card) -> bool {
    matches!(card.rank, Rank::Queen | Rank::King)
}

fn mate(card: Card) -> Card {
    Card {
        rank: if card.rank == Rank::Queen {
            Rank::King
        } else {
            Rank::Queen
        },
        suit: card.suit,
    }
}

fn remove_card(hand: &mut Vec<Card>, card: Card) -> Result<(), RuleError> {
    let index = hand
        .iter()
        .position(|held| *held == card)
        .ok_or(RuleError::CardNotHeld)?;
    hand.remove(index);
    Ok(())
}

fn deal_shuffle_seed(seed: u64, attempt: u64) -> [u8; 32] {
    if attempt == 0 {
        let mut expanded_seed = [0_u8; 32];
        expanded_seed[..8].copy_from_slice(&seed.to_le_bytes());
        return expanded_seed;
    }

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mille/deal-retry-seed/v1");
    hasher.update(&seed.to_le_bytes());
    hasher.update(&attempt.to_le_bytes());
    *hasher.finalize().as_bytes()
}

fn distribute_deal(deck: &[Card]) -> ([Vec<Card>; PLAYER_COUNT], Vec<Card>) {
    let mut hands: [Vec<Card>; PLAYER_COUNT] = std::array::from_fn(|_| Vec::new());
    let mut talon = Vec::new();
    let mut cursor = 0;

    // Three circuits: seat 1, seat 2, one talon card, seat 3.
    for _ in 0..3 {
        hands[0].push(deck[cursor]);
        cursor += 1;
        hands[1].push(deck[cursor]);
        cursor += 1;
        talon.push(deck[cursor]);
        cursor += 1;
        hands[2].push(deck[cursor]);
        cursor += 1;
    }
    // Four ordinary circuits.
    for _ in 0..4 {
        for hand in &mut hands {
            hand.push(deck[cursor]);
            cursor += 1;
        }
    }

    (hands, talon)
}

fn has_all_nines(hand: &[Card]) -> bool {
    Suit::ALL.into_iter().all(|suit| {
        hand.contains(&Card {
            rank: Rank::Nine,
            suit,
        })
    })
}

fn sort_hand(hand: &mut [Card]) {
    hand.sort_by_key(|card| (suit_order(card.suit), game_rank_strength(card.rank)));
}

const fn suit_order(suit: Suit) -> u8 {
    match suit {
        Suit::Spades => 0,
        Suit::Clubs => 1,
        Suit::Diamonds => 2,
        Suit::Hearts => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(rank: Rank, suit: Suit) -> Card {
        Card { rank, suit }
    }

    fn names() -> [String; 3] {
        ["Ada", "Bert", "Celina"].map(String::from)
    }

    fn dealt() -> Match {
        let mut game = Match::new(names(), Config::default(), Seat::ONE);
        game.deal_ordered(7, game_deck()).unwrap();
        game
    }

    fn active_round(game: &Match) -> &Round {
        let MatchPhase::Round(round) = &game.phase else {
            panic!("test expected an active round")
        };
        round
    }

    fn shuffled_order(seed: u64, attempt: u64) -> Vec<Card> {
        let mut deck = cards::Deck::from_cards(game_deck()).unwrap();
        deck.shuffle_with_seed(deal_shuffle_seed(seed, attempt));
        deck.cards().to_vec()
    }

    fn order_has_forbidden_hand(order: &[Card]) -> bool {
        distribute_deal(order)
            .0
            .iter()
            .any(|hand| has_all_nines(hand))
    }

    #[test]
    fn scoring_table_is_exact_and_each_suit_has_thirty() {
        assert_eq!(game_rank_strength(Rank::Nine), 0);
        assert!(game_rank_strength(Rank::Ace) > game_rank_strength(Rank::Ten));
        for suit in Suit::ALL {
            assert_eq!(
                game_deck()
                    .into_iter()
                    .filter(|card| card.suit == suit)
                    .map(card_points)
                    .sum::<u16>(),
                30
            );
        }
        assert_eq!(game_deck().into_iter().map(card_points).sum::<u16>(), 120);
    }

    #[test]
    fn marriage_values_and_support_are_exact() {
        let hand = vec![
            card(Rank::Queen, Suit::Spades),
            card(Rank::King, Suit::Spades),
            card(Rank::Queen, Suit::Hearts),
            card(Rank::King, Suit::Hearts),
        ];
        assert_eq!(marriage_value(Suit::Spades), 40);
        assert_eq!(marriage_value(Suit::Clubs), 60);
        assert_eq!(marriage_value(Suit::Diamonds), 80);
        assert_eq!(marriage_value(Suit::Hearts), 100);
        assert_eq!(support_ceiling(&hand), 260);
    }

    #[test]
    fn interleaved_deal_places_three_specific_talon_cards() {
        let game = dealt();
        let MatchPhase::Round(round) = game.phase else {
            panic!()
        };
        assert_eq!(
            round.talon,
            vec![game_deck()[2], game_deck()[6], game_deck()[10]]
        );
        assert_eq!(
            round.hands.iter().map(Vec::len).collect::<Vec<_>>(),
            [7, 7, 7]
        );
        let RoundPhase::Auction(auction) = round.phase else {
            panic!()
        };
        assert_eq!(auction.opener, Seat::TWO);
        assert_eq!(auction.highest_bid, 100);
        assert_eq!(auction.turn, Seat::THREE);
    }

    #[test]
    fn auction_is_exact_steps_and_pass_is_permanent() {
        let mut game = dealt();
        assert_eq!(
            game.apply(Seat::TWO, Action::Bid { points: 110 }),
            Err(RuleError::NotYourTurn)
        );
        assert_eq!(
            game.apply(Seat::THREE, Action::Bid { points: 120 }),
            Err(RuleError::InvalidBidStep)
        );
        game.apply(Seat::THREE, Action::Bid { points: 110 })
            .unwrap();
        game.apply(Seat::ONE, Action::Pass).unwrap();
        game.apply(Seat::TWO, Action::Pass).unwrap();
        let RoundPhase::Talon(talon) = &active_round(&game).phase else {
            panic!()
        };
        assert_eq!(talon.contractor, Seat::THREE);
        assert_eq!(talon.auction_bid, 110);
    }

    #[test]
    fn bids_above_support_are_rejected() {
        let mut game = dealt();
        assert_eq!(
            game.apply(Seat::THREE, Action::Bid { points: 110 }),
            Ok(ApplyOutcome {
                events: vec![GameEvent {
                    kind: EventKind::Auction,
                    message: "seat 3 bids 110".into()
                }]
            })
        );
        game.apply(Seat::ONE, Action::Bid { points: 120 }).unwrap();
        assert_eq!(
            game.apply(Seat::TWO, Action::Bid { points: 130 }),
            Err(RuleError::UnsupportedPoints)
        );
    }

    #[test]
    fn proof_is_sequential_and_smallest_sufficient() {
        let hand = vec![
            card(Rank::Queen, Suit::Clubs),
            card(Rank::King, Suit::Clubs),
            card(Rank::Queen, Suit::Hearts),
            card(Rank::King, Suit::Hearts),
        ];
        assert_eq!(validate_proof(&hand, 180, &[Suit::Clubs]), Ok(()));
        assert_eq!(validate_proof(&hand, 180, &[Suit::Hearts]), Ok(()));
        assert_eq!(
            validate_proof(&hand, 180, &[Suit::Clubs, Suit::Hearts]),
            Err(RuleError::InvalidProof)
        );
        assert_eq!(
            validate_proof(&hand, 230, &[Suit::Hearts]),
            Err(RuleError::InvalidProof)
        );
        assert_eq!(
            validate_proof(&hand, 230, &[Suit::Clubs, Suit::Hearts]),
            Ok(())
        );

        let only_small_marriages = vec![
            card(Rank::Queen, Suit::Clubs),
            card(Rank::King, Suit::Clubs),
            card(Rank::Queen, Suit::Spades),
            card(Rank::King, Suit::Spades),
        ];
        assert_eq!(
            validate_proof(&only_small_marriages, 190, &[Suit::Clubs, Suit::Spades]),
            Ok(())
        );
    }

    #[test]
    fn legal_play_follows_and_beats_only_when_led_suit_is_winning() {
        let hand = vec![
            card(Rank::Nine, Suit::Clubs),
            card(Rank::Ace, Suit::Clubs),
            card(Rank::Ace, Suit::Hearts),
        ];
        let trick = vec![PlayedCard {
            player: Seat::ONE,
            card: card(Rank::King, Suit::Clubs),
        }];
        assert_eq!(
            legal_cards(&hand, &trick, None),
            vec![card(Rank::Ace, Suit::Clubs)]
        );

        let trumped = vec![
            PlayedCard {
                player: Seat::ONE,
                card: card(Rank::King, Suit::Clubs),
            },
            PlayedCard {
                player: Seat::TWO,
                card: card(Rank::Nine, Suit::Hearts),
            },
        ];
        assert_eq!(
            legal_cards(&hand, &trumped, Some(Suit::Hearts)),
            vec![card(Rank::Nine, Suit::Clubs), card(Rank::Ace, Suit::Clubs)]
        );
    }

    #[test]
    fn void_player_may_discard_without_trumping() {
        let hand = vec![
            card(Rank::Nine, Suit::Spades),
            card(Rank::Ace, Suit::Hearts),
        ];
        let trick = vec![PlayedCard {
            player: Seat::ONE,
            card: card(Rank::Ace, Suit::Clubs),
        }];
        assert_eq!(legal_cards(&hand, &trick, Some(Suit::Hearts)), hand);
    }

    #[test]
    fn rounding_uses_five_up() {
        assert_eq!(round_to_ten(44), 40);
        assert_eq!(round_to_ten(45), 50);
        assert_eq!(round_to_ten(46), 50);
    }

    #[test]
    fn first_surrender_is_free_then_costs_bid_and_dealer_rotates() {
        let mut game = dealt();
        game.apply(Seat::THREE, Action::Pass).unwrap();
        game.apply(Seat::ONE, Action::Pass).unwrap();
        game.apply(Seat::TWO, Action::Surrender).unwrap();
        let RoundPhase::RoundFinished(result) = &active_round(&game).phase else {
            panic!()
        };
        assert_eq!(result.deltas, [60, 0, 60]);
        game.acknowledge_round().unwrap();
        assert_eq!(game.dealer, Seat::TWO);

        game.deal_ordered(8, game_deck()).unwrap();
        game.apply(Seat::ONE, Action::Pass).unwrap();
        game.apply(Seat::TWO, Action::Pass).unwrap();
        game.apply(Seat::THREE, Action::Surrender).unwrap();
        let RoundPhase::RoundFinished(result) = &active_round(&game).phase else {
            panic!()
        };
        assert_eq!(result.deltas, [60, 60, 0]);
    }

    #[test]
    fn claim_rejection_opens_claimant_hand_and_resumes_same_lead() {
        let mut game = dealt();
        let MatchPhase::Round(round) = &mut game.phase else {
            panic!()
        };
        round.hands = std::array::from_fn(|_| vec![card(Rank::Nine, Suit::Clubs)]);
        round.phase = RoundPhase::Play(new_play(Seat::TWO, 100));
        game.apply(Seat::TWO, Action::ClaimRemaining).unwrap();
        game.apply(Seat::THREE, Action::VoteOnClaim { accept: false })
            .unwrap();
        let RoundPhase::Play(play) = &active_round(&game).phase else {
            panic!()
        };
        assert!(play.open_hands[Seat::TWO.index()]);
        assert_eq!(play.turn, Seat::TWO);
    }

    #[test]
    fn accepted_claim_awards_every_unplayed_card_without_new_marriages() {
        let mut game = dealt();
        let MatchPhase::Round(round) = &mut game.phase else {
            panic!()
        };
        round.hands = [
            vec![card(Rank::Queen, Suit::Hearts)],
            vec![card(Rank::King, Suit::Hearts)],
            vec![card(Rank::Ace, Suit::Clubs)],
        ];
        round.phase = RoundPhase::Play(new_play(Seat::ONE, 100));
        game.apply(Seat::ONE, Action::ClaimRemaining).unwrap();
        game.apply(Seat::TWO, Action::VoteOnClaim { accept: true })
            .unwrap();
        game.apply(Seat::THREE, Action::VoteOnClaim { accept: true })
            .unwrap();
        let RoundPhase::RoundFinished(result) = &active_round(&game).phase else {
            panic!()
        };
        assert_eq!(result.raw_points[0], 18);
        assert_eq!(result.deltas[0], -100);
    }

    #[test]
    fn locked_defender_cannot_gain_but_can_lose_as_contractor() {
        let mut game = Match::new(names(), Config::default(), Seat::ONE);
        game.players[0].score = 900;
        game.deal_ordered(1, game_deck()).unwrap();
        let MatchPhase::Round(round) = &mut game.phase else {
            panic!()
        };
        assert!(round.locked_at_start[0]);
        let mut deltas = [60, -100, 60];
        suppress_locked_defenders(&mut deltas, round.locked_at_start, Seat::TWO);
        assert_eq!(deltas, [0, -100, 60]);
        let mut contractor_deltas = [-100, 60, 60];
        suppress_locked_defenders(&mut contractor_deltas, round.locked_at_start, Seat::ONE);
        assert_eq!(contractor_deltas[0], -100);
    }

    #[test]
    fn seed_derivation_separates_matches_and_rounds() {
        let a = Match::derive_round_seed(42, 0, 0);
        assert_eq!(a, Match::derive_round_seed(42, 0, 0));
        assert_ne!(a, Match::derive_round_seed(42, 0, 1));
        assert_ne!(a, Match::derive_round_seed(42, 1, 0));
    }

    #[test]
    fn seeded_deal_retries_a_first_shuffle_with_all_four_nines() {
        let rejected = shuffled_order(62, 0);
        let (rejected_hands, _) = distribute_deal(&rejected);
        assert!(has_all_nines(&rejected_hands[Seat::ONE.index()]));

        let mut game = Match::new(names(), Config::default(), Seat::ONE);
        let SeededDeal {
            order: accepted,
            four_nines_reshuffles,
        } = game.deal_seeded_with_report(62).unwrap();
        assert_eq!(four_nines_reshuffles, 1);
        assert_eq!(
            accepted,
            vec![
                card(Rank::Nine, Suit::Clubs),
                card(Rank::Ace, Suit::Diamonds),
                card(Rank::Queen, Suit::Hearts),
                card(Rank::Ten, Suit::Spades),
                card(Rank::Ten, Suit::Hearts),
                card(Rank::Ace, Suit::Clubs),
                card(Rank::King, Suit::Spades),
                card(Rank::Jack, Suit::Diamonds),
                card(Rank::Jack, Suit::Clubs),
                card(Rank::Nine, Suit::Spades),
                card(Rank::Ace, Suit::Spades),
                card(Rank::Ace, Suit::Hearts),
                card(Rank::Jack, Suit::Hearts),
                card(Rank::Jack, Suit::Spades),
                card(Rank::King, Suit::Diamonds),
                card(Rank::Nine, Suit::Hearts),
                card(Rank::King, Suit::Clubs),
                card(Rank::Queen, Suit::Clubs),
                card(Rank::Queen, Suit::Spades),
                card(Rank::Nine, Suit::Diamonds),
                card(Rank::Queen, Suit::Diamonds),
                card(Rank::Ten, Suit::Clubs),
                card(Rank::Ten, Suit::Diamonds),
                card(Rank::King, Suit::Hearts),
            ]
        );
        assert_ne!(accepted, rejected);
        assert_eq!(
            game.events,
            [
                GameEvent {
                    kind: EventKind::Deal,
                    message: "1 invalid all-four-nines deal was reshuffled".into(),
                },
                GameEvent {
                    kind: EventKind::Deal,
                    message: "round dealt; dealer is seat 1".into(),
                },
                GameEvent {
                    kind: EventKind::Auction,
                    message: "seat 2 opens at 100".into(),
                },
            ]
        );

        let round = active_round(&game);
        assert_eq!(round.seed, 62);
        assert!(!round.hands.iter().any(|hand| has_all_nines(hand)));

        let accepted_cards = accepted.iter().copied().collect::<BTreeSet<_>>();
        let round_cards = round
            .hands
            .iter()
            .flatten()
            .chain(&round.talon)
            .copied()
            .collect::<BTreeSet<_>>();
        assert_eq!(accepted.len(), 24);
        assert_eq!(accepted_cards, game_deck().into_iter().collect());
        assert_eq!(round_cards, accepted_cards);
    }

    #[test]
    fn seeded_deal_can_retry_more_than_once() {
        assert!(order_has_forbidden_hand(&shuffled_order(9_773, 0)));
        assert!(order_has_forbidden_hand(&shuffled_order(9_773, 1)));
        assert!(!order_has_forbidden_hand(&shuffled_order(9_773, 2)));

        let mut game = Match::new(names(), Config::default(), Seat::ONE);
        let report = game.deal_seeded_with_report(9_773).unwrap();
        assert_eq!(report.four_nines_reshuffles, 2);
        assert_eq!(report.order, shuffled_order(9_773, 2));
        assert_eq!(
            game.events.first(),
            Some(&GameEvent {
                kind: EventKind::Deal,
                message: "2 invalid all-four-nines deals were reshuffled".into(),
            })
        );
        assert_eq!(game.events.len(), 3);
    }

    #[test]
    fn seeded_redeal_is_reproducible_from_the_original_round_seed() {
        let mut first = Match::new(names(), Config::default(), Seat::ONE);
        let mut second = Match::new(names(), Config::default(), Seat::ONE);
        let first_order = first.deal_seeded(9_773).unwrap();
        let second_order = second.deal_seeded(9_773).unwrap();

        assert_eq!(first_order, second_order);
        assert_eq!(first, second);
        assert_eq!(active_round(&first).seed, 9_773);
    }

    #[test]
    fn accepted_seeded_deals_never_give_one_player_all_nines() {
        for seed in 0..1_000 {
            let mut game = Match::new(names(), Config::default(), Seat::ONE);
            let accepted = game.deal_seeded(seed).unwrap();
            assert!(!order_has_forbidden_hand(&accepted), "seed {seed}");
            assert!(
                !active_round(&game)
                    .hands
                    .iter()
                    .any(|hand| has_all_nines(hand)),
                "seed {seed}"
            );
        }
    }

    #[test]
    fn valid_seed_keeps_the_legacy_golden_order() {
        let mut game = Match::new(names(), Config::default(), Seat::ONE);
        let labels = game
            .deal_seeded(0)
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            [
                "K♦", "K♠", "9♥", "J♣", "Q♠", "A♥", "9♠", "K♥", "10♣", "A♠", "J♦", "J♥", "9♦",
                "10♥", "Q♦", "A♦", "10♠", "10♦", "J♠", "A♣", "Q♥", "9♣", "Q♣", "K♣",
            ]
        );
    }

    #[test]
    fn talon_can_be_hidden_only_for_a_winning_bid_of_one_hundred() {
        let mut game = Match::new(
            names(),
            Config {
                talon_visibility: TalonVisibility::HideAtOneHundred,
                ..Config::default()
            },
            Seat::ONE,
        );
        game.deal_ordered(1, game_deck()).unwrap();
        game.apply(Seat::THREE, Action::Pass).unwrap();
        game.apply(Seat::ONE, Action::Pass).unwrap();
        let active_round = active_round(&game);
        let RoundPhase::Talon(talon) = &active_round.phase else {
            panic!()
        };
        assert!(!talon.public);
        assert!(active_round.publicly_revealed.is_empty());
    }

    #[test]
    fn passed_opponent_still_gets_sequential_proof_choice() {
        let mut game = dealt();
        let MatchPhase::Round(round) = &mut game.phase else {
            panic!()
        };
        round.hands[0] = vec![
            card(Rank::Queen, Suit::Spades),
            card(Rank::King, Suit::Spades),
        ];
        round.phase = RoundPhase::Auction(Auction {
            opener: Seat::TWO,
            turn: Seat::ONE,
            highest_bidder: Seat::THREE,
            highest_bid: 120,
            active: [true, false, true],
            proof: None,
            revealed_marriages: std::array::from_fn(|_| BTreeSet::new()),
        });
        game.apply(Seat::ONE, Action::Bid { points: 130 }).unwrap();
        let RoundPhase::Auction(auction) = &active_round(&game).phase else {
            panic!()
        };
        assert_eq!(auction.proof.as_ref().unwrap().responder(), Some(Seat::TWO));
        game.apply(Seat::TWO, Action::RespondToProof { reveal: false })
            .unwrap();
        let RoundPhase::Auction(auction) = &active_round(&game).phase else {
            panic!()
        };
        assert_eq!(
            auction.proof.as_ref().unwrap().responder(),
            Some(Seat::THREE)
        );
    }

    #[test]
    fn valid_transfer_leaves_every_player_with_eight_and_invalid_gifts_are_atomic() {
        let mut game = dealt();
        let MatchPhase::Round(round) = &mut game.phase else {
            panic!()
        };
        round.hands[0] = game_deck()[..10].to_vec();
        round.hands[1] = game_deck()[10..17].to_vec();
        round.hands[2] = game_deck()[17..24].to_vec();
        round.phase = RoundPhase::Transfer(Transfer {
            contractor: Seat::ONE,
            auction_bid: 100,
        });
        let first = round.hands[0][0];
        let second = round.hands[0][1];
        assert_eq!(
            game.apply(
                Seat::ONE,
                Action::Transfer {
                    gifts: vec![
                        Gift {
                            recipient: Seat::TWO,
                            card: first
                        },
                        Gift {
                            recipient: Seat::TWO,
                            card: second
                        },
                    ],
                },
            ),
            Err(RuleError::InvalidTransfer)
        );
        let MatchPhase::Round(round) = &game.phase else {
            panic!()
        };
        assert_eq!(round.hands[0].len(), 10);
        game.apply(
            Seat::ONE,
            Action::Transfer {
                gifts: vec![
                    Gift {
                        recipient: Seat::TWO,
                        card: first,
                    },
                    Gift {
                        recipient: Seat::THREE,
                        card: second,
                    },
                ],
            },
        )
        .unwrap();
        let MatchPhase::Round(round) = &game.phase else {
            panic!()
        };
        assert_eq!(round.hands.each_ref().map(Vec::len), [8, 8, 8]);
    }

    #[test]
    fn final_contract_uses_the_final_eight_card_marriages() {
        let mut game = dealt();
        let MatchPhase::Round(round) = &mut game.phase else {
            panic!()
        };
        round.hands[0] = vec![
            card(Rank::Queen, Suit::Hearts),
            card(Rank::King, Suit::Hearts),
            card(Rank::Nine, Suit::Clubs),
        ];
        round.phase = RoundPhase::Contract(Contract {
            contractor: Seat::ONE,
            auction_bid: 100,
            points: 100,
            proof: None,
            revealed_marriages: BTreeSet::new(),
        });
        assert_eq!(
            game.apply(Seat::ONE, Action::ConfirmContract { points: 230 }),
            Err(RuleError::UnsupportedPoints)
        );
        game.apply(Seat::ONE, Action::ConfirmContract { points: 220 })
            .unwrap();
        let RoundPhase::Contract(contract) = &active_round(&game).phase else {
            panic!()
        };
        assert_eq!(contract.points, 220);
        assert_eq!(
            contract.proof.as_ref().unwrap().responders,
            [Seat::TWO, Seat::THREE]
        );
    }

    #[test]
    fn leading_a_marriage_declares_immediately_even_when_the_trick_is_lost() {
        let mut game = dealt();
        let MatchPhase::Round(round) = &mut game.phase else {
            panic!()
        };
        round.hands = [
            vec![
                card(Rank::Queen, Suit::Hearts),
                card(Rank::King, Suit::Hearts),
            ],
            vec![card(Rank::Ace, Suit::Hearts), card(Rank::Nine, Suit::Clubs)],
            vec![
                card(Rank::Nine, Suit::Hearts),
                card(Rank::Jack, Suit::Clubs),
            ],
        ];
        round.phase = RoundPhase::Play(new_play(Seat::ONE, 100));
        game.apply(
            Seat::ONE,
            Action::PlayCard {
                card: card(Rank::Queen, Suit::Hearts),
            },
        )
        .unwrap();
        game.apply(
            Seat::TWO,
            Action::PlayCard {
                card: card(Rank::Ace, Suit::Hearts),
            },
        )
        .unwrap();
        game.apply(
            Seat::THREE,
            Action::PlayCard {
                card: card(Rank::Nine, Suit::Hearts),
            },
        )
        .unwrap();
        let RoundPhase::Play(play) = &active_round(&game).phase else {
            panic!()
        };
        assert_eq!(play.leader, Seat::TWO);
        assert_eq!(play.trump, Some(Suit::Hearts));
        assert_eq!(play.marriage_points[0], 100);
    }

    #[test]
    fn later_marriage_replaces_trump_and_cannot_be_declared_off_lead() {
        let mut game = dealt();
        let MatchPhase::Round(round) = &mut game.phase else {
            panic!()
        };
        round.hands = [
            vec![card(Rank::Nine, Suit::Hearts)],
            vec![
                card(Rank::Queen, Suit::Clubs),
                card(Rank::King, Suit::Clubs),
            ],
            vec![card(Rank::Nine, Suit::Spades)],
        ];
        let mut play = new_play(Seat::ONE, 100);
        play.trump = Some(Suit::Hearts);
        play.leader = Seat::TWO;
        play.turn = Seat::TWO;
        round.phase = RoundPhase::Play(play);
        game.apply(
            Seat::TWO,
            Action::PlayCard {
                card: card(Rank::King, Suit::Clubs),
            },
        )
        .unwrap();
        let RoundPhase::Play(play) = &active_round(&game).phase else {
            panic!()
        };
        assert_eq!(play.trump, Some(Suit::Clubs));
        assert_eq!(play.marriage_points[1], 60);
    }

    #[test]
    fn contractor_score_is_exact_contract_or_negative_contract_and_never_raw_total() {
        let mut game = dealt();
        let mut made = new_play(Seat::ONE, 130);
        made.captured[0] = game_deck();
        made.marriage_points[0] = 100;
        let result = game.score_play([false; 3], &made);
        assert_eq!(result.raw_points[0], 220);
        assert_eq!(result.deltas[0], 130);

        let failed = new_play(Seat::TWO, 170);
        let result = game.score_play([false; 3], &failed);
        assert_eq!(result.deltas[1], -170);
    }

    #[test]
    fn successful_contractor_has_winner_priority_and_target_ties_continue() {
        let mut game = dealt();
        let result = RoundResult {
            contractor: Seat::ONE,
            contract: 100,
            raw_points: [100, 0, 0],
            deltas: [100, 60, 0],
            scores: [1_000, 1_200, 20],
            surrendered: false,
        };
        let round = Round {
            seed: 0,
            dealer: Seat::ONE,
            locked_at_start: [false; 3],
            hands: std::array::from_fn(|_| Vec::new()),
            talon: Vec::new(),
            publicly_revealed: BTreeSet::new(),
            phase: RoundPhase::RoundFinished(result),
        };
        game.finish_round(&round);
        assert_eq!(game.phase, MatchPhase::MatchFinished { winner: Seat::ONE });

        let mut tied = dealt();
        let tied_result = RoundResult {
            contractor: Seat::THREE,
            contract: 100,
            raw_points: [60, 60, 0],
            deltas: [60, 60, -100],
            scores: [1_000, 1_000, 0],
            surrendered: false,
        };
        let tied_round = Round {
            phase: RoundPhase::RoundFinished(tied_result),
            ..round
        };
        tied.finish_round(&tied_round);
        assert!(!matches!(tied.phase, MatchPhase::MatchFinished { .. }));
    }

    #[test]
    fn rejected_claim_can_be_made_again() {
        let mut game = dealt();
        let MatchPhase::Round(round) = &mut game.phase else {
            panic!()
        };
        round.hands = std::array::from_fn(|_| vec![card(Rank::Nine, Suit::Clubs)]);
        round.phase = RoundPhase::Play(new_play(Seat::ONE, 100));
        game.apply(Seat::ONE, Action::ClaimRemaining).unwrap();
        game.apply(Seat::TWO, Action::VoteOnClaim { accept: false })
            .unwrap();
        game.apply(Seat::ONE, Action::ClaimRemaining).unwrap();
        let RoundPhase::ClaimVote(vote) = &active_round(&game).phase else {
            panic!()
        };
        assert_eq!(vote.claimant, Seat::ONE);
        assert_eq!(vote.voters[0], Seat::TWO);
    }

    #[test]
    fn surrender_is_available_only_before_transfer_and_repeat_costs_the_bid() {
        let mut game = dealt();
        game.players[1].free_surrender_used = true;
        game.apply(Seat::THREE, Action::Pass).unwrap();
        game.apply(Seat::ONE, Action::Pass).unwrap();
        game.apply(Seat::TWO, Action::Surrender).unwrap();
        let RoundPhase::RoundFinished(result) = &active_round(&game).phase else {
            panic!()
        };
        assert_eq!(result.deltas, [60, -100, 60]);

        let mut after = dealt();
        after.apply(Seat::THREE, Action::Pass).unwrap();
        after.apply(Seat::ONE, Action::Pass).unwrap();
        after.apply(Seat::TWO, Action::ContinueAfterTalon).unwrap();
        assert_eq!(
            after.apply(Seat::TWO, Action::Surrender),
            Err(RuleError::WrongPhase)
        );
    }
}
