"""Behavioral checks; execute only after the user selects an execution policy."""
import json
from copy import deepcopy

import sdk
from environment import TradingCards


def request(game, method, *arguments):
    sdk._begin_request(method)
    result = getattr(game, method)(*arguments)
    return result, deepcopy(sdk._response_metadata())


def snapshot(game):
    state, metadata = request(game, "snapshot")
    assert metadata == {}
    return deepcopy(state)


def observe(game, player):
    before = snapshot(game)
    result, metadata = request(game, "observe", player)
    assert metadata == {} and snapshot(game) == before
    return result


def view(game, player):
    before = snapshot(game)
    result, metadata = request(game, "agent_view", player)
    assert metadata == {} and snapshot(game) == before
    # The model resends this every turn; it must stay small and JSON-serializable.
    assert len(json.dumps(result)) <= 32 * 1024
    return result


def attention(metadata, revision, next_player):
    terminal = next_player is None
    assert set(metadata) == ({"notices", "progress", "finished"} if terminal
                             else {"notices", "progress"})
    assert metadata["progress"] == {
        "revision": revision, "waiting_for": [] if terminal else [next_player],
    }
    notices = metadata["notices"]
    public = [notice for notice in notices if notice["to"] == "*"]
    private = [notice for notice in notices if notice["to"] != "*"]
    assert public and all(notice["wake"] is False for notice in public)
    assert all(set(notice) == ({"to", "message", "wake", "actionable"} if notice["to"] != "*"
                               else {"to", "message", "wake"})
               and isinstance(notice["message"], str) for notice in notices)
    if terminal:
        assert private == []
        assert set(metadata["finished"]) == {"summary"}
        assert isinstance(metadata["finished"]["summary"], str)
        assert metadata["finished"]["summary"]
    else:
        assert len(private) == 1
        assert private[0]["to"] == [next_player] and private[0]["wake"] is True
        # The turn prompt is an actionable task, so the host can retire it once obsolete.
        assert private[0]["actionable"] is True


def fresh(players, **settings):
    game = TradingCards()
    result, metadata = request(game, "initialize", {
        "seats": [{"id": player, "role": "player"} for player in players],
        "config": settings,
    })
    assert result == {}
    state = snapshot(game)
    assert state["players"] == list(players) and state["revision"] == 0
    attention(metadata, 0, players[0])
    first = observe(game, players[0])
    second = observe(game, players[1])
    assert first["phase"] == "trading" and first["active_player"] == players[0]
    assert first["available_actions"] == ["open_market"]
    assert second["available_actions"] == []
    assert first["you"] == players[0] and second["you"] == players[1]
    assert first["player_order"] == second["player_order"] == list(players)
    descriptors, metadata = request(game, "describe_actions")
    assert metadata == {} and snapshot(game) == state
    assert {item["name"] for item in descriptors} == {
        "next_hand", "open_market", "improve_market", "buy", "sell",
    }
    assert len(descriptors) == 5
    for item in descriptors:
        assert item["roles"] == ["player"] and "participants" not in item
        assert isinstance(item["description"], str) and item["description"]
        assert item["input_schema"]["type"] == "object"
        assert item["input_schema"]["additionalProperties"] is False
        assert item["output_schema"]["type"] == "object"
    return game


def rejected(game, method, *arguments):
    before = snapshot(game)
    try:
        request(game, method, *arguments)
    except ValueError:
        assert sdk._response_metadata() == {}
        assert snapshot(game) == before
    else:
        raise AssertionError(f"accepted illegal {method}: {arguments!r}")


def move(game, player, action, arguments, next_player):
    previous = snapshot(game)["revision"]
    result, metadata = request(game, "invoke", player, action, arguments)
    assert snapshot(game)["revision"] == previous + 1
    attention(metadata, previous + 1, next_player)
    assert set(result) == {"event"} and isinstance(result["event"], str)
    observation = observe(game, player)
    assert observation["active_player"] == next_player
    # The seat reads its updated state from its view, so the result stays small.
    assert view(game, player)["active_player"] == next_player
    return observation


