"""Best five-card poker ranks from five to seven standard-deck cards.

Ranks compare as ordinary tuples: category first, then descending tiebreaks.
Suits never break ties. Cards use uppercase ranks and lowercase suits, e.g. As.
"""

from collections import Counter

_RANKS = "23456789TJQKA"
_SUITS = "cdhs"
_NAMES = (
    "High card", "One pair", "Two pair", "Three of a kind", "Straight",
    "Flush", "Full house", "Four of a kind", "Straight flush",
)


def _straight_high(ranks):
    values = sorted(set(ranks), reverse=True)
    if 14 in values:
        values.append(1)
    run = 1
    for previous, current in zip(values, values[1:]):
        run = run + 1 if previous - current == 1 else 1
        if run == 5:
            return current + 4
    return None


def rank_hand(cards):
    """Return (category, *tiebreak_ranks), with ranks 2–14 and categories 0–8.

    Evaluate directly without enumerating five-card subsets. Reject malformed,
    duplicate, or out-of-range hands with ValueError.
    """
    cards = list(cards)
    if not 5 <= len(cards) <= 7:
        raise ValueError("A hand must contain five to seven cards")
    if any(not isinstance(card, str) or len(card) != 2
           or card[0] not in _RANKS or card[1] not in _SUITS for card in cards):
        raise ValueError("Cards must use rank+suit, e.g. As, Td, 2c")
    if len(set(cards)) != len(cards):
        raise ValueError("A hand cannot contain duplicate cards")

    ranks = [_RANKS.index(card[0]) + 2 for card in cards]
    counts = Counter(ranks)
    ordered = sorted(counts, reverse=True)
    suited = {suit: [] for suit in _SUITS}
    for card, rank in zip(cards, ranks):
        suited[card[1]].append(rank)
    flush = next((sorted(values, reverse=True) for values in suited.values()
                  if len(values) >= 5), None)

    if flush:
        high = _straight_high(flush)
        if high is not None:
            return (8, high)

    quads = [rank for rank in ordered if counts[rank] == 4]
    if quads:
        quad = quads[0]
        return (7, quad, next(rank for rank in ordered if rank != quad))

    trips = [rank for rank in ordered if counts[rank] >= 3]
    if trips:
        trip = trips[0]
        pairs = [rank for rank in ordered if rank != trip and counts[rank] >= 2]
        if pairs:
            return (6, trip, pairs[0])

    if flush:
        return (5, *flush[:5])
    high = _straight_high(ordered)
    if high is not None:
        return (4, high)
    if trips:
        trip = trips[0]
        kickers = [rank for rank in ordered if rank != trip][:2]
        return (3, trip, *kickers)

    pairs = [rank for rank in ordered if counts[rank] == 2]
    if len(pairs) >= 2:
        top_pairs = pairs[:2]
        kicker = next(rank for rank in ordered if rank not in top_pairs)
        return (2, *top_pairs, kicker)
    if pairs:
        pair = pairs[0]
        kickers = [rank for rank in ordered if rank != pair][:3]
        return (1, pair, *kickers)
    return (0, *ordered[:5])


def hand_name(rank):
    """Return the readable category of a rank_hand result."""
    return _NAMES[rank[0]]
