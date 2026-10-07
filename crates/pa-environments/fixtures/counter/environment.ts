import type { Environment, Json, Participant } from "./sdk.ts";
let value = 0;
const environment: Environment = {
    initialize() { value = 0; return {}; },
    describe_actions() { return [{ name: "increment", description: "Increment the shared counter", input_schema: { type: "object", properties: { amount: { type: "integer", minimum: 1 } }, required: ["amount"], additionalProperties: false }, output_schema: { type: "integer" }, participants: ["human", "agent"] }]; },
    observe() { return { value }; },
    handle_input(participant: Participant, text: string) { return { observation: { value }, agent_message: text }; },
    invoke(participant: Participant, action: string, args: Json) {
        const amount = (args as { amount: number }).amount;
        if (action !== "increment" || amount < 1) throw new Error("illegal action");
        value += amount;
        return value;
    },
    snapshot() { return { value }; },
    restore(snapshot: Json) { value = (snapshot as { value: number }).value; return {}; },
    dispose() { return {}; },
};
export default environment;