def roundtrip(game, players):
    state = snapshot(game)
    observations = {player: observe(game, player) for player in players}
    views = {player: view(game, player) for player in players}
    restored = TradingCards()
    result, metadata = request(restored, "restore", state)
    assert result == {} and metadata == {} and snapshot(restored) == state
    assert {player: observe(restored, player) for player in players} == observations
    assert {player: view(restored, player) for player in players} == views
    state["revision"] += 1000
    assert snapshot(restored) == snapshot(game)
    return restored


def card_values(value):
    if isinstance(value, dict):
        return [leaf for child in value.values() for leaf in card_values(child)]
    if isinstance(value, list):
        return [leaf for child in value for leaf in card_values(child)]
    return [value]


def known_deal(game, players):
    state = snapshot(game)
    first, second = players
    state["hands"] = {first: ["AS", "2H"], second: ["QD", "KC"]}
    state["community"] = ["3C", "10S"]
    result, metadata = request(game, "restore", state)
    assert (result, metadata) == ({}, {})
    assert snapshot(game) == state
    return state


def privacy(game, players):
    state = snapshot(game)
    first, second = players
    for player, opponent in ((first, second), (second, first)):
        observation = observe(game, player)
        assert observation["your_private_cards"] == state["hands"][player]
        assert observation["opponent_private_card_count"] == len(state["hands"][opponent])
        assert observation["community_cards"] == state["revealed"]
        assert "all_private_cards" not in observation and "settlement" not in observation
        leaves = card_values(observation)
        hidden = state["hands"][opponent] + state["community"][len(state["revealed"]):]
        assert all(card not in leaves for card in hidden)
        agent = view(game, player)
        assert agent["your_private_cards"] == state["hands"][player]
        assert agent["community_cards"] == state["revealed"]
        assert "all_private_cards" not in agent and "settlement" not in agent
        assert "config" not in agent and "run_rules" not in agent
        assert all(card not in card_values(agent) for card in hidden)


def settlement(game, players):
    observation = observe(game, players[0])
    state = snapshot(game)
    assert observation["market"] is None
    assert observation["community_cards"] == state["community"]
    assert observation["all_private_cards"] == state["hands"]
    ranks = {rank: index for index, rank in enumerate(
        ("A", "2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K"), 1)}
    value = sum(ranks[card[:-1]] for cards in state["hands"].values() for card in cards)
    value += sum(ranks[card[:-1]] for card in state["community"])
    pnl = {player: state["cash"][player] + state["positions"][player] * value
           for player in players}
    assert observation["settlement"] == {
        "value": value, "pnl": pnl,
        "winners": [player for player in players if pnl[player] == max(pnl.values())],
    }
    assert sum(pnl.values()) == 0
    return observation


