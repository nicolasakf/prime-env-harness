"""Small card fixture: legality and private hands belong to the environment."""
from copy import deepcopy
from sdk import notify


class Cards:
    def initialize(self, params):
        self.state = {"hands": {"human": ["seven_of_hearts"], "agent": ["ace_of_spades"]}, "played": ["five_of_hearts"]}
        return {}

    def describe_actions(self):
        return [{"name": "play_card", "description": "Play an owned card matching the top card's suit or rank", "input_schema": {"type": "object", "properties": {"card_id": {"type": "string"}}, "required": ["card_id"], "additionalProperties": False}, "output_schema": {"type": "object", "properties": {"played": {"type": "string"}}, "required": ["played"], "additionalProperties": False}, "participants": ["human", "agent"]}]

    def observe(self, participant):
        opponent = "agent" if participant == "human" else "human"
        return {"hand": list(self.state["hands"][participant]), "opponent_count": len(self.state["hands"][opponent]), "played": list(self.state["played"])}

    def handle_input(self, participant, text):
        # Private notes remain human-only and never reach the model.
        return {"observation": self.observe(participant), "agent_message": None if text.startswith("private:") else text}

    def invoke(self, participant, action, arguments):
        card = arguments["card_id"]
        if action != "play_card" or card not in self.state["hands"][participant]:
            raise ValueError("card is not owned by this participant")
        rank, suit = card.split("_of_")
        previous_rank, previous_suit = self.state["played"][-1].split("_of_")
        if rank != previous_rank and suit != previous_suit:
            raise ValueError("card does not match the top card")
        self.state["hands"][participant].remove(card)
        self.state["played"].append(card)
        notify("*", "A card was played", wake=False)
        notify(["agent" if participant == "human" else "human"], "Your turn")
        return {"played": card}

    def snapshot(self):
        return deepcopy(self.state)

    def restore(self, snapshot):
        self.state = deepcopy(snapshot)
        return {}

    def dispose(self):
        return {}


environment = Cards()
