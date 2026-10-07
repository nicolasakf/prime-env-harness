import assert from "node:assert/strict";
import environment from "./environment.ts";

await environment.initialize({});
const id = await environment.invoke("human", "submit_text", { text: "A research document" });
assert.equal(id, "document-1");
assert.equal(await environment.invoke("agent", "categorize", { id, category: "research" }), 3);
assert.throws(() => environment.invoke("agent", "submit_text", { text: "unauthorized" }));
const snapshot = await environment.snapshot();
await environment.initialize({});
await environment.restore(snapshot);
assert.deepEqual(await environment.observe("human"), { documents: [{ id: "document-1", words: 3, category: "research" }] });
