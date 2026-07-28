# Rule-test traceability

The authoritative tests live beside the crate they exercise. Run all of them
with `cargo test --workspace`.

| Rule or boundary | Test coverage |
|---|---|
| 24-card composition, rank order, values, 120 total | `scoring_table_is_exact_and_each_suit_has_thirty`, card-deck tests |
| Stable seeded shuffle and independent match/round seeds | `seeded_shuffle_is_reproducible_and_conservative`, `seed_has_a_golden_prefix`, `seed_derivation_separates_matches_and_rounds` |
| Interleaved 7+7+7+3 deal and forced 100 opener | `interleaved_deal_places_three_specific_talon_cards` |
| Exact +10 bidding, turn order, permanent pass, hand ceiling | `auction_is_exact_steps_and_pass_is_permanent`, `bids_above_support_are_rejected` |
| Sequential proof, passed-player challenge, smallest held subset | `proof_is_sequential_and_smallest_sufficient`, `passed_opponent_still_gets_sequential_proof_choice` |
| Talon visibility variant | `talon_can_be_hidden_only_for_a_winning_bid_of_one_hundred` |
| Two private gifts and final eight-card contract ceiling | `valid_transfer_leaves_every_player_with_eight_and_invalid_gifts_are_atomic`, `final_contract_uses_the_final_eight_card_marriages` |
| Per-player free surrender, repeat penalty, phase cutoff, dealer rotation | `first_surrender_is_free_then_costs_bid_and_dealer_rotates`, `surrender_is_available_only_before_transfer_and_repeat_costs_the_bid` |
| Follow suit, beat led winner if possible, no forced trump | `legal_play_follows_and_beats_only_when_led_suit_is_winning`, `void_player_may_discard_without_trumping` |
| Automatic marriage, immediate points, losing declaration, trump replacement | `leading_a_marriage_declares_immediately_even_when_the_trick_is_lost`, `later_marriage_replaces_trump_and_cannot_be_declared_off_lead` |
| Early claim accept/reject, open cards, repeat claim, no undeclared bonus | three `claim_*` / `accepted_claim_*` tests |
| Raw scoring, rounding 5 up, contractor cap/failure | `rounding_uses_five_up`, `contractor_score_is_exact_contract_or_negative_contract_and_never_raw_total` |
| 900 lock, negative unlock path, target priority and ties | `locked_defender_cannot_gain_but_can_lose_as_contractor`, `successful_contractor_has_winner_priority_and_target_ties_continue` |
| Observer/player/referee secrecy | all `mille-protocol` tests and `observer_projection_omits_base_seed_and_hidden_hands` |
| SQLite restore, credentials/tokens, revisions/idempotency, seed parsing | all `game-server` tests |
| Loopback-only room deletion, registry/SQLite removal, proxy forwarding checks | deletion tests in both server apps |
| HTML4 observer card backs and escaping | both `web-server` tests |

The live smoke test used both running processes to create a seeded room, join
Ada/Bert/Celina in order, verify the third join auto-started it, fetch all three
role projections, load every
required page route, submit a legal action at the expected revision, restart the
game server, and verify that revision, turn, history, passwords, and tokens were
restored from SQLite.
