from sdk import notify, finish, progress

class Relay:
    def initialize(self, params):
        self.seats = [seat["id"] for seat in params["seats"]]
        self.turn = 0
        self.rounds = params["config"].get("rounds", 1)
        notify([self.seats[0]], "Your turn")
        progress(0, waiting_for=[self.seats[0]])
        return {}

    def describe_actions(self):
        return [{"name": "pass", "description": "Pass the baton on your turn", "input_schema": {"type": "object", "additionalProperties": False}, "output_schema": {"type": "integer"}, "roles": ["player"]}]

    def observe(self, seat):
        return {"turn": self.turn, "next": self.seats[self.turn % len(self.seats)], "you": seat}

    def handle_input(self, seat, text):
        notify("*", seat + ": " + text, wake=False)
        return {"observation": self.observe(seat)}

    def invoke(self, seat, action, arguments):
        if action != "pass" or seat != self.seats[self.turn % len(self.seats)]:
            raise ValueError("not your turn")
        self.turn += 1
        notify("*", "Baton passed", wake=False)
        if self.turn == self.rounds * len(self.seats):
            progress(self.turn)
            finish("Relay completed")
        else:
            next_seat = self.seats[self.turn % len(self.seats)]
            notify([next_seat], "Your turn")
            progress(self.turn, waiting_for=[next_seat])
        return self.turn

    def snapshot(self):
        return {"seats": self.seats, "turn": self.turn, "rounds": self.rounds}

    def restore(self, snapshot):
        self.seats = snapshot["seats"]
        self.turn = snapshot["turn"]
        self.rounds = snapshot["rounds"]
        return {}

    def dispose(self):
        return {}

environment = Relay()
