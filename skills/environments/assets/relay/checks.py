"""Behavioral checks using the draft SDK's request metadata boundaries."""
from copy import deepcopy

import sdk
from environment import Relay


def request(environment, method, *arguments):
    sdk._begin_request(method)
    result = getattr(environment, method)(*arguments)
    return result, deepcopy(sdk._response_metadata())


for size in (1, 2, 4, 16):
    ids = [f"player-{index}" for index in range(1, size + 1)]
    relay = Relay()
    result, metadata = request(relay, "initialize", {
        "seats": [{"id": seat, "role": "player"} for seat in ids],
        "config": {"rounds": 2},
    })
    assert (result, metadata) == ({}, {
        "notices": [{"to": [ids[0]], "message": "Your turn", "wake": True}],
        "progress": {"revision": 0, "waiting_for": [ids[0]]},
    })
    for turn in range(2 * size):
        expected = ids[turn % size]
        state = deepcopy(relay.snapshot())
        observation, metadata = request(relay, "observe", expected)
        assert (observation, metadata) == ({
            "you": expected, "turn": turn, "next": expected, "finished": False,
        }, {})
        assert relay.snapshot() == state

        restored = Relay()
        result, metadata = request(restored, "restore", state)
        assert (result, metadata, restored.snapshot()) == ({}, {}, state)
        assert restored.observe(expected) == observation
        relay = restored

        result, metadata = request(relay, "handle_input", expected, "ready")
        assert (result, metadata) == ({"observation": observation}, {
            "notices": [{"to": "*", "message": f"{expected}: ready", "wake": False}],
        })
        assert relay.snapshot() == state

        for seat, action, arguments in (("unknown", "pass", {}),
                                        (expected, "wrong-action", {}),
                                        (expected, "pass", {"extra": 1})):
            try:
                request(relay, "invoke", seat, action, arguments)
            except ValueError:
                pass
            else:
                raise AssertionError("invalid move accepted")
            assert relay.snapshot() == state
        if size > 1:
            try:
                request(relay, "invoke", ids[(turn + 1) % size], "pass", {})
            except ValueError:
                pass
            else:
                raise AssertionError("out-of-turn move accepted")
            assert relay.snapshot() == state

        result, metadata = request(relay, "invoke", expected, "pass", {})
        notices = [{"to": "*", "message": f"{expected} passed the baton", "wake": False}]
        if turn + 1 == 2 * size:
            assert (result, metadata) == (turn + 1, {
                "notices": notices,
                "progress": {"revision": turn + 1, "waiting_for": []},
                "finished": {"summary": "Relay completed"},
            })
        else:
            next_seat = ids[(turn + 1) % size]
            notices.append({"to": [next_seat], "message": "Your turn", "wake": True})
            assert (result, metadata) == (turn + 1, {
                "notices": notices,
                "progress": {"revision": turn + 1, "waiting_for": [next_seat]},
            })
    state = deepcopy(relay.snapshot())
    try:
        request(relay, "invoke", ids[0], "pass", {})
    except ValueError:
        pass
    else:
        raise AssertionError("move after completion accepted")
    assert relay.snapshot() == state
    restored = Relay()
    request(restored, "restore", state)
    assert restored.observe(ids[0]) == {
        "you": ids[0], "turn": 2 * size, "next": None, "finished": True,
    }
