import { notify, progress, type Environment, type Json, type Participant } from "./sdk.ts";
let value = 0;
let seats: string[] = [];
const environment: Environment = {
    initialize(params: Json) { value = 0; seats = ((params as { seats?: { id: string }[] }).seats ?? [{ id: "human" }, { id: "agent" }]).map(seat => seat.id); return {}; },
    describe_actions() { return [{ name: "increment", description: "Increment the shared counter", input_schema: { type: "object", properties: { amount: { type: "integer", minimum: 1 } }, required: ["amount"], additionalProperties: false }, output_schema: { type: "integer" }, roles: ["human", "agent"] }]; },
    observe() { return { value }; },
    handle_input(participant: Participant, text: string) { notify(seats.filter(seat => seat !== participant), text); return { observation: { value } }; },
    invoke(participant: Participant, action: string, args: Json) {
        const amount = (args as { amount: number }).amount;
        if (action !== "increment" || amount < 1) throw new Error("illegal action");
        value += amount;
        notify(seats.filter(seat => seat !== participant), "Counter changed");
        progress(value);
        return value;
    },
    snapshot() { return { value, seats }; },
    restore(snapshot: Json) { ({ value, seats } = snapshot as { value: number, seats: string[] }); return {}; },
    dispose() { return {}; },
};
export default environment;
