"""Fixed-hand no-limit Hold'em. All authoritative state stays in snapshots."""
from copy import deepcopy
import json
from random import SystemRandom

from sdk import notify, progress, finish
from poker_eval import rank_hand, hand_name


class Holdem:
    def initialize(self, params):
        seats = [s["id"] for s in params["seats"]]
        config = {"hands": 20, "starting_chips": 1000, "small_blind": 5, "big_blind": 10}
        config.update(params.get("config", {}))
        if not 2 <= len(seats) <= 9 or len(set(seats)) != len(seats):
            raise ValueError("Use 2–9 unique players")
        if not 1 <= config["small_blind"] < config["big_blind"] <= config["starting_chips"]:
            raise ValueError("Require small_blind < big_blind <= starting_chips")
        decks = []
        for _ in range(config["hands"]):
            deck = [r + s for r in "23456789TJQKA" for s in "cdhs"]
            SystemRandom().shuffle(deck)
            decks.append(deck)
        self.state = {
            "seats": seats, "config": config, "decks": decks, "players": {
                s: {"stack": config["starting_chips"], "hole": [], "folded": False,
                    "all_in": False, "street_paid": 0, "total_paid": 0,
                    "acted": False, "last_faced": 0} for s in seats},
            "button": 0, "hand_number": 0, "revision": 0, "results": [],
            "stats": {s: {"hands": 0, "folds": 0, "checks": 0, "calls": 0,
                          "raises": 0, "pots_won": 0} for s in seats},
        }
        self._start_hand()
        self._attention()
        return {}

    def describe_actions(self):
        actions = []
        for name, description in [
            ("fold", "Surrender your hand."),
            ("check", "Pass when nothing is owed."),
            ("call", "Match the current bet, or call all-in if short."),
            ("all_in", "Commit your entire remaining stack when legal."),
            ("raise_to", "Bet or raise to a TOTAL street contribution of amount."),
        ]:
            schema = {"type": "object", "properties": {}, "additionalProperties": False}
            if name == "raise_to":
                schema["properties"] = {"amount": {"type": "integer", "minimum": 1,
                    "title": "Total street contribution", "description": "Not an added amount; see your legal raise bounds."}}
                schema["required"] = ["amount"]
            actions.append({"name": name, "description": description,
                "input_schema": schema, "output_schema": {"type": "object"}, "roles": ["player"]})
        return actions

    def _ordered(self, after):
        seats = self.state["seats"]
        i = seats.index(after)
        return seats[i + 1:] + seats[:i + 1]

    def _live(self):
        return [s for s, p in self.state["players"].items() if not p["folded"]]

    def _pay(self, seat, amount):
        p = self.state["players"][seat]
        amount = min(amount, p["stack"])
        p["stack"] -= amount
        p["street_paid"] += amount
        p["total_paid"] += amount
        p["all_in"] = p["stack"] == 0
        return amount

    def _start_hand(self):
        st = self.state
        funded = [s for s in st["seats"] if st["players"][s]["stack"] > 0]
        if st["hand_number"] >= st["config"]["hands"] or len(funded) < 2:
            st["phase"], st["turn"] = "finished", None
            return
        if st["hand_number"]:
            old_button = st["seats"][st["button"]]
            button = next(s for s in self._ordered(old_button) if s in funded)
            st["button"] = st["seats"].index(button)
        button = st["seats"][st["button"]]
        st["hand_number"] += 1
        st.update(deck=st["decks"].pop(0), board=[], history=[], phase="preflop",
                  current_bet=st["config"]["big_blind"], last_full_raise=st["config"]["big_blind"])
        st["hand_start_stacks"] = {s: p["stack"] for s, p in st["players"].items()}
        for s, p in st["players"].items():
            p.update(hole=[], folded=s not in funded, all_in=False,
                     street_paid=0, total_paid=0, acted=False, last_faced=0)
            if s in funded:
                st["stats"][s]["hands"] += 1
        deal_order = [s for s in self._ordered(button) if s in funded]
        if len(funded) == 2:
            deal_order = [button, deal_order[0]]
        for _ in range(2):
            for s in deal_order:
                st["players"][s]["hole"].append(st["deck"].pop())
        sb = button if len(funded) == 2 else deal_order[0]
        bb = next(s for s in self._ordered(sb) if s in funded)
        st["small_blind_seat"], st["big_blind_seat"] = sb, bb
        for s, key in [(sb, "small_blind"), (bb, "big_blind")]:
            paid = self._pay(s, st["config"][key])
            st["history"].append({"seat": s, "phase": "preflop", "action": key, "paid": paid})
        notify("*", f"Hand {st['hand_number']}: button {button}, blinds {sb}/{bb}.", wake=False)
        self._advance(bb)

    def _legal(self, seat):
        st = self.state
        if st["phase"] == "finished" or st["turn"] != seat:
            return {}
        p = st["players"][seat]
        owed = max(0, st["current_bet"] - p["street_paid"])
        maximum = p["street_paid"] + p["stack"]
        reopened = not p["acted"] or st["current_bet"] - p["last_faced"] >= st["last_full_raise"]
        opposition = any(st["players"][s]["stack"] > 0 for s in self._live() if s != seat)
        may_raise = reopened and opposition and maximum > st["current_bet"]
        legal = {"fold": True, "check": owed == 0, "call": min(owed, p["stack"]) if owed else False,
                 "all_in": maximum <= st["current_bet"] or may_raise}
        if may_raise:
            minimum = st["current_bet"] + st["last_full_raise"]
            legal["raise_to"] = {"minimum": min(minimum, maximum), "maximum": maximum,
                                 "short_all_in_only": maximum < minimum}
        return legal

    def observe(self, seat):
        st = self.state
        if seat not in st["players"]:
            raise ValueError("Unknown seat")
        public_players = []
        for s, p in st["players"].items():
            public_players.append({"seat": s, **{k: p[k] for k in
                ("stack", "folded", "all_in", "street_paid", "total_paid")}})
        return deepcopy({"hand_number": st["hand_number"], "hands_limit": st["config"]["hands"],
            "phase": st["phase"], "turn": st["turn"], "button": st["seats"][st["button"]],
            "small_blind_seat": st.get("small_blind_seat"), "big_blind_seat": st.get("big_blind_seat"),
            "board": st["board"], "hole_cards": st["players"][seat]["hole"],
            "players": public_players, "pot": sum(p["total_paid"] for p in st["players"].values()),
            "current_bet": st["current_bet"], "last_full_raise": st["last_full_raise"],
            "legal_actions": self._legal(seat), "history": st["history"][-80:],
            "earlier_actions_omitted": max(0, len(st["history"]) - 80),
            "recent_results": [{k: r[k] for k in ("hand_number", "board", "showdown", "pots",
                                "refunds", "chip_changes", "stacks")} for r in st["results"][-5:]],
            "opponent_statistics": st["stats"],
            "ranking": self._ranking() if st["phase"] == "finished" else []})

    def agent_view(self, seat):
        view = self.observe(seat)
        while len(json.dumps(view).encode("utf-8")) > 30 * 1024:
            if view["recent_results"]:
                view["recent_results"].pop(0)
            else:
                view["history"].pop(0)
                view["earlier_actions_omitted"] += 1
        return view

    def handle_input(self, seat, text):
        raise ValueError("Table talk is disabled. Use a poker action instead.")

    def invoke(self, seat, action, arguments):
        st = self.state
        legal = self._legal(seat)
        if not legal:
            raise ValueError("It is not your turn, or the session is finished")
        if action not in ("fold", "check", "call", "all_in", "raise_to"):
            raise ValueError("Unknown poker action")
        if not isinstance(arguments, dict) or set(arguments) != ({"amount"} if action == "raise_to" else set()):
            raise ValueError("Unexpected action arguments")
        if not legal.get(action):
            raise ValueError("Action is not legal; inspect legal_actions")
        p = st["players"][seat]
        target = None
        if action == "raise_to":
            target = arguments["amount"]
            bounds = legal["raise_to"]
            if isinstance(target, bool) or not isinstance(target, int) or not bounds["minimum"] <= target <= bounds["maximum"]:
                raise ValueError("Raise total is outside the legal bounds")
        elif action == "all_in":
            target = p["street_paid"] + p["stack"]
        old_bet = st["current_bet"]
        paid = 0
        if action == "fold":
            p["folded"] = True
            st["stats"][seat]["folds"] += 1
        elif action == "check":
            st["stats"][seat]["checks"] += 1
        elif action == "call":
            paid = self._pay(seat, legal["call"])
            st["stats"][seat]["calls"] += 1
        else:
            paid = self._pay(seat, target - p["street_paid"])
            if target > old_bet:
                increase = target - old_bet
                if increase >= st["last_full_raise"]:
                    st["last_full_raise"] = increase
                st["current_bet"] = target
                st["stats"][seat]["raises"] += 1
            else:
                st["stats"][seat]["calls"] += 1
        p["acted"], p["last_faced"] = True, st["current_bet"]
        event = {"seat": seat, "phase": st["phase"], "action": action, "paid": paid,
                 "street_total": p["street_paid"]}
        st["history"].append(event)
        notify("*", f"{seat}: {action}" + (f" to {p['street_paid']}" if target is not None else f" ({paid} chips)"), wake=False)
        st["revision"] += 1
        self._advance(seat)
        self._attention()
        return {"accepted": True, "action": event}

    def _advance(self, after):
        st = self.state
        while True:
            live = self._live()
            if len(live) == 1:
                self._settle(False)
                return
            can_act = [s for s in live if st["players"][s]["stack"] > 0]
            if len(can_act) <= 1:
                st["current_bet"] = max(st["players"][s]["street_paid"] for s in live)
            pending = [s for s in can_act if not st["players"][s]["acted"] or
                       st["players"][s]["street_paid"] < st["current_bet"]]
            if len(can_act) == 1 and st["players"][can_act[0]]["street_paid"] >= st["current_bet"]:
                pending = []
            if pending:
                st["turn"] = next(s for s in self._ordered(after) if s in pending)
                return
            if st["phase"] == "river":
                self._settle(True)
                return
            st["deck"].pop()  # Burn before each community-card street.
            count = 3 if st["phase"] == "preflop" else 1
            st["board"].extend(st["deck"].pop() for _ in range(count))
            st["phase"] = {"preflop": "flop", "flop": "turn", "turn": "river"}[st["phase"]]
            st["current_bet"], st["last_full_raise"] = 0, st["config"]["big_blind"]
            for p in st["players"].values():
                p.update(street_paid=0, acted=False, last_faced=0)
            after = st["seats"][st["button"]]
            notify("*", f"{st['phase']}: {' '.join(st['board'])}", wake=False)

    def _settle(self, showdown):
        st = self.state
        players = st["players"]
        contributions = sorted((p["total_paid"], s) for s, p in players.items())
        refunds = {}
        if contributions[-1][0] > contributions[-2][0]:
            top, s = contributions[-1]
            refund = top - contributions[-2][0]
            players[s]["stack"] += refund
            players[s]["total_paid"] -= refund
            refunds[s] = refund
        live = self._live()
        ranks = {s: rank_hand(players[s]["hole"] + st["board"]) for s in live} if showdown else {}
        revealed = {s: {"cards": players[s]["hole"], "hand": hand_name(ranks[s])} for s in live} if showdown else {}
        pots, previous, winners_seen = [], 0, set()
        levels = sorted({p["total_paid"] for p in players.values() if p["total_paid"]})
        order = self._ordered(st["seats"][st["button"]])
        for level in levels:
            contributors = [s for s, p in players.items() if p["total_paid"] >= level]
            amount = (level - previous) * len(contributors)
            previous = level
            eligible = [s for s in live if players[s]["total_paid"] >= level]
            if showdown:
                best = max(ranks[s] for s in eligible)
                winners = [s for s in order if s in eligible and ranks[s] == best]
            else:
                winners = live
            share, odd = divmod(amount, len(winners))
            awards = {s: share + (i < odd) for i, s in enumerate(winners)}
            for s, award in awards.items():
                players[s]["stack"] += award
            winners_seen.update(winners)
            pots.append({"amount": amount, "eligible": eligible, "winners": winners, "awards": awards})
        for s in winners_seen:
            st["stats"][s]["pots_won"] += 1
        result = {"hand_number": st["hand_number"], "board": deepcopy(st["board"]),
            "showdown": deepcopy(revealed), "pots": pots, "refunds": refunds,
            "chip_changes": {s: p["stack"] - st["hand_start_stacks"][s] for s, p in players.items()},
            "stacks": {s: p["stack"] for s, p in players.items()}, "history": deepcopy(st["history"])}
        st["results"].append(result)
        notify("*", json.dumps({k: result[k] for k in ("hand_number", "board", "showdown", "pots", "refunds", "stacks")}), wake=False)
        for p in players.values():
            p["street_paid"], p["total_paid"] = 0, 0
            p["all_in"] = p["stack"] == 0
        st["turn"] = None
        self._start_hand()

    def _ranking(self):
        stacks = {s: p["stack"] for s, p in self.state["players"].items()}
        return [{"seat": s, "chips": stacks[s], "rank": 1 + sum(v > stacks[s] for v in stacks.values())}
                for s in sorted(stacks, key=lambda s: -stacks[s])]

    def _attention(self):
        st = self.state
        if st["phase"] == "finished":
            progress(st["revision"], waiting_for=[])
            finish("Session complete. Final ranking: " + json.dumps(self._ranking()))
        else:
            progress(st["revision"], waiting_for=[st["turn"]])
            notify([st["turn"]], "Your poker turn. Choose one action from legal_actions.", actionable=True)

    def snapshot(self):
        return deepcopy(self.state)

    def restore(self, snapshot):
        self.state = deepcopy(snapshot)
        return {}

    def dispose(self):
        return {}


environment = Holdem()
