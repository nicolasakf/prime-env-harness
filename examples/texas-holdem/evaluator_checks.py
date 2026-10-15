"""Evaluator-specific assertions; call evaluator_checks() explicitly to run."""

from itertools import combinations
from random import Random

from poker_eval import hand_name, rank_hand


def evaluator_checks():
    """Check category selection, wheel handling, ties, and all kicker forms."""
    assert rank_hand("As 2d 3h 4c 5s Kd Qc".split()) == (4, 5)
    assert rank_hand("As 2d 3h 4c 5s 6d 7c".split()) == (4, 7)
    assert rank_hand("As 2s 3s 4s 5s Kd Qc".split()) == (8, 5)
    assert rank_hand("9s Ts Js Qs Ks Ad Ac".split()) == (8, 13)
    assert rank_hand("As Ks Qs Js Ts 2d 3c".split()) == (8, 14)
    assert rank_hand("9s 9d 9h 9c As Kd Qc".split()) == (7, 9, 14)
    assert rank_hand("As Ad Ah Ks Kd Kh 2c".split()) == (6, 14, 13)
    assert rank_hand("Ks Kd Kh As Ad 2h 3c".split()) == (6, 13, 14)
    assert rank_hand("As Js 9s 7s 5s 3s 2s".split()) == (5, 14, 11, 9, 7, 5)
    assert rank_hand("As Kd Qh Jc 9s 3d 2c".split()) == (0, 14, 13, 12, 11, 9)
    assert rank_hand("As Ad Kh Qc Js 3d 2c".split()) == (1, 14, 13, 12, 11)
    assert rank_hand("As Ad Kh Kc Qs Qd 2c".split()) == (2, 14, 13, 12)
    assert rank_hand("As Ad Ah Kc Qs 3d 2c".split()) == (3, 14, 13, 12)

    # Suit changes and unused cards do not break ties.
    assert rank_hand("As Kd Qh Jc Ts 3d 2c".split()) == rank_hand(
        "Ah Kc Qd Js Th 9d 8c".split())
    assert rank_hand("As Ad Kh Qc Js 3d 2c".split()) == rank_hand(
        "Ah Ac Kd Qs Jh 9d 8c".split())

    # Compare the final kicker for each non-straight category that has kickers.
    stronger_weaker = (
        ("As Kd Qh Jc 9s", "Ah Kc Qd Js 8h"),
        ("As Ad Kh Qc Js", "Ah Ac Kd Qs Th"),
        ("As Ad Kh Kc Qs", "Ah Ac Kd Ks Jh"),
        ("As Ad Ah Kc Qs", "As Ad Ah Kc Js"),
        ("As Js 9s 7s 5s", "Ah Jh 9h 7h 4h"),
        ("9s 9d 9h 9c As", "9s 9d 9h 9c Ks"),
    )
    for stronger, weaker in stronger_weaker:
        assert rank_hand(stronger.split()) > rank_hand(weaker.split())
    assert rank_hand("Ks Kd Kh Qc Qs".split()) > rank_hand(
        "Qs Qd Qh Ac As".split())
    assert rank_hand("Ks Kd Kh Ac As".split()) > rank_hand(
        "Ks Kd Kh Qc Qs".split())
    assert hand_name(rank_hand("As Ks Qs Js Ts".split())) == "Straight flush"

    deck = [rank + suit for rank in "23456789TJQKA" for suit in "cdhs"]
    random = Random(20261007)
    for _ in range(200):
        cards = random.sample(deck, 7)
        expected = max(_reference_five(subset) for subset in combinations(cards, 5))
        assert rank_hand(cards) == expected, (cards, expected, rank_hand(cards))


def _reference_five(cards):
    """Small independent five-card oracle, used only by checks."""
    values = sorted(("--23456789TJQKA".index(card[0]) for card in cards), reverse=True)
    groups = sorted(((values.count(value), value) for value in set(values)), reverse=True)
    pattern = [count for count, _ in groups]
    flush = len({card[1] for card in cards}) == 1
    straight = 0
    if len(groups) == 5:
        if values[0] - values[-1] == 4:
            straight = values[0]
        elif values == [14, 5, 4, 3, 2]:
            straight = 5
    if flush and straight:
        return (8, straight)
    if pattern == [4, 1]:
        return (7, groups[0][1], groups[1][1])
    if pattern == [3, 2]:
        return (6, groups[0][1], groups[1][1])
    if flush:
        return (5, *values)
    if straight:
        return (4, straight)
    if pattern == [3, 1, 1]:
        return (3, *(value for _, value in groups))
    if pattern == [2, 2, 1]:
        return (2, *(value for _, value in groups))
    if pattern == [2, 1, 1, 1]:
        return (1, *(value for _, value in groups))
    return (0, *values)
