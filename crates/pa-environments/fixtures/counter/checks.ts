import assert from "node:assert/strict";
import environment from "./environment.ts";
await environment.initialize({});
assert.equal(await environment.invoke("agent", "increment", { amount: 2 }), 2);
const snapshot = await environment.snapshot();
await environment.initialize({});
await environment.restore(snapshot);
assert.deepEqual(await environment.observe("human"), { value: 2 });
