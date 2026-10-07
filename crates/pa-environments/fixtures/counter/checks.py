from environment import Counter

counter = Counter()
counter.initialize({})
assert counter.invoke("agent", "increment", {"amount": 2}) == 2
state = counter.snapshot()
counter.initialize({})
counter.restore(state)
assert counter.observe("human") == {"value": 2}
