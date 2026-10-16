import { readFileSync } from "node:fs";
import { createInterface } from "node:readline";
import { randomUUID } from "node:crypto";
import { format } from "node:util";

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
export type SeatId = string;
export type Participant = SeatId;
export type RoleName = string;
export interface Action {
    name: string;
    description: string;
    input_schema: Json;
    output_schema: Json;
    participants?: RoleName[];
    roles?: RoleName[];
}
/** Human observation plus a private agent message (the UI shows a delivery notice).
 * Agent observations and tool responses are also private. Publish public information
 * in the human observation; keep private state out of action schemas.
 */
export interface MissedTurn { cause: "idle" | "deadline" | "suspended"; misses: number; limit: number }
export interface InputResult { observation: Json; agent_message?: string | null }
export interface Environment {
    initialize(params: Json): Json | Promise<Json>;
    describe_actions(): Action[] | Promise<Action[]>;
    observe(participant: Participant): Json | Promise<Json>;
    /**
     * Optional. With manifest `"agent_context": "stateless"` this is ALL an agent sees
     * each turn: current state plus a bounded history summary, filtered for the seat.
     * Defaults to `observe`. Keep it under 32 KiB.
     */
    agent_view?(participant: Participant): Json | Promise<Json>;
    /** Optional. What a live viewer draws for `{kind: "public"}` or `{kind: "seat", seat}`, filtered like `observe`. Without it the run has no live view. */
    frame?(perspective: { kind: "public" } | { kind: "seat"; seat: string }): Json | Promise<Json>;
    /**
     * Optional, with manifest `"missed_turns": true`. Called when an awaited agent seat's turn ends
     * without an action. Warn the seat with `notify(..., { wake: true })`, act for it, or do nothing;
     * without a wake from here the host re-prompts generically, and once `misses` exceeds `limit` it
     * stops the run unless this call resolved the seat's turn. Cause `"suspended"` means the
     * builder took the seat out of play: it is reported once per progress revision and never
     * re-prompted, so act for the seat to keep the run moving.
     */
    missed_turn?(participant: Participant, report: MissedTurn): Json | Promise<Json>;
    handle_input(participant: Participant, text: string): InputResult | Promise<InputResult>;
    invoke(participant: Participant, action: string, args: Json): Json | Promise<Json>;
    snapshot(): Json | Promise<Json>;
    restore(snapshot: Json): Json | Promise<Json>;
    dispose(): Json | Promise<Json>;
}

console.log = (...values: unknown[]) => process.stderr.write(`${format(...values)}\n`);
console.info = console.log;
const input = createInterface({ input: process.stdin });
const requests: unknown[] = [];
let requestReady: (() => void) | undefined;
let closed = false;
const pending = new Map<string, { resolve: (value: Json) => void; reject: (error: Error) => void }>();
input.on("line", (line) => {
    const frame = JSON.parse(line) as Record<string, unknown>;
    if (frame.kind === "host_response" && typeof frame.id === "string") {
        const call = pending.get(frame.id);
        if (!call) throw new Error("unknown host response");
        pending.delete(frame.id);
        if (frame.success) call.resolve(frame.result as Json);
        else call.reject(new Error(String(frame.error)));
    } else { requests.push(frame); requestReady?.(); }
});
input.on("close", () => { closed = true; requestReady?.(); });
function emit(value: unknown): void { process.stdout.write(`${JSON.stringify(value)}\n`); }
export async function host_call(method: string, params: Json): Promise<Json> {
    const id = randomUUID();
    const result = new Promise<Json>((resolve, reject) => pending.set(id, { resolve, reject }));
    emit({ kind: "host_call", id, method, params });
    return result;
}
export async function serve(environment: Environment): Promise<void> {
    const protocolVersion = (JSON.parse(readFileSync(new URL("./environment.json", import.meta.url), "utf8")) as { protocol_version: number }).protocol_version;
    if (![1, 2].includes(protocolVersion)) throw new Error("unsupported environment protocol");
    emit({ kind: "ready", protocol_version: protocolVersion });
    while (!closed || requests.length) {
        if (!requests.length) { await new Promise<void>((resolve) => { requestReady = resolve; }); requestReady = undefined; continue; }
        const request = requests.shift() as { id: string; protocol_version: number; method: string; participant: Participant; params: Record<string, Json> };
        noticeBuffer = []; finishBuffer = undefined; progressBuffer = undefined; requestMethod = request.method;
        try {
            if (request.protocol_version !== protocolVersion) throw new Error("unsupported environment protocol");
            let result: unknown;
            switch (request.method) {
                case "initialize": result = await environment.initialize(request.params); break;
                case "describe_actions": result = await environment.describe_actions(); break;
                case "observe": result = await environment.observe(request.participant); break;
                case "agent_view": result = await (environment.agent_view ?? environment.observe).call(environment, request.participant); break;
                case "frame": result = environment.frame ? await environment.frame.call(environment, request.params.perspective as never) : null; break;
                case "handle_input": result = await environment.handle_input(request.participant, String(request.params.text)); break;
                case "invoke": result = await environment.invoke(request.participant, String(request.params.action), request.params.arguments); break;
                case "missed_turn":
                    if (!environment.missed_turn) throw new Error("unknown environment method");
                    result = await environment.missed_turn(request.participant, request.params as unknown as MissedTurn); break;
                case "snapshot": result = await environment.snapshot(); break;
                case "restore": result = await environment.restore(request.params); break;
                case "dispose": result = await environment.dispose(); break;
                default: throw new Error("unknown environment method");
            }
            emit({ kind: "response", id: request.id, success: true, result, ...(noticeBuffer.length ? { notices: noticeBuffer } : {}), ...(finishBuffer ? { finished: finishBuffer } : {}), ...(progressBuffer ? { progress: progressBuffer } : {}) });
            if (request.method === "dispose") { input.close(); break; }
        } catch (error) { emit({ kind: "response", id: request.id, success: false, error: String(error) }); }
    }
}