# Occupants are deliberately absent: the second seat can be human or agent.
for players in (("alice", "bob"), ("dealer-7", "reader_2"), ("seat-z", "human-second")):
    first, second = players
    game = fresh(players, private_cards_per_player=2, community_card_count=2,
                 trades_per_round=2)
    dealt = snapshot(game)
    assert len(set(dealt["hands"][first] + dealt["hands"][second] + dealt["community"])) == 6
    known_deal(game, players)
    privacy(game, players)
    game = roundtrip(game, players)
    for seat in players:
        before = snapshot(game)
        result, metadata = request(game, "handle_input", seat, "ready")
        # Chat is public, wakes nobody and is not game progress.
        assert result == {"observation": {"posted": before["event_seq"] + 1}} and metadata == {
            "notices": [{"to": "*", "message": f"[hand 1 round 1] {seat}: ready", "wake": False}],
        }
        after = snapshot(game)
        assert after["revision"] == before["revision"] and after["event_seq"] == before["event_seq"] + 1
        assert after["events"][-1] == {"seq": after["event_seq"], "hand": 1, "round": 1,
                                       "actor": seat, "kind": "chat", "text": "ready"}
        assert {key: value for key, value in after.items() if key not in ("events", "event_seq")} == {
            key: value for key, value in before.items() if key not in ("events", "event_seq")}
        for reader in players:
            assert view(game, reader)["recent_events"][-1] == after["events"][-1]
    chatted = snapshot(game)["events"]
    assert [event["actor"] for event in chatted] == list(players)
    rejected(game, "observe", "unknown")
    rejected(game, "handle_input", "unknown", "ready")
    rejected(game, "invoke", "unknown", "open_market", {"bid": 75, "ask": 85})
    rejected(game, "invoke", first, "wrong-action", {})
    rejected(game, "invoke", first, "start_game", {})
    rejected(game, "invoke", second, "open_market", {"bid": 75, "ask": 85})
    for quote in ({"bid": 85, "ask": 85}, {"bid": -1, "ask": 85},
                  {"bid": 86, "ask": 85}, {"bid": True, "ask": 85},
                  {"bid": 75.0, "ask": 85}, {"bid": 75},
                  {"bid": 75, "ask": 85, "seat": second}):
        rejected(game, "invoke", first, "open_market", quote)
    rejected(game, "invoke", first, "open_market", [])
    move(game, first, "open_market", {"bid": 75, "ask": 85}, second)
    rejected(game, "invoke", first, "buy", {})
    for quote in ({"bid": 74, "ask": 85}, {"bid": 75, "ask": 86},
                  {"bid": 75, "ask": 85}):
        rejected(game, "invoke", second, "improve_market", quote)
    move(game, second, "improve_market", {"bid": 77, "ask": 85}, first)
    assert observe(game, first)["market"] == {
        "bid": 77, "ask": 85, "bid_owner": second, "ask_owner": second,
    }
    assert observe(game, first)["available_actions"] == ["buy", "sell", "improve_market"]
    rejected(game, "invoke", first, "sell", {"extra": 1})
    move(game, first, "sell", {}, second)
    assert observe(game, first)["trades"][-1] == {
        "round": 1, "buyer": second, "seller": first, "price": 77,
    }
    assert observe(game, second)["available_actions"] == ["open_market"]
    assert observe(game, second)["market"] is None
    privacy(game, players)
    game = roundtrip(game, players)
    move(game, second, "open_market", {"bid": 80, "ask": 81}, first)
    assert observe(game, first)["available_actions"] == ["buy", "sell"]
    rejected(game, "invoke", first, "improve_market", {"bid": 80, "ask": 81})
    rejected(game, "invoke", first, "buy", {"extra": 1})
    move(game, first, "buy", {}, second)
    revealed = observe(game, first)
    assert revealed["community_cards"] == ["3C"]
    assert revealed["positions"] == {first: 0, second: 0}
    assert revealed["cash"] == {first: -4, second: 4}
    assert revealed["round"] == 2 and revealed["trades_this_round"] == 0
    assert observe(game, second)["available_actions"] == ["open_market"]
    privacy(game, players)
    game = roundtrip(game, players)
    move(game, second, "open_market", {"bid": 0, "ask": 1}, first)
    move(game, first, "buy", {}, second)
    move(game, second, "open_market", {"bid": 8, "ask": 9}, first)
    move(game, first, "sell", {}, None)
    finished = settlement(game, players)
    assert finished["phase"] == "finished" and finished["active_player"] is None
    assert finished["available_actions"] == []
    assert observe(game, second)["available_actions"] == []
    assert finished["settlement"]["value"] == 41
    assert finished["total_pnl"] == finished["settlement"]["pnl"]
    assert finished["winners"] == finished["settlement"]["winners"]
    game = roundtrip(game, players)
    rejected(game, "invoke", first, "open_market", {"bid": 1, "ask": 2})
    rejected(game, "invoke", first, "next_hand", {})

    replacement = fresh(players, private_cards_per_player=1, community_card_count=1,
                        trades_per_round=1)
    move(replacement, first, "open_market", {"bid": 40, "ask": 60}, second)
    move(replacement, second, "improve_market", {"bid": 40, "ask": 55}, first)
    assert observe(replacement, first)["market"] == {
        "bid": 40, "ask": 55, "bid_owner": second, "ask_owner": second,
    }
    move(replacement, first, "improve_market", {"bid": 45, "ask": 55}, second)
    assert observe(replacement, second)["market"] == {
        "bid": 45, "ask": 55, "bid_owner": first, "ask_owner": first,
    }
    assert observe(replacement, second)["available_actions"] == ["buy", "sell", "improve_market"]
    move(replacement, second, "buy", {}, None)
    assert observe(replacement, second)["trades"] == [
        {"round": 1, "buyer": second, "seller": first, "price": 55}]
    settlement(replacement, players)

    multi = fresh(players, private_cards_per_player=1, community_card_count=1,
                  trades_per_round=1, settlement_rounds=2)
    move(multi, first, "open_market", {"bid": 5, "ask": 6}, second)
    move(multi, second, "buy", {}, first)
    first_hand = settlement(multi, players)
    assert first_hand["phase"] == "settled" and first_hand["settlement_round"] == 1
    assert first_hand["available_actions"] == ["next_hand"]
    assert observe(multi, second)["available_actions"] == []
    assert first_hand["total_pnl"] == first_hand["settlement"]["pnl"]
    multi = roundtrip(multi, players)
    rejected(multi, "invoke", second, "next_hand", {})
    rejected(multi, "invoke", first, "next_hand", {"bid": 1})
    rejected(multi, "invoke", first, "open_market", {"bid": 1, "ask": 2})
    first_total = first_hand["total_pnl"]
    # The controller starts hand 2, but opening priority flips: the other seat opens it.
    second_hand = move(multi, first, "next_hand", {}, second)
    assert second_hand["phase"] == "trading" and second_hand["settlement_round"] == 1
    assert second_hand["total_pnl"] == first_total
    assert second_hand["round"] == 1 and second_hand["community_cards"] == []
    assert second_hand["market"] is None and second_hand["trades_this_round"] == 0
    assert second_hand["trades"] == [] and second_hand["positions"] == {first: 0, second: 0}
    assert second_hand["cash"] == {first: 0, second: 0}
    assert second_hand["available_actions"] == []
    assert observe(multi, second)["available_actions"] == ["open_market"]
    assert second_hand["opening_priority"] == {"hand": 2, "hand_opener": second, "round_opener": second}
    waiting = view(multi, first)
    assert waiting["active_player"] == second and waiting["available_actions"] == []
    assert waiting["opening_priority"] == second_hand["opening_priority"]
    assert "all_private_cards" not in second_hand and "settlement" not in second_hand
    state = snapshot(multi)
    assert state["settlement"] is None
    assert len(set(state["hands"][first] + state["hands"][second] + state["community"])) == 3
    multi = roundtrip(multi, players)
    move(multi, second, "open_market", {"bid": 9, "ask": 10}, first)
    move(multi, first, "sell", {}, None)
    last = settlement(multi, players)
    assert last["phase"] == "finished" and last["settlement_round"] == 2
    assert last["total_pnl"] == {
        player: first_total[player] + last["settlement"]["pnl"][player] for player in players}
    assert last["winners"] == [player for player in players
                              if last["total_pnl"][player] == max(last["total_pnl"].values())]
    multi = roundtrip(multi, players)
    rejected(multi, "invoke", first, "next_hand", {})


