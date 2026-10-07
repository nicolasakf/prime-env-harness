from environment import Cards

environment = Cards()
environment.initialize({})
assert "ace_of_spades" not in str(environment.observe("human"))
assert "seven_of_hearts" not in str(environment.observe("agent"))
assert environment.handle_input("human", "private: hello")["agent_message"] is None
try:
    environment.invoke("agent", "play_card", {"card_id": "seven_of_hearts"})
    raise AssertionError("identity must determine ownership")
except ValueError:
    pass
try:
    environment.invoke("agent", "play_card", {"card_id": "ace_of_spades"})
    raise AssertionError("wrong suit must fail")
except ValueError:
    pass
assert environment.invoke("human", "play_card", {"card_id": "seven_of_hearts"}) == {"played": "seven_of_hearts"}
restored = Cards()
restored.restore(environment.snapshot())
assert restored.observe("human") == environment.observe("human")
