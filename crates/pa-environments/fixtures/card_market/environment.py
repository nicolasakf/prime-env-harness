"""Sequential, two-seat trading game with private card information."""
import json
from copy import deepcopy
from random import SystemRandom

from sdk import finish, notify, progress

RANKS = ("A", "2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K")
SUITS = ("S", "H", "D", "C")
# An agent's view is resent every turn, so older history is folded, never listed.
RECENT_EVENTS = 30
RECENT_HANDS = 10
# Public chat can be far larger than a trade; the view carries a bounded excerpt and the
# serialized view stays well under the host's 32 KiB limit by dropping the oldest detail.
CHAT_VIEW_CHARS = 300
VIEW_BUDGET_BYTES = 24 * 1024


class TradingCards:
    def initialize(self, params):
        seats = params["seats"]
        players = [seat["id"] for seat in seats]
        if len(players) != 2 or len(set(players)) != 2 or any(seat["role"] != "player" for seat in seats):
            raise ValueError("exactly two distinct player seats required")
        cfg = {"private_cards_per_player": 2, "community_card_count": 5,
               "trades_per_round": 3, "settlement_rounds": 1}
        supplied = params.get("config", {})
        if not isinstance(supplied, dict) or set(supplied) - set(cfg):
            raise ValueError("unknown configuration field")
        cfg.update(supplied)
        if any(type(value) is not int or value < 1 for value in cfg.values()):
            raise ValueError("configuration values must be positive integers")
        if cfg["private_cards_per_player"] > 25 or cfg["community_card_count"] > 50 or cfg["trades_per_round"] > 100:
            raise ValueError("configuration exceeds field bounds")
        if 2 * cfg["private_cards_per_player"] + cfg["community_card_count"] > 52:
            raise ValueError("not enough cards in a standard 52-card deck")
        self.state = {"players": players, "revision": 0, "phase": "trading", "config": cfg,
                      "settlement_round": 0, "total_pnl": {p: 0 for p in players}, "history": [],
                      "events": [], "event_seq": 0}
        self.deal_hand()
        self.publish("Game started.")
        return {}

    def deal_hand(self):
        s = self.state
        deck = [rank + suit for suit in SUITS for rank in RANKS]
        SystemRandom().shuffle(deck)
        n = s["config"]["private_cards_per_player"]
        s.update({"hands": {p: deck[i*n:(i+1)*n] for i, p in enumerate(s["players"])},
                  "community": deck[2*n:2*n + s["config"]["community_card_count"]],
                  "round": 0, "revealed": [], "market": None, "trades_this_round": 0,
                  "trades": [], "positions": {p: 0 for p in s["players"]},
                  "cash": {p: 0 for p in s["players"]}, "settlement": None,
                  "active": self.opener(s["settlement_round"], 0), "phase": "trading",
                  # The feed outlives the hand so a seat still reads what was said between hands.
                  "events": s["events"][-RECENT_EVENTS:]})

    def opener(self, hand_index, round_index):
        """Who opens a round: priority alternates each round and flips with each hand."""
        return self.state["players"][(hand_index + round_index) % 2]

    def record(self, actor, kind, **detail):
        """Append one public event; trades, quotes and chat share this single source."""
        s = self.state
        s["event_seq"] += 1
        settled = s["phase"] in ("settled", "finished")
        s["events"].append({
            "seq": s["event_seq"],
            "hand": s["settlement_round"] if settled else s["settlement_round"] + 1,
            "round": s["config"]["community_card_count"] if settled else s["round"] + 1,
            "actor": actor, "kind": kind, **detail})

    def publish(self, message):
        s = self.state
        notify("*", message, wake=False)
        if s["phase"] in ("settled", "finished"):
            # Settled hands are public, but each notice stays below the SDK size limit.
            for player in s["players"]:
                notify("*", f"{player} hand: {', '.join(s['hands'][player])}", wake=False)
            notify("*", f"Settlement {s['settlement_round']}: value {s['settlement']['value']}; "
                   f"hand P&L {s['settlement']['pnl']}; cumulative P&L {s['total_pnl']}", wake=False)
        waiting = [] if s["phase"] == "finished" else [s["active"]]
        progress(s["revision"], waiting_for=waiting)
        if waiting:
            task = "Start the next hand with next_hand." if s["phase"] == "settled" else "Your trading turn."
            notify(waiting, task, actionable=True)
        else:
            winners = [p for p in s["players"] if s["total_pnl"][p] == max(s["total_pnl"].values())]
            finish(f"Game finished after {s['settlement_round']} settlements. "
                   f"Cumulative P&L: {s['total_pnl']}. Winners: {', '.join(winners)}.")

    def describe_actions(self):
        quote = {"type": "object", "properties": {
                 "bid": {"type": "integer", "title": "Bid", "description": "Price paid to a seller.", "minimum": 0},
                 "ask": {"type": "integer", "title": "Ask", "description": "Price charged to a buyer; must exceed the bid.", "minimum": 1}},
                 "required": ["bid", "ask"], "additionalProperties": False}
        empty = {"type": "object", "properties": {}, "additionalProperties": False}
        output = {"type": "object"}
        return [
            {"name": "next_hand", "description": "First roster seat starts the next freshly dealt hand after viewing the previous settlement; only cumulative P&L carries over.", "input_schema": empty, "output_schema": output, "roles": ["player"]},
            {"name": "open_market", "description": "Active opener posts a new two-sided integer market (bid < ask); also used by the executed quote owner after a non-final trade.", "input_schema": quote, "output_schema": output, "roles": ["player"]},
            {"name": "improve_market", "description": "Active player replaces the entire two-sided market, raising bid and/or lowering ask; at least one side must improve and the active player owns both quotes.", "input_schema": quote, "output_schema": output, "roles": ["player"]},
            {"name": "buy", "description": "Active player buys one contract at the ask from its owner; cannot execute own quote.", "input_schema": empty, "output_schema": output, "roles": ["player"]},
            {"name": "sell", "description": "Active player sells one contract at the bid to its owner; cannot execute own quote.", "input_schema": empty, "output_schema": output, "roles": ["player"]},
        ]

    def observe(self, participant):
        if participant not in self.state["players"]:
            raise ValueError("unknown participant")
        s = self.state
        visible = {"phase": s["phase"], "you": participant, "player_order": list(s["players"]),
                   "settlement_round": s["settlement_round"], "total_pnl": dict(s["total_pnl"]),
                   "round": min(s["round"] + 1, s["config"]["community_card_count"]),
                   "opening_priority": self.opening_priority(), "config": deepcopy(s["config"]),
                   "community_cards": list(s["revealed"]),
                   "active_player": s["active"], "market": deepcopy(s["market"]),
                   "run_rules": self.run_rules(),
                   "trades_this_round": s["trades_this_round"],
                   "trades": [{**trade, "round": trade["round"] + 1} for trade in s["trades"]],
                   "positions": dict(s["positions"]), "cash": dict(s["cash"])}
        visible["your_private_cards"] = list(s["hands"].get(participant, []))
        visible["opponent_private_card_count"] = len(s["hands"].get(self.other(participant), []))
        visible["available_actions"] = self.available(participant)
        if s["phase"] in ("settled", "finished"):
            visible["all_private_cards"] = deepcopy(s["hands"])
            visible["settlement"] = deepcopy(s["settlement"])
        if s["phase"] == "finished":
            visible["winners"] = [p for p in s["players"] if s["total_pnl"][p] == max(s["total_pnl"].values())]
        return visible

    def agent_view(self, participant):
        """Everything an agent sees each turn: current state plus a bounded history.

        Fixed rules live in the instructions and the run configuration in the system
        prompt, so neither is repeated here. Only public events and the seat's own
        cards appear; the opponent's cards are revealed only by settlement. Public chat
        and market events arrive in one ordered feed, and the serialized view is held
        under a byte budget by dropping the oldest detail, always saying how much.
        """
        s = self.state
        if participant not in s["players"]:
            raise ValueError("unknown participant")
        cfg = s["config"]
        events = deepcopy(s["events"][-RECENT_EVENTS:])
        view = {"you": participant, "opponent": self.other(participant),
                "player_order": list(s["players"]), "phase": s["phase"],
                "opening_priority": self.opening_priority(),
                "progress": {"hands_settled": s["settlement_round"], "hands_total": cfg["settlement_rounds"],
                             "trading_round": min(s["round"] + 1, cfg["community_card_count"]),
                             "rounds_per_hand": cfg["community_card_count"],
                             "trades_this_round": s["trades_this_round"],
                             "trades_per_round": cfg["trades_per_round"]},
                "community_cards": list(s["revealed"]),
                "your_private_cards": list(s["hands"].get(participant, [])),
                "active_player": s["active"], "market": deepcopy(s["market"]),
                "positions": dict(s["positions"]), "cash": dict(s["cash"]),
                "total_pnl": dict(s["total_pnl"]),
                "available_actions": self.available(participant),
                "recent_events": events}
        if s["event_seq"] > len(events):
            view["earlier_events_omitted"] = s["event_seq"] - len(events)
        history = s["history"]
        view["past_hands"] = deepcopy(history[-RECENT_HANDS:])
        if len(history) > RECENT_HANDS:
            older = history[:-RECENT_HANDS]
            view["earlier_hands_summary"] = {
                "hands": len(older),
                "pnl": {p: sum(hand["pnl"][p] for hand in older) for p in s["players"]}}
        if s["phase"] in ("settled", "finished"):
            view["all_private_cards"] = deepcopy(s["hands"])
            view["settlement"] = deepcopy(s["settlement"])
        if s["phase"] == "finished":
            view["winners"] = [p for p in s["players"] if s["total_pnl"][p] == max(s["total_pnl"].values())]
        return self.fit(view)

    @staticmethod
    def fit(view):
        """Drop the oldest detail until the view fits its byte budget, counting what went."""
        def size():
            return len(json.dumps(view, separators=(",", ":")).encode("utf-8"))
        for hand in view["past_hands"]:
            if size() <= VIEW_BUDGET_BYTES:
                return view
            if hand["actions"]:
                hand["actions_omitted"] = len(hand["actions"])
                hand["actions"] = []
        while size() > VIEW_BUDGET_BYTES and len(view["recent_events"]) > 1:
            del view["recent_events"][0]
            view["earlier_events_omitted"] = view.get("earlier_events_omitted", 0) + 1
        return view

    def opening_priority(self):
        """Who opens, derived from committed state: hand and round alone decide it."""
        s = self.state
        hand_index = s["settlement_round"]
        if s["phase"] == "finished":
            return None
        round_index = 0 if s["phase"] == "settled" else s["round"]
        return {"hand": hand_index + 1, "hand_opener": self.opener(hand_index, 0),
                "round_opener": self.opener(hand_index, round_index)}

    def run_rules(self):
        """Configuration-derived rules summary; computed fresh, never stored in state."""
        s = self.state
        cfg = s["config"]
        n, c = cfg["private_cards_per_player"], cfg["community_card_count"]
        t, h = cfg["trades_per_round"], cfg["settlement_rounds"]
        first, second = s["players"]
        plural = lambda count, word: f"{count} {word}" + ("" if count == 1 else "s")
        summary = [
            f"This run has {plural(h, 'hand')}, each freshly dealt, with one settlement after each hand.",
            f"Two players; {plural(n, 'private card')} each and {plural(c, 'community card')} per hand.",
            f"Each hand has {plural(c, 'trading round')} with {plural(t, 'executed contract')} per round: "
            f"{plural(c * t, 'trade')} per hand and {plural(c * t * h, 'trade')} in a completed match.",
            "One community card is revealed after each round's executions.",
            f"The reveal of community card {c} settles the hand immediately; there is no post-reveal trading round.",
            f"Each one-contract execution settles to the rank sum of all {2 * n + c} dealt cards: "
            f"both players' {plural(n, 'private card')} and all {plural(c, 'community card')}.",
            f"Cash and positions reset after each settlement; cumulative P&L carries across all {plural(h, 'hand')} "
            "and determines the winner.",
            f"{first} (first roster seat) controls next_hand. Opening priority alternates every round and flips "
            f"with every hand: {first} opens round 1 of odd-numbered hands and {second} opens round 1 of "
            f"even-numbered hands" + ("; the opener of round r in hand k is "
            f"{first} when k + r is even, otherwise {second}." if c > 1 else "."),
        ]
        return {"hands": h, "hands_settled": s["settlement_round"], "private_cards_per_player": n,
                "community_cards_per_hand": c, "trading_rounds_per_hand": c,
                "executions_per_round": t, "trades_per_hand": c * t,
                "trades_per_completed_match": c * t * h, "cards_in_settlement_value": 2 * n + c,
                "summary": summary}

    def other(self, player):
        return next(p for p in self.state["players"] if p != player)

    def available(self, participant):
        s = self.state
        if s["phase"] == "settled":
            return ["next_hand"] if participant == s["players"][0] else []
        if s["phase"] != "trading" or s["active"] != participant:
            return []
        if s["market"] is None:
            return ["open_market"]
        m = s["market"]
        result = []
        if m["ask_owner"] != participant:
            result.append("buy")
        if m["bid_owner"] != participant:
            result.append("sell")
        if m["ask"] - m["bid"] > 1:
            result.append("improve_market")
        return result

    def handle_input(self, participant, text):
        """Public table chat, open only to the active seat: both players see it in their next view; it never wakes anyone."""
        if participant not in self.state["players"]:
            raise ValueError("unknown participant")
        # Chat is refused off-turn: a rejection carries no state, so a model has nothing new to
        # act on and ends its turn, where an accepted message would hand it a fresh state and
        # keep one never-ending turn (and its ever-growing context) alive just to wait.
        if self.state["active"] != participant:
            raise ValueError("chat is only open on your own turn; end your turn and wait for a wake")
        if not isinstance(text, str) or len(f"{participant}: {text}".encode("utf-8")) > 16384:
            raise ValueError("chat message must fit within 16 KiB including sender")
        excerpt = {"text": text[:CHAT_VIEW_CHARS]}
        if len(text) > CHAT_VIEW_CHARS:
            excerpt["truncated"] = True
        self.record(participant, "chat", **excerpt)
        event = self.state["events"][-1]
        notify("*", f"[hand {event['hand']} round {event['round']}] {participant}: {text}", wake=False)
        # The host requires exactly {"observation": ...} but accepts any JSON inside it. The
        # caller already receives its updated state beside this result, so repeating a full
        # observation here only doubles what every chat message adds to a context.
        return {"observation": {"posted": event["seq"]}}

    def invoke(self, participant, action, arguments):
        if participant not in self.state["players"]:
            raise ValueError("unknown participant")
        if action not in self.available(participant):
            raise ValueError("action unavailable: not your turn, illegal side, or game finished")
        if not isinstance(arguments, dict):
            raise ValueError("arguments must be an object")
        s = self.state
        if action == "next_hand":
            if arguments:
                raise ValueError("next_hand takes no arguments")
            self.deal_hand()
            message = f"{participant} started hand {s['settlement_round'] + 1}."
        elif action in ("open_market", "improve_market"):
            if set(arguments) != {"bid", "ask"} or any(type(arguments[k]) is not int for k in arguments):
                raise ValueError("market requires integer bid and ask")
            bid, ask = arguments["bid"], arguments["ask"]
            if bid < 0 or bid >= ask:
                raise ValueError("market requires 0 <= bid < ask")
            if action == "open_market":
                s["market"] = {"bid": bid, "ask": ask, "bid_owner": participant, "ask_owner": participant}
            else:
                previous = s["market"]
                if bid < previous["bid"] or ask > previous["ask"] or (bid == previous["bid"] and ask == previous["ask"]):
                    raise ValueError("market must strictly improve without worsening either side")
                s["market"] = {"bid": bid, "ask": ask,
                               "bid_owner": participant, "ask_owner": participant}
            s["active"] = self.other(participant)
            self.record(participant, action, bid=bid, ask=ask)
            message = f"{participant} {action}: bid {bid}, ask {ask}."
        elif action in ("buy", "sell"):
            if arguments:
                raise ValueError("buy and sell take no arguments")
            m = s["market"]
            provider = m["ask_owner"] if action == "buy" else m["bid_owner"]
            price = m["ask"] if action == "buy" else m["bid"]
            buyer, seller = (participant, provider) if action == "buy" else (provider, participant)
            s["positions"][buyer] += 1
            s["positions"][seller] -= 1
            s["cash"][buyer] -= price
            s["cash"][seller] += price
            s["trades_this_round"] += 1
            s["trades"].append({"round": s["round"], "buyer": buyer, "seller": seller, "price": price})
            s["market"] = None
            self.record(participant, action, price=price, counterparty=provider)
            message = f"{buyer} bought one contract from {seller} at {price}."
            if s["trades_this_round"] == s["config"]["trades_per_round"]:
                s["revealed"].append(s["community"][s["round"]])
                s["events"][-1]["revealed"] = s["revealed"][-1]
                s["round"] += 1
                message += f" Round {s['round']} complete; revealed {s['revealed'][-1]}."
                s["trades_this_round"] = 0
                if s["round"] == s["config"]["community_card_count"]:
                    value = sum(self.rank(card) for hand in s["hands"].values() for card in hand)
                    value += sum(self.rank(card) for card in s["community"])
                    pnl = {p: s["cash"][p] + s["positions"][p] * value for p in s["players"]}
                    for player in s["players"]:
                        s["total_pnl"][player] += pnl[player]
                    s["settlement_round"] += 1
                    s["settlement"] = {"value": value, "pnl": pnl,
                                       "winners": [p for p in s["players"] if pnl[p] == max(pnl.values())]}
                    # The hand's public market behavior, compact: how each side traded, not what was said.
                    actions = [self.describe(event) for event in s["events"]
                               if event["hand"] == s["settlement_round"] and event["kind"] != "chat"]
                    s["history"].append({"hand": s["settlement_round"], "value": value, "pnl": pnl,
                                         "cards": deepcopy(s["hands"]), "board": list(s["community"]),
                                         "actions": actions})
                    s["phase"] = "finished" if s["settlement_round"] == s["config"]["settlement_rounds"] else "settled"
                    s["active"] = None if s["phase"] == "finished" else s["players"][0]
                else:
                    s["active"] = self.opener(s["settlement_round"], s["round"])
            else:
                s["active"] = provider
        s["revision"] += 1
        self.publish(message)
        # The caller reads its updated state from observe/agent_view, not from here.
        return {"event": message}

    @staticmethod
    def describe(event):
        """One market event as a short line for a settled hand's history."""
        who, kind, label = event["actor"], event["kind"], f"R{event['round']}"
        if kind == "buy":
            return f"{label} {who} buys at {event['price']} from {event['counterparty']}"
        if kind == "sell":
            return f"{label} {who} sells at {event['price']} to {event['counterparty']}"
        verb = "quotes" if kind == "open_market" else "improves to"
        return f"{label} {who} {verb} {event['bid']}/{event['ask']}"

    @staticmethod
    def rank(card):
        return RANKS.index(card[:-1]) + 1

    def snapshot(self):
        return deepcopy(self.state)

    def restore(self, snapshot):
        self.state = deepcopy(snapshot)
        return {}

    def dispose(self):
        return {}


environment = TradingCards()