players = ("config-owner", "config-peer")
default = fresh(players)
assert observe(default, players[0])["config"] == {
    "private_cards_per_player": 2, "community_card_count": 5,
    "trades_per_round": 3, "settlement_rounds": 1,
}
championship = fresh(players, private_cards_per_player=2, community_card_count=5,
                     trades_per_round=2, settlement_rounds=50)
rules = observe(championship, players[0])["run_rules"]
assert rules == observe(championship, players[1])["run_rules"]
assert {key: value for key, value in rules.items() if key != "summary"} == {
    "hands": 50, "hands_settled": 0, "private_cards_per_player": 2,
    "community_cards_per_hand": 5, "trading_rounds_per_hand": 5,
    "executions_per_round": 2, "trades_per_hand": 10,
    "trades_per_completed_match": 500, "cards_in_settlement_value": 9,
}
assert rules["summary"] == [
    "This run has 50 hands, each freshly dealt, with one settlement after each hand.",
    "Two players; 2 private cards each and 5 community cards per hand.",
    "Each hand has 5 trading rounds with 2 executed contracts per round: 10 trades per hand and 500 trades in a completed match.",
    "One community card is revealed after each round's executions.",
    "The reveal of community card 5 settles the hand immediately; there is no post-reveal trading round.",
    "Each one-contract execution settles to the rank sum of all 9 dealt cards: both players' 2 private cards and all 5 community cards.",
    "Cash and positions reset after each settlement; cumulative P&L carries across all 50 hands and determines the winner.",
    "config-owner (first roster seat) controls next_hand. Opening priority alternates every round and flips with every hand: config-owner opens round 1 of odd-numbered hands and config-peer opens round 1 of even-numbered hands; the opener of round r in hand k is config-owner when k + r is even, otherwise config-peer.",
]
assert all(card not in " ".join(rules["summary"]) for card in snapshot(championship)["hands"][players[0]])
small = fresh(players, private_cards_per_player=1, community_card_count=1,
              trades_per_round=1, settlement_rounds=1)
