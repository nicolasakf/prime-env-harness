from sdk import notify, progress
class Counter:
    def __init__(self):
        self.value = 0

    def initialize(self, params):
        self.value = 0
        self.seats = [seat["id"] for seat in params.get("seats", [{"id": "human"}, {"id": "agent"}])]
        return {}

    def describe_actions(self):
        return [{"name": "increment", "description": "Increment the shared counter", "input_schema": {"type": "object", "properties": {"amount": {"type": "integer", "minimum": 1}}, "required": ["amount"], "additionalProperties": False}, "output_schema": {"type": "integer"}, "roles": ["human", "agent"]}]

    def observe(self, participant):
        return {"value": self.value}

    def handle_input(self, participant, text):
        notify([seat for seat in self.seats if seat != participant], text)
        return {"observation": self.observe(participant)}

    def invoke(self, participant, action, arguments):
        if action != "increment" or arguments["amount"] < 1:
            raise ValueError("illegal action")
        self.value += arguments["amount"]
        notify([seat for seat in self.seats if seat != participant], "Counter changed")
        progress(self.value)
        return self.value

    def snapshot(self):
        return {"value": self.value, "seats": self.seats}

    def restore(self, snapshot):
        self.value = snapshot["value"]
        self.seats = snapshot["seats"]
        return {}

    def dispose(self):
        return {}


environment = Counter()
