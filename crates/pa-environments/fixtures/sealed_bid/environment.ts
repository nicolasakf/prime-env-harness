import { notify, finish, progress, type Environment, type Json } from "./sdk.ts";
let seats: string[] = [];
let bids: Record<string, number> = {};
const environment: Environment = {
    initialize(params: Json) {
        seats = (params as { seats: { id: string }[] }).seats.map(seat => seat.id);
        bids = {};
        notify("*", "Submit your sealed bid");
        progress(0, { waiting_for: seats });
        return {};
    },
    describe_actions() { return [{ name: "bid", description: "Submit a private bid once", input_schema: { type: "object", properties: { amount: { type: "integer", minimum: 0 } }, required: ["amount"], additionalProperties: false }, output_schema: { type: "object" }, roles: ["bidder"] }]; },
    observe(seat) { return { your_bid: bids[seat] ?? null, submitted: Object.keys(bids).length }; },
    handle_input(seat, text) { notify("*", seat + ": " + text, { wake: false }); return { observation: this.observe(seat) }; },
    invoke(seat, action, args) {
        if (action !== "bid" || seat in bids) throw new Error("bid already submitted or unknown action");
        bids[seat] = (args as { amount: number }).amount;
        const waiting = seats.filter(seat => !(seat in bids));
        progress(Object.keys(bids).length, { waiting_for: waiting });
        if (!waiting.length) {
            const winner = [...seats].sort((a, b) => bids[b] - bids[a] || a.localeCompare(b))[0];
            notify("*", "Winner: " + winner, { wake: false });
            finish("Bidding completed");
        }
        return {};
    },
    snapshot() { return { seats, bids }; },
    restore(snapshot: Json) { ({ seats, bids } = snapshot as { seats: string[], bids: Record<string, number> }); return {}; },
    dispose() { return {}; },
};
export default environment;
