"""Behavioral checks. Run with the draft's selected execution backend."""
import json
from random import Random

import sdk
from environment import Holdem
from evaluator_checks import evaluator_checks


def committed(game, method, *args):
    sdk._begin_request(method)
    result = getattr(game, method)(*args)
    meta = sdk._response_metadata()
    turn = game.state["turn"]
    wakes = [notice for notice in meta.get("notices", []) if notice["wake"]]
    assert [notice["to"] for notice in wakes] == ([[turn]] if turn else [])
    assert meta["progress"]["waiting_for"] == ([turn] if turn else [])
    assert meta["progress"]["revision"] == game.state["revision"]
    assert ("finished" in meta) == (game.state["phase"] == "finished")
    for notice in meta.get("notices", []):
        if notice["to"] == "*":
            assert not notice["wake"]
    conserved(game)
    return result


def new_game(count=3, hands=1, starting_chips=1000):
    game = Holdem()
    params = {
        "seats": [{"id": f"p{i}", "role": "player"} for i in range(count)],
        "config": {"hands": hands, "starting_chips": starting_chips,
                   "small_blind": 5, "big_blind": 10},
    }
    committed(game, "initialize", params)
    return game


def conserved(game):
    state = game.state
    expected = len(state["seats"]) * state["config"]["starting_chips"]
    total = sum(p["stack"] + p["total_paid"] for p in state["players"].values())
    assert total == expected, (total, expected, state)
    for player in state["players"].values():
        assert player["stack"] >= 0
        assert 0 <= player["street_paid"] <= player["total_paid"]
    for seat in state["seats"]:
        view = game.observe(seat)
        assert len(json.dumps(game.agent_view(seat)).encode()) <= 32768
        assert view["pot"] == sum(p["total_paid"] for p in state["players"].values())
        assert sum(p["stack"] for p in view["players"]) + view["pot"] == expected


def rejected(game, seat, action, arguments=None):
    before = json.loads(json.dumps(game.snapshot()))
    sdk._begin_request("invoke")
    try:
        game.invoke(seat, action, {} if arguments is None else arguments)
    except ValueError:
        pass
    else:
        raise AssertionError(f"Accepted illegal action: {seat} {action} {arguments}")
    assert game.snapshot() == before
    assert sdk._response_metadata() == {}


def act(game, action, arguments=None):
    revision = game.state["revision"]
    committed(game, "invoke", game.state["turn"], action,
              {} if arguments is None else arguments)
    assert game.state["revision"] > revision


def passive(game):
    legal = game.observe(game.state["turn"])["legal_actions"]
    act(game, "check" if legal.get("check") else "call")


def finish_passively(game, limit=250):
    for _ in range(limit):
        if game.state["phase"] == "finished":
            break
        passive(game)
    assert game.state["phase"] == "finished"
    assert game.state["turn"] is None
    conserved(game)


