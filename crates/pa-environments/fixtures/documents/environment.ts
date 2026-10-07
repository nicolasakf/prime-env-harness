import type { Action, Environment, Json, Participant } from "./sdk.ts";

interface Document { id: string; text: string; words: number; category: string | null }
let documents: Document[] = [];
const environment: Environment = {
    initialize() { documents = []; return {}; },
    describe_actions(): Action[] { return [
        { name: "submit_text", description: "Add a document for processing", participants: ["human"], input_schema: { type: "object", properties: { text: { type: "string", minLength: 1 } }, required: ["text"], additionalProperties: false }, output_schema: { type: "string" } },
        { name: "categorize", description: "Categorize a submitted document", participants: ["agent"], input_schema: { type: "object", properties: { id: { type: "string" }, category: { type: "string", enum: ["research", "business", "other"] } }, required: ["id", "category"], additionalProperties: false }, output_schema: { type: "integer", minimum: 0 } },
    ]; },
    observe(participant: Participant) { return { documents: documents.map(document => participant === "agent" ? { ...document } : { id: document.id, words: document.words, category: document.category }) } as Json; },
    handle_input(participant: Participant, text: string) { return { observation: this.observe(participant) as Json, agent_message: text }; },
    invoke(participant: Participant, action: string, args: Json) {
        if (action === "submit_text" && participant === "human") {
            const { text } = args as { text: string };
            const id = `document-${documents.length + 1}`;
            documents.push({ id, text, words: text.trim().split(/\s+/u).length, category: null });
            return id;
        }
        if (action === "categorize" && participant === "agent") {
            const { id, category } = args as { id: string; category: string };
            const document = documents.find(document => document.id === id);
            if (!document || !["research", "business", "other"].includes(category)) throw new Error("unknown document or category");
            document.category = category;
            return document.words;
        }
        throw new Error("action is unavailable to this participant");
    },
    snapshot() { return JSON.parse(JSON.stringify({ documents })) as Json; },
    restore(snapshot: Json) { documents = (snapshot as unknown as { documents: Document[] }).documents; return {}; },
    dispose() { return {}; },
};
export default environment;
