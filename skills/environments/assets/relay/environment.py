from sdk import finish, notify, progress


class Relay:
    def initialize(self, params):
        self.seats = [seat["id"] for seat in params["seats"]]
        self.rounds = params["config"]["rounds"]
        self.turn = 0
        notify([self.seats[0]], "Your turn")
        progress(0, waiting_for=[self.seats[0]])
        return {}

    def describe_actions(self):
        return [{
            "name": "pass",
            "description": "Pass the baton on your turn",
            "input_schema": {"type": "object", "additionalProperties": False},
            "output_schema": {"type": "integer"},
            "roles": ["player"],
        }]

    def observe(self, seat):
        if seat not in self.seats:
            raise ValueError("unknown seat")
        done = self.turn == self.rounds * len(self.seats)
        return {
            "you": seat,
            "turn": self.turn,
            "next": None if done else self.seats[self.turn % len(self.seats)],
            "finished": done,
        }

    def handle_input(self, seat, text):
        observation = self.observe(seat)
        notify("*", f"{seat}: {text}", wake=False)
        return {"observation": observation}

    def invoke(self, seat, action, arguments):
        if (action != "pass" or arguments != {}
                or self.turn >= self.rounds * len(self.seats)
                or seat != self.seats[self.turn % len(self.seats)]):
            raise ValueError("illegal pass")
        self.turn += 1
        notify("*", f"{seat} passed the baton", wake=False)
        if self.turn == self.rounds * len(self.seats):
            progress(self.turn)
            finish("Relay completed")
        else:
            next_seat = self.seats[self.turn % len(self.seats)]
            notify([next_seat], "Your turn")
            progress(self.turn, waiting_for=[next_seat])
        return self.turn

    def snapshot(self):
        return {"seats": list(self.seats), "turn": self.turn, "rounds": self.rounds}

    def restore(self, snapshot):
        self.seats = list(snapshot["seats"])
        self.turn = snapshot["turn"]
        self.rounds = snapshot["rounds"]
        return {}

    def dispose(self):
        return {}


environment = Relay()