small_rules = observe(small, players[0])["run_rules"]
assert small_rules["trades_per_hand"] == small_rules["trades_per_completed_match"] == 1
assert small_rules["cards_in_settlement_value"] == 3
assert small_rules["summary"][0] == "This run has 1 hand, each freshly dealt, with one settlement after each hand."
assert small_rules["summary"][-1] == "config-owner (first roster seat) controls next_hand. Opening priority alternates every round and flips with every hand: config-owner opens round 1 of odd-numbered hands and config-peer opens round 1 of even-numbered hands."
assert snapshot(small)["config"] == observe(small, players[0])["config"]
before = snapshot(championship)
observe(championship, players[0])
assert snapshot(championship) == before and "run_rules" not in before
# A full legal championship-shaped hand: five rounds, two executions each, five reveals.
state = snapshot(championship)
first, second = players
state["hands"] = {first: ["AS", "2H"], second: ["QD", "KC"]}
state["community"] = ["3C", "10S", "4D", "5H", "6S"]
assert request(championship, "restore", state) == ({}, {})
for index in range(5):
    opener, other = players[index % 2], players[(index + 1) % 2]
    assert observe(championship, opener)["round"] == index + 1
    move(championship, opener, "open_market", {"bid": 10, "ask": 12}, other)
    move(championship, other, "buy", {}, opener)
    assert observe(championship, opener)["trades_this_round"] == 1
    move(championship, opener, "open_market", {"bid": 20, "ask": 22}, other)
    move(championship, other, "sell", {}, other if index < 4 else first)
    assert observe(championship, first)["community_cards"] == state["community"][:index + 1]
final = settlement(championship, players)
assert final["phase"] == "settled" and final["settlement_round"] == 1
assert len(snapshot(championship)["trades"]) == 10
assert final["settlement"]["value"] == 1 + 2 + 12 + 13 + 3 + 10 + 4 + 5 + 6
assert final["total_pnl"] == final["settlement"]["pnl"]
assert observe(championship, first)["run_rules"]["hands_settled"] == 1
rejected(championship, "invoke", second, "next_hand", {})
move(championship, first, "next_hand", {}, second)
assert observe(championship, first)["total_pnl"] == final["total_pnl"]
assert observe(championship, first)["opening_priority"] == {
    "hand": 2, "hand_opener": second, "round_opener": second}
assert observe(championship, first)["positions"] == {first: 0, second: 0}

for settings in ({"private_cards_per_player": 25, "community_card_count": 2},
                 {"private_cards_per_player": 1, "community_card_count": 50},
                 {"trades_per_round": 100}, {"settlement_rounds": 101}):
    fresh(players, **settings)
invalid_settings = [
    {"unknown": 1}, {"private_cards_per_player": 25, "community_card_count": 5},
    {"private_cards_per_player": 26}, {"community_card_count": 51},
    {"trades_per_round": 101},
]
for field in ("private_cards_per_player", "community_card_count", "trades_per_round", "settlement_rounds"):
    invalid_settings.extend({field: value} for value in (0, -1, True, 1.5, "1"))
for settings in invalid_settings:
    rejected(default, "initialize", {
        "seats": [{"id": player, "role": "player"} for player in players],
        "config": settings,
    })
for seats in ([], [{"id": players[0], "role": "player"}],
              [{"id": players[0], "role": "player"}] * 2,
              [{"id": player, "role": "other"} for player in players],
              [{"id": player, "role": "player"} for player in (*players, "third")]):
    rejected(default, "initialize", {"seats": seats, "config": {}})


# The agent view carries the history a stateless seat needs, folded to stay small.
first, second = players = ("view-first", "view-second")
game = fresh(players, private_cards_per_player=1, community_card_count=1,
             trades_per_round=40)
