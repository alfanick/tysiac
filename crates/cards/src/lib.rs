//! Reusable primitives for standard playing-card games.
//!
//! Game-specific rank strength, scoring, following rules, and trump policy stay
//! in the consuming game crate.

use std::fmt;

use rand_chacha::ChaCha12Rng;
use rand_core::{RngCore, SeedableRng};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The four French suits. Declaration order is never inferred from this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Suit {
    Clubs,
    Diamonds,
    Hearts,
    Spades,
}

impl Suit {
    pub const ALL: [Self; 4] = [Self::Clubs, Self::Diamonds, Self::Hearts, Self::Spades];

    #[must_use]
    pub const fn symbol(self) -> char {
        match self {
            Self::Clubs => '♣',
            Self::Diamonds => '♦',
            Self::Hearts => '♥',
            Self::Spades => '♠',
        }
    }

    const fn index(self) -> u8 {
        match self {
            Self::Clubs => 0,
            Self::Diamonds => 1,
            Self::Hearts => 2,
            Self::Spades => 3,
        }
    }
}

/// Standard ranks. Games must provide their own strength table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Rank {
    Two,
    Three,
    Four,
    Five,
    Six,
    Seven,
    Eight,
    Nine,
    Ten,
    Jack,
    Queen,
    King,
    Ace,
}

