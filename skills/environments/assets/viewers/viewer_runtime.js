// Environment viewer runtime: the `env` bridge and the playback shell.
// Injected ahead of the viewer's own code, by viewer_tool.py into a replay
// export or by the host's live-view server into a live page. It knows steps,
// sequences and playback; it never knows anything about a particular game.
(() => {
  "use strict";
  const replay = document.getElementById("env-replay-data");
  const data = JSON.parse((replay || document.getElementById("env-live-config")).textContent);
  const live = !replay;
  const steps = live ? [] : data.steps;
  const states = [];
  const listeners = [];
  const showShell = data.meta.viewer.shell !== false;
  const speeds = [1, 2, 4, 8, 16];
  let index = 0;
  let playing = false;
  let timer = null;
  let speed = 2;
  let marks = [];
  let ready = false;
  let shell = null;
  // Live pages follow the newest step until the viewer moves back in time.
  let following = live;
  let connection = live ? "connecting" : null;

  const deepFreeze = (value) => {
    if (value && typeof value === "object" && !Object.isFrozen(value)) {
      Object.freeze(value);
      for (const child of Object.values(value)) deepFreeze(child);
    }
    return value;
  };

  const setField = (target, key, value) =>
    Object.defineProperty(target, key, { value, writable: true, enumerable: true, configurable: true });

  // Copy-on-write patching: each state shares every unchanged subtree with the one before it.
  const applyPatch = (base, patch) => {
    let root = base;
    const copied = new Set();
    const own = (container) => {
      if (copied.has(container)) return container;
      const copy = Array.isArray(container) ? container.slice() : { ...container };
      copied.add(copy);
      return copy;
    };
    for (const op of patch) {
      if (op.path === "") {
        root = deepFreeze(op.value);
        continue;
      }
      const tokens = op.path.slice(1).split("/").map((t) => t.replace(/~1/g, "/").replace(/~0/g, "~"));
      const last = tokens.pop();
      root = own(root);
      let parent = root;
      for (const token of tokens) {
        const key = Array.isArray(parent) ? Number(token) : token;
        const child = own(parent[key]);
        setField(parent, key, child);
        parent = child;
      }
      const value = op.op === "remove" ? undefined : deepFreeze(op.value);
      if (Array.isArray(parent)) {
        const at = last === "-" ? parent.length : Number(last);
        if (op.op === "add") parent.splice(at, 0, value);
        else if (op.op === "replace") parent[at] = value;
        else parent.splice(at, 1);
      } else if (op.op === "remove") {
        delete parent[last];
      } else {
        setField(parent, last, value);
      }
    }
    for (const container of copied) Object.freeze(container);
    return root;
  };

  const stateAt = (i) => {
    let from = i;
    while (from > 0 && states[from] === undefined && !("state" in steps[from])) from -= 1;
    if (states[from] === undefined) states[from] = deepFreeze(steps[from].state);
    for (let at = from + 1; at <= i; at += 1) {
      states[at] = "state" in steps[at] ? deepFreeze(steps[at].state) : applyPatch(states[at - 1], steps[at].patch);
    }
    return states[i];
  };

  const clamp = (i) => Math.max(0, Math.min(steps.length - 1, Math.trunc(Number(i)) || 0));

  const frame = (i) => {
    const at = clamp(i);
    const step = steps[at];
    return Object.freeze({
      index: at,
      sequence: step.sequence,
      state: stateAt(at),
      previous: at > 0 ? stateAt(at - 1) : null,
      op: step.op,
      deliveries: step.deliveries,
      time: step.time,
      recorded: step.recorded,
      finished: step.finished || null,
      first: at === 0,
      last: at === steps.length - 1,
    });
  };

  const report = (error) => {
    console.error(error);
    if (shell) shell.error(String((error && error.message) || error));
  };

  const emit = () => {
    const current = frame(index);
    if (shell) shell.update(current);
    for (const listener of listeners) {
      try {
        listener(current);
      } catch (error) {
        report(error);
      }
    }
  };

  const seek = (i) => {
    const next = clamp(i);
    following = live && next === steps.length - 1;
    if (next === index && ready) {
      if (shell) shell.update(frame(index));
      return;
    }
    index = next;
    if (ready) emit();
  };

  const pause = () => {
    playing = false;
    clearInterval(timer);
    timer = null;
    if (shell) shell.update(frame(index));
  };

  const play = () => {
    if (live && index >= steps.length - 1) {
      seek(steps.length - 1);
      return;
    }
    if (index >= steps.length - 1) seek(0);
    playing = true;
    clearInterval(timer);
    timer = setInterval(() => {
      if (index >= steps.length - 1) pause();
      else seek(index + 1);
    }, 1000 / speed);
    if (shell) shell.update(frame(index));
  };

  const jumpMark = (direction) => {
    const ordered = direction > 0 ? marks : [...marks].reverse();
    const target = ordered.find((mark) => (direction > 0 ? mark.index > index : mark.index < index));
    if (target) seek(target.index);
  };

  const env = Object.freeze({
    meta: deepFreeze(data.meta),
    get length() {
      return steps.length;
    },
    get live() {
      return live;
    },
    frame,
    get index() {
      return index;
    },
    get playing() {
      return playing;
    },
    onFrame(listener) {
      listeners.push(listener);
      if (ready) {
        try {
          listener(frame(index));
        } catch (error) {
          report(error);
        }
      }
    },
    seek,
    step: (delta) => seek(index + delta),
    play,
    pause,
    toggle: () => (playing ? pause() : play()),
    setSpeed(stepsPerSecond) {
      speed = Math.max(0.25, Number(stepsPerSecond) || 2);
      if (playing) play();
    },
    // Chapter labels for the playback bar, such as rounds or phases: [{index, label}].
    setMarks(list) {
      marks = (list || [])
        .map((mark) => ({ index: clamp(mark.index), label: String(mark.label) }))
        .sort((a, b) => a.index - b.index);
      if (shell) shell.marks(marks);
    },
    nextMark: () => jumpMark(1),
    previousMark: () => jumpMark(-1),
  });
  Object.defineProperty(window, "env", { value: env, writable: false, configurable: false });

  const createShell = () => {
    const host = document.createElement("env-playback");
    host.style.cssText = "all:initial;position:fixed;left:0;right:0;bottom:0;z-index:2147483647;";
    const root = host.attachShadow({ mode: "closed" });
    root.innerHTML = `<style>
      :host { --bg: rgba(250,250,250,.96); --fg: #1d1d1f; --muted: #6e6e73; --line: #d2d2d7; --accent: #0a64d8; --bad: #c4321d; --live: #2b8a3e; }
      @media (prefers-color-scheme: dark) {
        :host { --bg: rgba(24,24,26,.96); --fg: #f2f2f2; --muted: #a1a1a6; --line: #3a3a3c; --accent: #5ea2ff; --bad: #ff7a66; --live: #69db7c; }
      }
      .bar { display: flex; align-items: center; gap: 8px; height: 44px; padding: 0 12px; box-sizing: border-box;
        background: var(--bg); color: var(--fg); border-top: 1px solid var(--line);
        font: 12px/1.2 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
      button, select { font: inherit; color: inherit; background: transparent; border: 1px solid var(--line);
        border-radius: 6px; height: 28px; min-width: 28px; padding: 0 6px; cursor: pointer; }
      button:hover, select:hover { border-color: var(--accent); }
      .track { position: relative; flex: 1; min-width: 80px; display: flex; align-items: center; }
      input[type=range] { width: 100%; accent-color: var(--accent); margin: 0; }
      .ticks { position: absolute; left: 0; right: 0; top: -6px; height: 6px; pointer-events: none; }
      .ticks i { position: absolute; width: 1px; height: 6px; background: var(--muted); }
      .where { color: var(--muted); white-space: nowrap; font-variant-numeric: tabular-nums; }
      .error { color: var(--bad); max-width: 30%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
      .error:empty { display: none; }
      .live { color: var(--muted); white-space: nowrap; }
      .live.on { color: var(--live); border-color: var(--live); }
      @media (max-width: 560px) { .speed, .marks, [data-do="start"], [data-do="end"] { display: none; } }
    </style>
    <div class="bar" role="toolbar" aria-label="Replay controls">
      <button data-do="start" title="First step (Home)">⏮</button>
      <button data-do="back" title="Previous step (←)">‹</button>
      <button data-do="toggle" title="Play or pause (Space)">▶</button>
      <button data-do="forward" title="Next step (→)">›</button>
      <button data-do="end" title="Last step (End)">⏭</button>
      <div class="track"><div class="ticks"></div><input type="range" min="0" step="1" aria-label="Step"></div>
      <span class="where"></span>
      <button class="live" data-do="live" title="Follow the newest step" hidden></button>
      <select class="marks" aria-label="Jump to" hidden></select>
      <select class="speed" aria-label="Speed" title="Steps per second"></select>
      <span class="error" role="alert"></span>
    </div>`;
    const $ = (selector) => root.querySelector(selector);
    const range = $("input");
    const where = $(".where");
    const toggle = $('[data-do="toggle"]');
    const speedSelect = $(".speed");
    const markSelect = $(".marks");
    const ticks = $(".ticks");
    const errorLine = $(".error");
    const liveButton = $(".live");
    range.max = String(steps.length - 1);
    speedSelect.innerHTML = speeds.map((s) => `<option value="${s}">${s}×</option>`).join("");
    speedSelect.value = String(speed);
    const drawTicks = () => {
      const span = Math.max(1, steps.length - 1);
      ticks.innerHTML = marks.map((mark) => `<i style="left:${(100 * mark.index) / span}%"></i>`).join("");
    };
    const showConnection = () => {
      liveButton.hidden = !live;
      const labels = { connecting: "Connecting…", reconnecting: "Reconnecting…", closed: "Disconnected", ended: "Ended" };
      liveButton.textContent = labels[connection] || (following ? "● Live" : "Go live");
      liveButton.classList.toggle("on", connection === "open" && following);
    };
    const actions = {
      start: () => seek(0),
      back: () => seek(index - 1),
      toggle: () => env.toggle(),
      forward: () => seek(index + 1),
      end: () => seek(steps.length - 1),
      live: () => seek(steps.length - 1),
    };
    root.addEventListener("click", (event) => {
      const button = event.target.closest("button");
      if (button) actions[button.dataset.do]();
    });
    range.addEventListener("input", () => {
      // pause() redraws the slider at the old step, so read the dragged value first.
      const target = Number(range.value);
      pause();
      seek(target);
    });
    speedSelect.addEventListener("change", () => env.setSpeed(Number(speedSelect.value)));
    markSelect.addEventListener("change", () => {
      if (markSelect.value !== "") seek(Number(markSelect.value));
    });
    window.addEventListener("keydown", (event) => {
      const target = event.target;
      if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return;
      if (target && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName))) return;
      const keys = {
        " ": () => env.toggle(),
        ArrowLeft: () => seek(index - 1),
        ArrowRight: () => seek(index + 1),
        Home: () => seek(0),
        End: () => seek(steps.length - 1),
        "[": () => env.previousMark(),
        "]": () => env.nextMark(),
      };
      if (keys[event.key]) {
        event.preventDefault();
        keys[event.key]();
      }
    });
    document.documentElement.style.setProperty("--env-shell-height", "44px");
    document.body.appendChild(host);
    return {
      update(current) {
        range.max = String(Math.max(0, steps.length - 1));
        range.value = String(current.index);
        drawTicks();
        showConnection();
        toggle.textContent = playing ? "⏸" : "▶";
        const label = [...marks].reverse().find((mark) => mark.index <= current.index);
        where.textContent = `${current.index + 1} / ${steps.length}${label ? ` · ${label.label}` : ""}`;
        markSelect.value = label ? String(label.index) : "";
      },
      marks(list) {
        markSelect.hidden = list.length === 0;
        markSelect.innerHTML =
          `<option value="" disabled>Jump to…</option>` +
          list.map((mark) => `<option value="${mark.index}"></option>`).join("");
        list.forEach((mark, i) => {
          markSelect.options[i + 1].textContent = mark.label;
        });
        if (steps.length) this.update(frame(index));
      },
      waiting() {
        where.textContent = "Waiting for the first step…";
        showConnection();
      },
      error(message) {
        errorLine.textContent = `Viewer error: ${message}`;
        errorLine.title = message;
      },
    };
  };

  window.addEventListener("error", (event) => report(event.error || event.message));
  window.addEventListener("unhandledrejection", (event) => report(event.reason));
  const refreshShell = () => {
    if (shell && ready) shell.update(frame(index));
    else if (shell) shell.waiting();
  };

  // Live pages stream steps from the host as server-sent events; the browser
  // reconnects on its own and resumes after the last step it received.
  const connect = () => {
    const source = new EventSource(data.events);
    source.onopen = () => {
      if (connection !== "ended") connection = "open";
      refreshShell();
    };
    source.onerror = () => {
      if (connection !== "ended") connection = source.readyState === EventSource.CLOSED ? "closed" : "reconnecting";
      refreshShell();
    };
    source.onmessage = (message) => {
      const step = JSON.parse(message.data);
      if (step.error) {
        report(new Error(step.error));
        return;
      }
      steps.push(step);
      if (step.finished) connection = "ended";
      if (!ready || following) {
        ready = true;
        index = steps.length - 1;
        emit();
      } else {
        refreshShell();
      }
    };
  };

  const start = () => {
    if (showShell) shell = createShell();
    else document.documentElement.style.setProperty("--env-shell-height", "0px");
    if (marks.length && shell) shell.marks(marks);
    if (live) connect();
    if (steps.length) {
      ready = true;
      emit();
    } else {
      refreshShell();
    }
  };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start);
  else start();
})();
