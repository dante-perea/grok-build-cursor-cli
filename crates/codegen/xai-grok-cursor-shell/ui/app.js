/* Cursor Agents Home client — dumb view over WS/REST control plane */
(() => {
  const state = {
    planMode: true,
    view: "home",
    ws: null,
  };

  const $ = (sel) => document.querySelector(sel);
  const homeView = $("#view-home");
  const sessionView = $("#view-session");
  const historyList = $("#history-list");
  const transcript = $("#transcript");
  const activityList = $("#activity-list");
  const diffList = $("#diff-list");
  const diffInspect = $("#diff-inspect");
  const inputHome = $("#composer-input");
  const inputSession = $("#composer-input-session");
  const planChip = $("#plan-chip");
  const modelChip = $("#model-chip");
  const ctxProject = $("#ctx-project");

  function setView(view) {
    state.view = view;
    if (view === "session") {
      homeView.classList.add("hidden");
      sessionView.classList.remove("hidden");
    } else {
      sessionView.classList.add("hidden");
      homeView.classList.remove("hidden");
    }
  }

  function renderHistory(items) {
    historyList.innerHTML = "";
    (items || []).forEach((s) => {
      const li = document.createElement("li");
      li.dataset.id = s.id;
      li.innerHTML = `<span>${escapeHtml(s.title)}</span><span class="meta">local</span>`;
      li.addEventListener("click", () => {
        // v1: show title as status; no transcript replay
        setView("session");
      });
      historyList.appendChild(li);
    });
  }

  function renderChat(messages) {
    transcript.innerHTML = "";
    (messages || []).forEach((m) => {
      const div = document.createElement("div");
      div.className = `msg ${m.role}`;
      const role =
        m.role === "user" ? "You" : m.role === "assistant" ? "Grok" : "System";
      div.innerHTML = `<div class="role">${role}${m.streaming ? " …" : ""}</div><div class="bubble"></div>`;
      div.querySelector(".bubble").textContent = m.content || "";
      transcript.appendChild(div);
    });
    transcript.scrollTop = transcript.scrollHeight;
  }

  function renderActivity(entries) {
    activityList.innerHTML = "";
    (entries || [])
      .slice()
      .reverse()
      .forEach((e) => {
        const li = document.createElement("li");
        li.textContent = `${statusIcon(e.status)} ${e.title}${e.tool_name ? " [" + e.tool_name + "]" : ""}`;
        activityList.appendChild(li);
      });
  }

  function renderDiffs(items) {
    diffList.innerHTML = "";
    (items || []).forEach((d, i) => {
      const li = document.createElement("li");
      const dec =
        d.decision === "accepted" ? "✓" : d.decision === "rejected" ? "✗" : "·";
      li.innerHTML = `${dec} ${escapeHtml(d.path)} <small>(${escapeHtml(d.summary)})</small>
        <button data-act="accept" data-id="${d.id}">a</button>
        <button data-act="reject" data-id="${d.id}">r</button>`;
      li.querySelector('[data-act="accept"]').onclick = () =>
        send({ type: "accept_diff", id: d.id });
      li.querySelector('[data-act="reject"]').onclick = () =>
        send({ type: "reject_diff", id: d.id });
      if (i === 0 && d.inspect_preview) {
        diffInspect.textContent = d.inspect_preview;
      }
      diffList.appendChild(li);
    });
    if (!items || !items.length) {
      diffInspect.textContent = "No proposed changes yet.";
    }
  }

  function statusIcon(s) {
    if (s === "completed") return "✓";
    if (s === "failed") return "✗";
    if (s === "running") return "●";
    return "○";
  }

  function applySnapshot(snap) {
    if (snap.view) setView(snap.view);
    if (snap.history) renderHistory(snap.history);
    if (snap.chat) renderChat(snap.chat);
    if (snap.activity) renderActivity(snap.activity);
    if (snap.diffs) renderDiffs(snap.diffs);
    if (typeof snap.plan_mode === "boolean") {
      state.planMode = snap.plan_mode;
      planChip.classList.toggle("active", state.planMode);
    }
    if (snap.model_label) modelChip.textContent = snap.model_label + " ▾";
    if (snap.workspace) {
      const parts = snap.workspace.split(/[/\\]/);
      ctxProject.textContent = parts[parts.length - 1] || snap.workspace;
    }
  }

  function escapeHtml(s) {
    return String(s)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");
  }

  function send(msg) {
    if (state.ws && state.ws.readyState === WebSocket.OPEN) {
      state.ws.send(JSON.stringify(msg));
      return;
    }
    // REST fallback for tests / no WS
    if (msg.type === "submit") {
      fetch("/api/prompt", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ prompt: msg.prompt, plan_mode: msg.plan_mode }),
      })
        .then((r) => r.json())
        .then((snap) => applySnapshot(snap))
        .catch(console.error);
    } else if (msg.type === "accept_diff" || msg.type === "reject_diff") {
      fetch(`/api/diff/${encodeURIComponent(msg.id)}/${msg.type === "accept_diff" ? "accept" : "reject"}`, {
        method: "POST",
      })
        .then((r) => r.json())
        .then((snap) => applySnapshot(snap))
        .catch(console.error);
    } else if (msg.type === "new_agent") {
      fetch("/api/new_agent", { method: "POST" })
        .then((r) => r.json())
        .then((snap) => applySnapshot(snap))
        .catch(console.error);
    }
  }

  function submitFrom(el) {
    const prompt = (el.value || "").trim();
    if (!prompt) return;
    el.value = "";
    setView("session");
    send({ type: "submit", prompt, plan_mode: state.planMode });
  }

  planChip.addEventListener("click", () => {
    state.planMode = !state.planMode;
    planChip.classList.toggle("active", state.planMode);
  });

  $("#btn-new-agent").addEventListener("click", () => {
    send({ type: "new_agent" });
    setView("home");
  });
  $("#btn-submit-home").addEventListener("click", () => submitFrom(inputHome));
  $("#btn-submit-session").addEventListener("click", () =>
    submitFrom(inputSession)
  );
  inputHome.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submitFrom(inputHome);
    }
  });
  inputSession.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submitFrom(inputSession);
    }
  });
  window.addEventListener("keydown", (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "n") {
      e.preventDefault();
      $("#btn-new-agent").click();
    }
  });

  function connectWs() {
    const proto = location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${proto}://${location.host}/ws`);
    state.ws = ws;
    ws.onmessage = (ev) => {
      try {
        const msg = JSON.parse(ev.data);
        if (msg.type === "snapshot") applySnapshot(msg);
        else if (msg.type === "event" && msg.snapshot) applySnapshot(msg.snapshot);
      } catch (e) {
        console.error(e);
      }
    };
    ws.onopen = () => {
      ws.send(JSON.stringify({ type: "hello" }));
    };
    ws.onclose = () => {
      setTimeout(connectWs, 1500);
    };
  }

  // Bootstrap snapshot via REST
  fetch("/api/snapshot")
    .then((r) => r.json())
    .then(applySnapshot)
    .catch(() => {});
  connectWs();
})();