impl Rank {
    pub const ALL: [Self; 13] = [
        Self::Two,
        Self::Three,
        Self::Four,
        Self::Five,
        Self::Six,
        Self::Seven,
        Self::Eight,
        Self::Nine,
        Self::Ten,
        Self::Jack,
        Self::Queen,
        Self::King,
        Self::Ace,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Two => "2",
            Self::Three => "3",
            Self::Four => "4",
            Self::Five => "5",
            Self::Six => "6",
            Self::Seven => "7",
            Self::Eight => "8",
            Self::Nine => "9",
            Self::Ten => "10",
            Self::Jack => "J",
            Self::Queen => "Q",
            Self::King => "K",
            Self::Ace => "A",
        }
    }

    const fn index(self) -> u8 {
        match self {
            Self::Two => 0,
            Self::Three => 1,
            Self::Four => 2,
            Self::Five => 3,
            Self::Six => 4,
            Self::Seven => 5,
            Self::Eight => 6,
            Self::Nine => 7,
            Self::Ten => 8,
            Self::Jack => 9,
            Self::Queen => 10,
            Self::King => 11,
            Self::Ace => 12,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct Card {
    pub rank: Rank,
    pub suit: Suit,
}

impl Card {
    #[must_use]
    pub const fn new(rank: Rank, suit: Suit) -> Self {
        Self { rank, suit }
    }

    #[must_use]
    pub const fn bit_index(self) -> u8 {
        self.suit.index() * 13 + self.rank.index()
    }
}

impl fmt::Display for Card {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}{}", self.rank.label(), self.suit.symbol())
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CardSetError {
    #[error("card already exists in the set")]
    Duplicate,
    #[error("card is not present in the set")]
    Missing,
}

/// A compact set for one standard 52-card deck.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CardSet(u64);

impl CardSet {
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub const fn contains(self, card: Card) -> bool {
        self.0 & (1_u64 << card.bit_index()) != 0
    }

    /// Adds one card.
    ///
    /// # Errors
    ///
    /// Returns [`CardSetError::Duplicate`] when the card is already present.
    pub fn insert(&mut self, card: Card) -> Result<(), CardSetError> {
        if self.contains(card) {
            return Err(CardSetError::Duplicate);
        }
        self.0 |= 1_u64 << card.bit_index();
        Ok(())
    }

    /// Removes one card.
    ///
    /// # Errors
    ///
    /// Returns [`CardSetError::Missing`] when the card is not present.
    pub fn remove(&mut self, card: Card) -> Result<(), CardSetError> {
        if !self.contains(card) {
            return Err(CardSetError::Missing);
        }
        self.0 &= !(1_u64 << card.bit_index());
        Ok(())
    }

    pub fn iter(self) -> impl Iterator<Item = Card> {
        Suit::ALL.into_iter().flat_map(move |suit| {
            Rank::ALL
                .into_iter()
                .map(move |rank| Card::new(rank, suit))
                .filter(move |card| self.contains(*card))
        })
    }

    /// Builds a set while checking uniqueness.
    ///
    /// # Errors
    ///
    /// Returns [`CardSetError::Duplicate`] when the input repeats a card.
    pub fn try_from_cards(iter: impl IntoIterator<Item = Card>) -> Result<Self, CardSetError> {
        let mut set = Self::empty();
        for card in iter {
            set.insert(card)?;
        }
        Ok(set)
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum DeckError {
    #[error("deck contains duplicate cards")]
    Duplicate,
    #[error("deck is empty")]
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deck {
    cards: Vec<Card>,
}

impl Deck {
    /// Builds a non-empty deck containing no duplicate cards.
    ///
    /// # Errors
    ///
    /// Returns [`DeckError::Empty`] for no cards and [`DeckError::Duplicate`]
    /// when a card occurs more than once.
    pub fn from_cards(cards: impl IntoIterator<Item = Card>) -> Result<Self, DeckError> {
        let cards: Vec<_> = cards.into_iter().collect();
        if cards.is_empty() {
            return Err(DeckError::Empty);
        }
        CardSet::try_from_cards(cards.iter().copied()).map_err(|_| DeckError::Duplicate)?;
        Ok(Self { cards })
    }

    #[must_use]
    pub fn standard() -> Self {
        Self {
            cards: Suit::ALL
                .into_iter()
                .flat_map(|suit| Rank::ALL.into_iter().map(move |rank| Card::new(rank, suit)))
                .collect(),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.cards.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    #[must_use]
    pub fn cards(&self) -> &[Card] {
        &self.cards
    }

    pub fn draw(&mut self) -> Option<Card> {
        self.cards.pop()
    }

    /// Stable v1 Fisher–Yates shuffle.
    ///
    /// Both the generator and unbiased index selection are fixed so golden
    /// seeded games remain reproducible across dependency updates.
    pub fn shuffle_with_seed(&mut self, seed: [u8; 32]) {
        let mut rng = ChaCha12Rng::from_seed(seed);
        for upper in (1..self.cards.len()).rev() {
            let selected = uniform_below(&mut rng, upper + 1);
            self.cards.swap(upper, selected);
        }
    }
}

fn uniform_below(rng: &mut impl RngCore, upper: usize) -> usize {
    debug_assert!(upper > 0);
    let upper = u64::try_from(upper).expect("card deck length fits u64");
    let zone = u64::MAX - (u64::MAX % upper);
    loop {
        let value = rng.next_u64();
        if value < zone {
            return usize::try_from(value % upper).expect("selected deck index fits usize");
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayedCard<P> {
    pub player: P,
    pub card: Card,
}

/// Return the winning play for a complete or partial trick.
///
/// Off-suit non-trumps never win. Rank strength is supplied by the game.
#[must_use]
pub fn trick_winner<P: Copy>(
    plays: &[PlayedCard<P>],
    trump: Option<Suit>,
    strength: impl Fn(Rank) -> u8,
) -> Option<PlayedCard<P>> {
    let led = plays.first()?.card.suit;
    plays.iter().copied().max_by_key(|play| {
        let category = if Some(play.card.suit) == trump {
            2
        } else {
            u8::from(play.card.suit == led)
        };
        (category, strength(play.card.rank))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(rank: Rank, suit: Suit) -> Card {
        Card::new(rank, suit)
    }

    #[test]
    fn standard_deck_contains_every_card_once() {
        let deck = Deck::standard();
        assert_eq!(deck.len(), 52);
        let set = CardSet::try_from_cards(deck.cards().iter().copied()).unwrap();
        assert_eq!(set.len(), 52);
    }

    #[test]
    fn set_rejects_duplicates_and_missing_removals() {
        let ace = card(Rank::Ace, Suit::Hearts);
        let mut set = CardSet::empty();
        assert_eq!(set.insert(ace), Ok(()));
        assert_eq!(set.insert(ace), Err(CardSetError::Duplicate));
        assert_eq!(set.remove(ace), Ok(()));
        assert_eq!(set.remove(ace), Err(CardSetError::Missing));
    }

    #[test]
    fn seeded_shuffle_is_reproducible_and_conservative() {
        let seed = [42; 32];
        let mut first = Deck::standard();
        let mut second = Deck::standard();
        first.shuffle_with_seed(seed);
        second.shuffle_with_seed(seed);
        assert_eq!(first, second);
        assert_eq!(
            CardSet::try_from_cards(first.cards().iter().copied())
                .unwrap()
                .len(),
            52
        );
    }

    #[test]
    fn seed_has_a_golden_prefix() {
        let mut deck = Deck::standard();
        deck.shuffle_with_seed([7; 32]);
        let labels: Vec<_> = deck.cards()[..5].iter().map(ToString::to_string).collect();
        assert_eq!(labels, ["4♣", "2♠", "7♣", "A♠", "8♦"]);
    }

    #[test]
    fn trump_beats_led_suit_and_off_suit_loses() {
        let plays = [
            PlayedCard {
                player: 1,
                card: card(Rank::Ace, Suit::Spades),
            },
            PlayedCard {
                player: 2,
                card: card(Rank::Jack, Suit::Hearts),
            },
            PlayedCard {
                player: 3,
                card: card(Rank::King, Suit::Clubs),
            },
        ];
        let winner = trick_winner(&plays, Some(Suit::Hearts), Rank::index).unwrap();
        assert_eq!(winner.player, 2);
        let winner = trick_winner(&plays, None, Rank::index).unwrap();
        assert_eq!(winner.player, 1);
    }
}