for _ in range(18):
    move(game, first, "open_market", {"bid": 10, "ask": 12}, second)
    move(game, second, "buy", {}, first)
events = snapshot(game)["events"]
assert len(events) == 36
recent = view(game, first)
assert recent["recent_events"] == events[-30:] and recent["earlier_events_omitted"] == 6
assert view(game, second)["recent_events"] == recent["recent_events"]
assert recent["recent_events"][-1] == {"seq": 36, "hand": 1, "round": 1, "actor": second,
                                       "kind": "buy", "price": 12, "counterparty": first}
assert recent["progress"] == {"hands_settled": 0, "hands_total": 1, "trading_round": 1,
                              "rounds_per_hand": 1, "trades_this_round": 18, "trades_per_round": 40}
assert recent["past_hands"] == [] and "earlier_hands_summary" not in recent
game = fresh(players, private_cards_per_player=1, community_card_count=1,
             trades_per_round=1, settlement_rounds=13)
for hand in range(1, 14):
    opener, other = players if hand % 2 else players[::-1]
    move(game, opener, "open_market", {"bid": hand, "ask": hand + 1}, other)
    move(game, other, "buy", {}, None if hand == 13 else first)
    settled = view(game, second)
    assert settled["phase"] == ("finished" if hand == 13 else "settled")
    assert settled["all_private_cards"] == snapshot(game)["hands"]
    assert settled["past_hands"][-1] == snapshot(game)["history"][-1]
    assert settled["past_hands"][-1]["hand"] == hand
    if hand < 13:
        move(game, first, "next_hand", {}, players[hand % 2])
        fresh_hand = view(game, first)
        assert fresh_hand["community_cards"] == [] and fresh_hand["market"] is None
        assert "all_private_cards" not in fresh_hand and "settlement" not in fresh_hand
        assert fresh_hand["total_pnl"] == settled["total_pnl"]
final = view(game, first)
history = snapshot(game)["history"]
assert len(history) == 13 and final["past_hands"] == history[-10:]
assert final["earlier_hands_summary"] == {
    "hands": 3, "pnl": {player: sum(hand["pnl"][player] for hand in history[:3]) for player in players}}
assert final["winners"] == observe(game, first)["winners"]
game = roundtrip(game, players)
rejected(game, "agent_view", "unknown")


# Opening priority is derived from hand and round alone: the opener of round r in hand k
# (both one-based) is players[(k + r) % 2], whoever the quote owner or controller is.
def opener_of(players, hand, round_number):
    return players[(hand + round_number) % 2]


def quick(game, player, action, arguments, next_player):
    """A move checked by the fields that matter, without the full per-move audit."""
    result, metadata = request(game, "invoke", player, action, arguments)
    assert game.state["active"] == next_player
    waiting = metadata["progress"]["waiting_for"]
    assert waiting == ([] if next_player is None else [next_player])
    return result


def play_match(players, hands, rounds):
    """Play whole hands with one execution per round; return how often each seat opened."""
    first = players[0]
    game = fresh(players, private_cards_per_player=1, community_card_count=rounds,
                 trades_per_round=1, settlement_rounds=hands)
    openings = {player: 0 for player in players}
    for hand in range(1, hands + 1):
        if hand > 1:
            # The controller is the first seat even when it is not the opener.
            opener = opener_of(players, hand, 1)
            started = move(game, first, "next_hand", {}, opener) if hand < 4 else quick(
                game, first, "next_hand", {}, opener)
            if hand < 4:
                assert started["opening_priority"] == {
                    "hand": hand, "hand_opener": opener, "round_opener": opener}
                assert started["available_actions"] == (["open_market"] if opener == first else [])
                assert view(game, first)["active_player"] == opener
        for round_number in range(1, rounds + 1):
            opener = opener_of(players, hand, round_number)
            other = players[1] if opener == players[0] else players[0]
            assert game.state["active"] == opener and game.state["round"] == round_number - 1
            assert game.opening_priority()["round_opener"] == opener
            openings[opener] += 1
            quick(game, opener, "open_market", {"bid": 4, "ask": 6}, other)
            if round_number < rounds:
                follower = opener_of(players, hand, round_number + 1)
            else:
                follower = None if hand == hands else first
            quick(game, other, "buy", {}, follower)
        if hand < 4:
            game = roundtrip(game, players)
    return openings


