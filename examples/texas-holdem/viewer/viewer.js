// Hold'em viewer. A replay reads the full snapshot; a live page reads frame(),
// the same shape with undealt decks removed and other seats' hole cards masked
// as "??". Both depend on this environment's state shape, so viewer.json lists
// the definition versions it was checked against.
(() => {
  "use strict";
  const SUITS = { c: "♣", d: "♦", h: "♥", s: "♠" };
  const COLORS = ["#4c78a8", "#f58518", "#54a24b", "#e45756", "#72b7b2", "#b279a2", "#ff9da6", "#9d755d", "#bab0ac"];
  const VERBS = { fold: "folds", check: "checks", call: "calls", all_in: "goes all-in", raise_to: "raises to" };
  const $ = (id) => document.getElementById(id);
  const models = Object.fromEntries(env.meta.seats.map((seat) => [seat.id, seat.model || seat.occupant || ""]));

  const el = (tag, attributes = {}, ...children) => {
    const node = document.createElement(tag);
    for (const [key, value] of Object.entries(attributes)) {
      if (value === false || value == null) continue;
      if (key === "class") node.className = value;
      else if (key === "style") node.style.cssText = value;
      else node.setAttribute(key, value);
    }
    for (const child of children.flat()) {
      if (child != null && child !== false) node.append(child instanceof Node ? child : String(child));
    }
    return node;
  };

  const card = (code, extra = "") => {
    if (code === "??") return el("div", { class: `card back ${extra}`, title: "hidden" });
    const rank = code[0] === "T" ? "10" : code[0];
    const suit = code[1];
    const color = suit === "h" || suit === "d" ? "red" : "black";
    return el("div", { class: `card ${color} ${extra}`, title: code }, rank + SUITS[suit]);
  };
  const emptyCard = () => el("div", { class: "card empty" });

  const pot = (state) => Object.values(state.players).reduce((sum, player) => sum + player.total_paid, 0);
  const position = (index, count, rx, ry) => {
    const angle = Math.PI / 2 + (2 * Math.PI * index) / count;
    return { left: 50 + rx * Math.cos(angle), top: 50 + ry * Math.sin(angle) };
  };

  // Chapter marks: the first step of every hand, extended as live steps arrive.
  const marks = [];
  let scanned = 0;
  let lastHand = 0;
  const extendMarks = () => {
    const before = marks.length;
    for (; scanned < env.length; scanned += 1) {
      const hand = env.frame(scanned).state.hand_number;
      if (hand !== lastHand) marks.push({ index: scanned, label: `Hand ${hand}` });
      lastHand = hand;
    }
    if (marks.length !== before) env.setMarks(marks);
  };

  // A live page receives other seats' operations only through the frame, so
  // the move is then read from the hand history the step appended.
  const describe = (frame) => {
    const op = frame.op;
    if (!op) {
      const results = frame.state.results;
      const settled = frame.previous && results.length > frame.previous.results.length;
      // A move that ends a hand is the last event of that hand's result.
      const history = settled ? results[results.length - 1].history : frame.state.history;
      const event = history[history.length - 1];
      const grew = settled || (frame.previous && event && history.length > frame.previous.history.length);
      if (!grew || event.action === "small_blind" || event.action === "big_blind") return ["Cards are dealt."];
      const detail = event.action === "raise_to" || event.action === "all_in" ? ` ${event.street_total}`
        : event.paid ? ` (${event.paid})` : "";
      return [el("strong", {}, event.seat), ` ${VERBS[event.action] || event.action}${detail}${event.automatic ? " automatically" : ""}`];
    }
    if (op.method === "initialize") return ["Cards are dealt."];
    if (op.method === "missed_turn") {
      return [el("strong", {}, op.seat), op.result && op.result.auto_folded
        ? " did not act in time and folds automatically" : " missed the turn and was warned"];
    }
    if (op.method === "invoke") {
      const amount = op.arguments && op.arguments.amount != null ? ` ${op.arguments.amount}` : "";
      if (!op.success) {
        return [el("strong", {}, op.seat), ` tried ${op.action}${amount}: `,
          el("span", { class: "rejected" }, `rejected (${String(op.result)})`)];
      }
      const event = (op.result && op.result.action) || {};
      const detail = op.action === "raise_to" || op.action === "all_in" ? ` ${event.street_total}`
        : event.paid ? ` (${event.paid})` : "";
      return [el("strong", {}, op.seat), ` ${VERBS[op.action] || op.action}${detail}`];
    }
    if (op.method === "handle_input") return [el("strong", {}, op.seat || "player"), " spoke (rejected: no table talk)"];
    return [op.method || ""];
  };

  const renderSeats = (frame, state) => {
    const container = $("seats");
    container.replaceChildren();
    const count = state.seats.length;
    const button = state.seats[state.button];
    const compact = window.matchMedia("(max-width: 860px)").matches;
    state.seats.forEach((seat, index) => {
      const player = state.players[seat];
      const out = player.folded && player.hole.length === 0;
      const spot = position(index, count, compact ? 34 : 44, compact ? 38 : 41);
      const acted = frame.op && frame.op.seat === seat && frame.op.method === "invoke" && frame.op.success;
      const badges = el("div", { class: "badges" },
        seat === button && el("span", { class: "badge button", title: "Dealer button" }, "D"),
        seat === state.small_blind_seat && el("span", { class: "badge" }, "SB"),
        seat === state.big_blind_seat && el("span", { class: "badge" }, "BB"));
      container.append(el("div", {
        class: ["seat", state.turn === seat && "turn", player.folded && "folded", out && "out", acted && "acted"]
          .filter(Boolean).join(" "),
        style: `left:${spot.left}%;top:${spot.top}%`,
      }, badges,
      el("div", { class: "name" }, seat),
      el("div", { class: "model", title: models[seat] }, models[seat]),
      el("div", { class: "hole" }, player.hole.map((code) => card(code, "small"))),
      el("div", { class: "stack" }, out ? "out" : `${player.stack} chips${player.all_in ? " · all-in" : ""}`)));
      if (player.street_paid > 0) {
        const chips = position(index, count, compact ? 18 : 27, compact ? 22 : 24);
        container.append(el("div", { class: "bet", style: `left:${chips.left}%;top:${chips.top}%` }, player.street_paid));
      }
    });
  };

  const renderBoard = (frame, state) => {
    const previous = frame.previous;
    const sameHand = previous && previous.hand_number === state.hand_number;
    const seen = new Set(sameHand ? previous.board : []);
    const cards = state.board.map((code) => card(code, seen.has(code) ? "" : "fresh"));
    while (cards.length < 5) cards.push(emptyCard());
    $("board").replaceChildren(...cards);
    $("street").textContent = state.phase === "finished" ? "Session complete" : `Hand ${state.hand_number} · ${state.phase}`;
    $("pot").textContent = state.phase === "finished" ? "" : `Pot ${pot(state)}`;
  };

  // A hand settles and the next one is dealt in the same operation, so the
  // showdown is read from the result the operation appended.
  const renderResult = (frame, state) => {
    const box = $("result");
    const previous = frame.previous;
    if (!previous || state.results.length <= previous.results.length) {
      box.hidden = true;
      return null;
    }
    const result = state.results[state.results.length - 1];
    const winners = new Set(result.pots.flatMap((p) => p.winners));
    const rows = Object.entries(result.chip_changes).filter(([seat, change]) => change !== 0 || winners.has(seat));
    const showdown = result.showdown || {};
    box.replaceChildren(
      el("h3", {}, `Hand ${result.hand_number} result${Object.keys(showdown).length ? " · showdown" : ""}`),
      el("div", { class: "board" }, result.board.map((code) => card(code, "small"))),
      ...rows.map(([seat, change]) => el("div", { class: "row" },
        el("span", { class: "who" }, seat),
        el("span", { class: "hole" }, (showdown[seat] ? showdown[seat].cards : []).map((code) => card(code, "small"))),
        el("span", { class: "what" }, showdown[seat] ? showdown[seat].hand : winners.has(seat) ? "uncontested" : ""),
        el("span", { class: `gain ${change > 0 ? "up" : "down"}` }, change > 0 ? `+${change}` : change))));
    box.hidden = false;
    return result;
  };

  const renderLog = (state, settled) => {
    const history = settled ? settled.history : state.history;
    const items = history.map((event, index) => el("li", { class: index === history.length - 1 ? "latest" : "" },
      el("span", {}, el("span", { class: "street-tag" }, event.phase), " ", el("strong", {}, event.seat), " ",
        (VERBS[event.action] || event.action.replace("_", " ")) + (event.automatic ? " (auto)" : "")),
      el("span", {}, event.street_total != null && (event.action === "raise_to" || event.action === "all_in")
        ? event.street_total : event.paid || "")));
    const log = $("log");
    log.replaceChildren(...items);
    log.scrollTop = log.scrollHeight;
  };

  const renderChart = (state) => {
    const start = state.config.starting_chips;
    const series = state.seats.map((seat) => [start, ...state.results.map((r) => r.stacks[seat])]);
    const hands = Math.max(1, state.config.hands);
    const top = Math.max(...series.flat()) * 1.15;
    const width = 300;
    const height = 140;
    const pad = 22;
    const x = (i) => pad + ((width - pad - 6) * i) / hands;
    const y = (v) => height - pad - ((height - pad - 6) * v) / top;
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", `0 0 ${width} ${height}`);
    svg.setAttribute("role", "img");
    svg.setAttribute("aria-label", "Chip stacks after each hand");
    const add = (tag, attributes) => {
      const node = document.createElementNS("http://www.w3.org/2000/svg", tag);
      for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, value);
      svg.append(node);
      return node;
    };
    add("line", { class: "axis", x1: pad, x2: width - 6, y1: y(0), y2: y(0) });
    add("line", { class: "axis", x1: pad, x2: width - 6, y1: y(start), y2: y(start), "stroke-dasharray": "3 3" });
    add("text", { x: 2, y: y(start) + 3 }).textContent = String(start);
    add("text", { x: pad, y: height - 6 }).textContent = "hand 0";
    add("text", { x: width - 40, y: height - 6 }).textContent = `hand ${hands}`;
    series.forEach((points, i) => {
      add("polyline", { points: points.map((v, h) => `${x(h)},${y(v)}`).join(" "), fill: "none",
        stroke: COLORS[i % COLORS.length], "stroke-width": 2, "stroke-linejoin": "round" });
    });
    const legend = el("div", { class: "legend" },
      state.seats.map((seat, i) => el("span", { style: `--swatch:${COLORS[i % COLORS.length]}` }, seat)));
    $("chart").replaceChildren(svg, legend);
  };

  const renderStats = (state) => {
    const start = state.config.starting_chips;
    const head = el("tr", {}, ["Player", "Stack", "Net", "Pots", "Raises", "Folds"].map((h) => el("th", {}, h)));
    const rows = state.seats.map((seat) => {
      const stats = state.stats[seat];
      const stack = state.players[seat].stack;
      const net = stack + state.players[seat].total_paid - start;
      return el("tr", {}, el("td", { title: seat }, seat), el("td", {}, stack),
        el("td", { class: `gain ${net > 0 ? "up" : net < 0 ? "down" : ""}` }, net > 0 ? `+${net}` : net),
        el("td", {}, stats.pots_won), el("td", {}, stats.raises), el("td", {}, stats.folds));
    });
    $("stats").replaceChildren(el("thead", {}, head), el("tbody", {}, rows));
  };

  env.onFrame((frame) => {
    extendMarks();
    const state = frame.state;
    $("title").textContent = env.meta.name;
    $("subtitle").textContent =
      `Blinds ${state.config.small_blind}/${state.config.big_blind} · ${state.config.hands} hands · ` +
      `${state.config.starting_chips} chips each`;
    renderBoard(frame, state);
    renderSeats(frame, state);
    const settled = renderResult(frame, state);
    $("move").replaceChildren(...describe(frame), frame.op && frame.op.finished ? " · session complete" : "");
    renderLog(state, settled);
    renderChart(state);
    renderStats(state);
  });
})();