def river_game(stacks, board, holes):
    game = new_game(len(stacks), starting_chips=sum(stacks) // len(stacks))
    state = game.snapshot()
    state.update(board=board, phase="river", turn="p0", current_bet=0,
                 last_full_raise=10, history=[],
                 hand_start_stacks={f"p{i}": stack for i, stack in enumerate(stacks)})
    for i, stack in enumerate(stacks):
        state["players"][f"p{i}"].update(
            stack=stack, hole=holes[i], folded=False, all_in=False,
            street_paid=0, total_paid=0, acted=False, last_faced=0)
    used = set(board + [card for hand in holes for card in hand])
    state["deck"] = [r + s for r in "23456789TJQKA" for s in "cdhs"
                     if r + s not in used]
    game.restore(json.loads(json.dumps(state)))
    conserved(game)
    return game


def check_rosters_and_privacy():
    for count in range(2, 10):
        game = new_game(count)
        dealt = [card for p in game.state["players"].values() for card in p["hole"]]
        assert len(dealt) == len(set(dealt)) == count * 2
        for seat in game.state["seats"]:
            view = game.observe(seat)
            assert view["hole_cards"] == game.state["players"][seat]["hole"]
            assert len(view["players"]) == count
            for player in view["players"]:
                assert not {"hole", "hole_cards", "deck"}.intersection(player)
            assert "deck" not in view
            assert "state" not in view
        finish_passively(game)
        assert len(game.state["board"]) == 5
        assert len(game.state["results"]) == 1
    for count in (0, 1, 10):
        game = Holdem()
        try:
            game.initialize({"seats": [{"id": f"p{i}", "role": "player"}
                                       for i in range(count)], "config": {}})
        except ValueError:
            pass
        else:
            raise AssertionError(f"Accepted roster size {count}")


def check_heads_up_and_invalid():
    game = new_game(2)
    seats = game.state["seats"]
    button = seats[game.state["button"]]
    other = next(seat for seat in seats if seat != button)
    assert game.state["turn"] == button
    assert game.state["players"][button]["street_paid"] == 5
    assert game.state["players"][other]["street_paid"] == 10
    before = game.snapshot()
    sdk._begin_request("handle_input")
    try:
        game.handle_input(button, "Can we make a deal?")
    except ValueError:
        pass
    else:
        raise AssertionError("Table talk was accepted")
    assert game.snapshot() == before
    assert sdk._response_metadata() == {}
    rejected(game, other, "fold")
    rejected(game, "outsider", "fold")
    rejected(game, button, "check")
    rejected(game, button, "unknown")
    rejected(game, button, "raise_to", {"amount": 19})
    rejected(game, button, "raise_to", {"amount": 1001})
    rejected(game, button, "raise_to", {"amount": True})
    rejected(game, button, "raise_to", {"amount": 20.5})
    rejected(game, button, "raise_to", {})
    rejected(game, button, "call", {"amount": 5})
    act(game, "call")
    assert game.state["turn"] == other
    rejected(game, other, "call")
    act(game, "check")
    assert game.state["phase"] == "flop"
    assert game.state["turn"] == other
    assert len(game.state["board"]) == 3
    assert game.observe(other)["legal_actions"]["raise_to"]["minimum"] == 10
    finish_passively(game)
    rejected(game, button, "check")


def check_runout_and_tie():
    game = new_game(2)
    act(game, "all_in")
    act(game, "call")
    assert game.state["phase"] == "finished"
    assert len(game.state["board"]) == 5
    assert len(game.state["results"]) == 1
    tie = river_game([100, 100], ["As", "Ks", "Qs", "Js", "Ts"],
                     [["2c", "3d"], ["4c", "5d"]])
    act(tie, "all_in")
    act(tie, "call")
    assert tie.state["phase"] == "finished"
    assert [tie.state["players"][s]["stack"] for s in tie.state["seats"]] == [100, 100]


def check_side_pots():
    game = river_game([100, 200, 300], ["2c", "3d", "7h", "9s", "Jc"],
                      [["As", "Ad"], ["Ks", "Kd"], ["Qs", "Qd"]])
    act(game, "all_in")
    act(game, "all_in")
    act(game, "call")
    assert game.state["phase"] == "finished"
    assert [game.state["players"][s]["stack"] for s in game.state["seats"]] == [300, 200, 100]
    assert len(game.state["results"]) == 1
    refund = river_game([100, 600, 200], ["2c", "3d", "7h", "9s", "Jc"],
                        [["As", "Ad"], ["Ks", "Kd"], ["Qs", "Qd"]])
    act(refund, "all_in")
    act(refund, "all_in")
    act(refund, "call")
    assert refund.state["phase"] == "finished"
    assert [refund.state["players"][s]["stack"] for s in refund.state["seats"]] == [300, 600, 0]
    assert refund.state["results"][0]["refunds"] == {"p1": 400}


def check_short_raises():
    board = ["2c", "3d", "7h", "9s", "Jc"]
    holes = [["As", "Ad"], ["Ks", "Kd"], ["Qs", "Qd"], ["Ts", "Td"]]
    game = river_game([1000, 150, 1850], board, holes[:3])
    act(game, "raise_to", {"amount": 100})
    act(game, "all_in")
    act(game, "call")
    assert game.state["turn"] == "p0"
    legal = game.observe("p0")["legal_actions"]
    assert legal.get("call")
    assert not legal.get("raise_to")
    assert not legal.get("all_in")
    rejected(game, "p0", "raise_to", {"amount": 250})
    rejected(game, "p0", "all_in")
    act(game, "call")
    assert game.state["phase"] == "finished"
    game = river_game([1000, 150, 200, 2650], board, holes)
    act(game, "raise_to", {"amount": 100})
    act(game, "all_in")
    act(game, "all_in")
    act(game, "call")
    assert game.state["turn"] == "p0"
    legal = game.observe("p0")["legal_actions"]
    assert legal["raise_to"]["minimum"] == 300
    assert legal["all_in"]
    act(game, "raise_to", {"amount": 300})
    assert game.state["turn"] == "p3"
    act(game, "call")
    assert game.state["phase"] == "finished"


def check_fold_and_odd_chip():
    game = new_game(3)
    folded = game.state["turn"]
    act(game, "fold")
    assert game.state["players"][folded]["folded"]
    rejected(game, folded, "call")
    act(game, "fold")
    assert game.state["phase"] == "finished"
    assert game.state["results"][0]["showdown"] == {}
    assert len(game.state["results"][0]["pots"]) == 1
    tie = river_game([100, 100, 100], ["As", "Ks", "Qs", "Js", "Ts"],
                     [["2c", "3d"], ["4c", "5d"], ["6c", "7d"]])
    state = tie.snapshot()
    for player in state["players"].values():
        player.update(stack=99, total_paid=1)
    tie.restore(state)
    act(tie, "fold")
    act(tie, "check")
    act(tie, "check")
    assert tie.state["phase"] == "finished"
    assert [tie.state["players"][s]["stack"] for s in tie.state["seats"]] == [99, 101, 100]
    assert "p0" not in tie.state["results"][0]["showdown"]


def check_short_big_blind():
    game = new_game(2)
    state = game.snapshot()
    draws = "As Ks Ad Kd 4c 2c 3d 7h 4d 9s 4h Jc".split()
    deck = [r + s for r in "23456789TJQKA" for s in "cdhs" if r + s not in draws]
    state.update(hand_number=0, decks=[deck + list(reversed(draws))], results=[])
    for seat, stack in [("p0", 1997), ("p1", 3)]:
        state["players"][seat].update(stack=stack, street_paid=0, total_paid=0)
    game.restore(state)
    sdk._begin_request("initialize")
    game._start_hand()
    game._attention()
    meta = sdk._response_metadata()
    assert game.state["phase"] == "finished" and game.state["turn"] is None
    assert "finished" in meta and meta["progress"]["waiting_for"] == []
    assert not any(n["wake"] for n in meta.get("notices", []))
    assert game.state["results"][0]["refunds"] == {"p0": 2}
    assert game.state["results"][0]["pots"][0]["amount"] == 6
    assert game.state["players"]["p0"]["stack"] == 2000
    conserved(game)


def check_seeded_play():
    for count in range(2, 10):
        rng = Random(7100 + count)
        game = new_game(count, hands=4, starting_chips=100)
        state = game.snapshot()
        cards = [r + s for r in "23456789TJQKA" for s in "cdhs"]
        rng.shuffle(cards)
        for player in state["players"].values():
            player["hole"] = [cards.pop(), cards.pop()]
        state["deck"] = cards
        for deck in state["decks"]:
            deck[:] = [r + s for r in "23456789TJQKA" for s in "cdhs"]
            rng.shuffle(deck)
        game.restore(state)
        for _ in range(1200):
            if game.state["phase"] == "finished":
                break
            legal = game.observe(game.state["turn"])["legal_actions"]
            choices = [a for a in ("fold", "check", "call", "all_in", "raise_to") if legal.get(a)]
            assert choices
            action = rng.choice(choices)
            args = {"amount": rng.randint(legal[action]["minimum"], legal[action]["maximum"])} if action == "raise_to" else {}
            act(game, action, args)
            for result in game.state["results"]:
                assert sum(result["stacks"].values()) == count * 100
                assert all(pot["eligible"] and pot["winners"] for pot in result["pots"])
                assert all(sum(pot["awards"].values()) == pot["amount"] for pot in result["pots"])
        assert game.state["phase"] == "finished"
        assert 1 <= len(game.state["results"]) <= 4


def check_restore_and_hand_limit():
    game = new_game(3, hands=2)
    passive(game)
    snapshot = json.loads(json.dumps(game.snapshot()))
    restored = Holdem()
    restored.restore(snapshot)
    assert restored.snapshot() == game.snapshot()
    for seat in game.state["seats"]:
        assert restored.observe(seat) == game.observe(seat)
    passive(game)
    passive(restored)
    assert restored.snapshot() == game.snapshot()
    finish_passively(game)
    assert len(game.state["results"]) == 2
    assert game.state["hand_number"] == 2
    finished = Holdem()
    finished.restore(json.loads(json.dumps(game.snapshot())))
    assert finished.snapshot() == game.snapshot()
    rejected(finished, "p0", "fold")


evaluator_checks()
check_rosters_and_privacy()
check_heads_up_and_invalid()
check_runout_and_tie()
check_side_pots()
check_short_raises()
check_fold_and_odd_chip()
check_short_big_blind()
check_seeded_play()
check_restore_and_hand_limit()
print("Holdem behavioral checks passed")