for players in (("alice", "bob"), ("zed", "amy")):
    for rounds in (1, 3, 4):
        openings = play_match(players, 2, rounds)
        assert sum(openings.values()) == 2 * rounds
        if rounds % 2 == 1:
            # An odd round count gives the first seat the extra opening only in odd hands.
            assert openings == {players[0]: rounds // 2 + 1 + rounds // 2,
                                players[1]: rounds // 2 + 1 + rounds // 2}
        else:
            assert openings == {players[0]: rounds, players[1]: rounds}
thirty = play_match(("first-seat", "second-seat"), 30, 3)
assert thirty == {"first-seat": 45, "second-seat": 45}

# A trade that does not end a round hands the market to the quote owner; at a round
# boundary the calculated opener wins over that owner. Hand 2 is opened by the second seat.
first, second = players = ("tick", "tock")
game = fresh(players, private_cards_per_player=1, community_card_count=3,
             trades_per_round=2, settlement_rounds=2)
for round_number in (1, 2, 3):
    opener = opener_of(players, 1, round_number)
    other = second if opener == first else first
    move(game, opener, "open_market", {"bid": 1, "ask": 3}, other)
    move(game, other, "buy", {}, opener)
    move(game, opener, "open_market", {"bid": 1, "ask": 3}, other)
    move(game, other, "sell", {}, opener_of(players, 1, round_number + 1) if round_number < 3 else first)
move(game, first, "next_hand", {}, second)
move(game, second, "open_market", {"bid": 1, "ask": 3}, first)
move(game, first, "buy", {}, second)
move(game, second, "open_market", {"bid": 1, "ask": 3}, first)
mid_hand = roundtrip(game, players)
move(mid_hand, first, "sell", {}, first)
# Round 1 ended with the second seat as quote owner; round 2 belongs to the first seat.
assert observe(mid_hand, first)["opening_priority"] == {
    "hand": 2, "hand_opener": second, "round_opener": first}
assert observe(mid_hand, first)["available_actions"] == ["open_market"]
assert snapshot(mid_hand)["market"] is None and snapshot(mid_hand)["active"] == first


# Public chat reaches both seats in their next view, in order, without moving the game.
first, second = players = ("chat-first", "chat-second")
game = fresh(players, private_cards_per_player=1, community_card_count=2, trades_per_round=1,
             settlement_rounds=2)
before = snapshot(game)
for notice_seq, (seat, text) in enumerate(
        ((first, "good luck"), (second, "x" * 5000), (first, "your move?")), start=1):
    result, metadata = request(game, "handle_input", seat, text)
    assert set(metadata) == {"notices"} and len(metadata["notices"]) == 1
    notice = metadata["notices"][0]
    assert notice["to"] == "*" and notice["wake"] is False
    assert notice["message"] == f"[hand 1 round 1] {seat}: {text}"
    assert result == {"observation": {"posted": notice_seq}}
state = snapshot(game)
assert state["revision"] == before["revision"] and state["active"] == first
assert state["market"] is None and state["trades"] == []
assert [event["seq"] for event in state["events"]] == [1, 2, 3]
for reader in players:
    feed = view(game, reader)["recent_events"]
    assert feed == state["events"]
    assert [event["actor"] for event in feed] == [first, second, first]
    assert feed[1]["truncated"] is True and len(feed[1]["text"]) == 300
    assert all(card not in card_values(feed) for card in
               state["hands"][first if reader == second else second] + state["community"])
game = roundtrip(game, players)
move(game, first, "open_market", {"bid": 1, "ask": 2}, second)
request(game, "handle_input", second, "nice quote")
feed = view(game, first)["recent_events"]
assert [event["kind"] for event in feed] == ["chat", "chat", "chat", "open_market", "chat"]
assert [event["seq"] for event in feed] == [1, 2, 3, 4, 5]
assert feed[3] == {"seq": 4, "hand": 1, "round": 1, "actor": first, "kind": "open_market",
                   "bid": 1, "ask": 2}
assert snapshot(game)["revision"] == 1
# Chat sent while a hand is settled stays in the feed into the next hand.
move(game, second, "buy", {}, second)
move(game, second, "open_market", {"bid": 1, "ask": 2}, first)
move(game, first, "buy", {}, first)
request(game, "handle_input", first, "gg")
after = view(game, second)
assert after["phase"] == "settled"
assert after["recent_events"][-1] == {"seq": 9, "hand": 1, "round": 2, "actor": first,
                                      "kind": "chat", "text": "gg"}
moved = move(game, first, "next_hand", {}, second)
carried = view(game, second)
assert carried["recent_events"] == after["recent_events"]
assert carried["recent_events"][-1]["text"] == "gg"
rejected(game, "handle_input", "stranger", "hello")
rejected(game, "handle_input", first, "x" * 20000)
rejected(game, "handle_input", first, 5)

# Many messages: the feed keeps 30 and says exactly how many came before it.
for number in range(40):
    request(game, "handle_input", second, f"message {number}")
feed = view(game, first)
assert len(feed["recent_events"]) == 30
assert feed["earlier_events_omitted"] == snapshot(game)["event_seq"] - 30
assert [event["seq"] for event in feed["recent_events"]] == list(
    range(snapshot(game)["event_seq"] - 29, snapshot(game)["event_seq"] + 1))
assert feed["recent_events"][-1]["text"] == "message 39"


# A settled hand's history says how it was traded, not just how it ended, and stays small.
first, second = players = ("hist-first", "hist-second")
game = fresh(players, private_cards_per_player=1, community_card_count=2, trades_per_round=1,
             settlement_rounds=3)
move(game, first, "open_market", {"bid": 10, "ask": 14}, second)
move(game, second, "improve_market", {"bid": 11, "ask": 14}, first)
move(game, first, "sell", {}, second)
request(game, "handle_input", first, "not a market action")
move(game, second, "open_market", {"bid": 20, "ask": 22}, first)
move(game, first, "buy", {}, first)
hand_one = view(game, second)["past_hands"][0]
assert hand_one["actions"] == [
    "R1 hist-first quotes 10/14", "R1 hist-second improves to 11/14",
    "R1 hist-first sells at 11 to hist-second", "R2 hist-second quotes 20/22",
    "R2 hist-first buys at 22 from hist-second"]
assert set(hand_one) == {"hand", "value", "pnl", "cards", "board", "actions"}
assert set(hand_one["cards"]) == set(players)
move(game, first, "next_hand", {}, second)
current = view(game, first)
assert all(card not in card_values({key: value for key, value in current.items() if key != "past_hands"})
           for card in snapshot(game)["hands"][second] + snapshot(game)["community"])

# The serialized view stays under its byte budget by dropping the oldest history first.
game = fresh(players, private_cards_per_player=1, community_card_count=1,
             trades_per_round=100, settlement_rounds=5)
for hand in range(1, 6):
    opener, other = players if hand % 2 else players[::-1]
    for index in range(100):
        quick(game, opener, "open_market", {"bid": 10 + index % 5, "ask": 20 + index % 7}, other)
        quick(game, other, "buy" if index % 2 == 0 else "sell", {},
              None if (hand == 5 and index == 99) else (first if index == 99 else opener))
    if hand < 5:
        quick(game, first, "next_hand", {}, players[hand % 2])
final = view(game, first)
assert len(json.dumps(final, separators=(",", ":")).encode("utf-8")) <= 24 * 1024
history = snapshot(game)["history"]
assert [hand["hand"] for hand in final["past_hands"]] == [1, 2, 3, 4, 5]
dropped = [hand for hand in final["past_hands"] if hand["actions"] == []]
kept = [hand for hand in final["past_hands"] if hand["actions"]]
assert dropped and kept and final["past_hands"] == dropped + kept
for shown, real in zip(final["past_hands"], history):
    if shown["actions"]:
        assert len(shown["actions"]) == 200 and "actions_omitted" not in shown
    else:
        assert shown["actions_omitted"] == 200
    assert {key: shown[key] for key in ("hand", "value", "pnl", "cards", "board")} == {
        key: real[key] for key in ("hand", "value", "pnl", "cards", "board")}
assert len(final["recent_events"]) + final["earlier_events_omitted"] == snapshot(game)["event_seq"]