interface Notice { to: "*" | string[]; message?: string; wake: boolean; actionable?: boolean }
let noticeBuffer: Notice[] = [];
let finishBuffer: { summary: string } | undefined;
let progressBuffer: { revision: number; waiting_for: string[] } | undefined;
let requestMethod: string | undefined;
function mutationMetadata(): void {
    if (requestMethod !== undefined && !["initialize", "invoke", "handle_input", "missed_turn"].includes(requestMethod)) throw new Error("notifications and completion require a committed operation");
}
/** `actionable` marks a wake as the task for the named seats to act on now: the host drops it from a wake that starts after the environment stopped awaiting the seat (`progress({waiting_for})`). Needs `wake` and named seats. */
export function notify(to: "*" | string[], message?: string | null, options: { wake?: boolean; actionable?: boolean } = {}): void {
    mutationMetadata();
    if (to !== "*" && (!Array.isArray(to) || to.length < 1 || to.length > 16 || to.some(seat => typeof seat !== "string" || !/^[A-Za-z0-9_-]{1,64}$/.test(seat)))) throw new Error("notify target must be '*' or 1–16 seat IDs");
    if (message !== undefined && message !== null && (typeof message !== "string" || Buffer.byteLength(message) > 16384)) throw new Error("notice message must be at most 16 KiB");
    const wake = options.wake ?? true;
    const actionable = options.actionable ?? false;
    if (typeof wake !== "boolean" || typeof actionable !== "boolean") throw new Error("wake and actionable must be booleans");
    if (actionable && (!wake || to === "*")) throw new Error("an actionable notice must wake named seats");
    if (noticeBuffer.length >= 64) throw new Error("at most 64 notices per operation");
    noticeBuffer.push({ to: to === "*" ? to : [...to], wake, ...(actionable ? { actionable: true } : {}), ...(message === undefined || message === null ? {} : { message }) });
}
export function finish(summary: string): void {
    mutationMetadata();
    if (typeof summary !== "string" || Buffer.byteLength(summary) > 16384) throw new Error("finish summary must be at most 16 KiB");
    finishBuffer = { summary };
}
export function progress(revision: number, options: { waiting_for?: string[] } = {}): void {
    mutationMetadata();
    const seats = options.waiting_for ?? [];
    if (!Number.isSafeInteger(revision) || revision < 0) throw new Error("revision must be a nonnegative safe integer");
    if (!Array.isArray(seats) || seats.some(seat => typeof seat !== "string" || !/^[A-Za-z0-9_-]{1,64}$/.test(seat))) throw new Error("waiting_for must be a list of seat IDs");
    progressBuffer = { revision, waiting_for: [...seats] };
}
