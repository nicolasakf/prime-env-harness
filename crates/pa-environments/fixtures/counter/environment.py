class Counter:
    def __init__(self):
        self.value = 0

    def initialize(self, params):
        self.value = 0
        return {}

    def describe_actions(self):
        return [{"name": "increment", "description": "Increment the shared counter", "input_schema": {"type": "object", "properties": {"amount": {"type": "integer", "minimum": 1}}, "required": ["amount"], "additionalProperties": False}, "output_schema": {"type": "integer"}, "participants": ["human", "agent"]}]

    def observe(self, participant):
        return {"value": self.value}

    def handle_input(self, participant, text):
        return {"observation": self.observe(participant), "agent_message": text}

    def invoke(self, participant, action, arguments):
        if action != "increment" or arguments["amount"] < 1:
            raise ValueError("illegal action")
        self.value += arguments["amount"]
        return self.value

    def snapshot(self):
        return {"value": self.value}

    def restore(self, snapshot):
        self.value = snapshot["value"]
        return {}

    def dispose(self):
        return {}


environment = Counter()
