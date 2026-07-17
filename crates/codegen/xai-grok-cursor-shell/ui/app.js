/* Cursor Agents Home — modes, slash menu, attach, projects */
(() => {
  const state = {
    planMode: false,
    agentMode: "agent",
    view: "home",
    ws: null,
    attachments: [],
    projects: [],
    slashCommands: [],
    slashIndex: 0,
    slashFor: null, // which textarea
  };

  const $ = (sel) => document.querySelector(sel);
  const homeView = $("#view-home");
  const sessionView = $("#view-session");
  const historyList = $("#history-list");
  const projectsList = $("#projects-list");
  const transcript = $("#transcript");
  const activityList = $("#activity-list");
  const diffList = $("#diff-list");
  const diffInspect = $("#diff-inspect");
  const inputHome = $("#composer-input");
  const inputSession = $("#composer-input-session");
  const modeChip = $("#mode-chip");
  const modeChipSession = $("#mode-chip-session");
  const modelChip = $("#model-chip");
  const ctxProject = $("#ctx-project");
  const ctxBranch = $("#ctx-branch");
  const slashMenu = $("#slash-menu");
  const slashMenuSession = $("#slash-menu-session");
  const fileInput = $("#file-input");
  const projectModal = $("#project-modal");
  const projectModalList = $("#project-modal-list");
  const projectSearch = $("#project-search");

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

  function modeLabel(m) {
    if (m === "plan") return "Plan";
    if (m === "always") return "Always";
    return "Agent";
  }

  function modeClass(m) {
    if (m === "plan") return "mode-plan";
    if (m === "always") return "mode-always";
    return "mode-agent";
  }

  function applyModeChips() {
    for (const el of [modeChip, modeChipSession]) {
      if (!el) continue;
      el.textContent = modeLabel(state.agentMode);
      el.classList.remove("mode-plan", "mode-always", "mode-agent");
      el.classList.add(modeClass(state.agentMode));
    }
    // Placeholder reflects mode
    const ph =
      state.agentMode === "plan"
        ? "Plan and design before coding…  (type / for commands)"
        : "Plan, search, build anything…  (type / for commands)";
    if (inputHome) inputHome.placeholder = ph;
  }

  function renderHistory(items) {
    historyList.innerHTML = "";
    (items || []).forEach((s) => {
      const li = document.createElement("li");
      li.innerHTML = `<span>${escapeHtml(s.title)}</span><span class="meta">local</span>`;
      li.addEventListener("click", () => setView("session"));
      historyList.appendChild(li);
    });
  }

  function renderProjects(items) {
    state.projects = items || [];
    projectsList.innerHTML = "";
    (items || []).slice(0, 12).forEach((p) => {
      const li = document.createElement("li");
      li.innerHTML = `<span>${escapeHtml(p.name)}</span><span class="meta">${p.is_git ? "git" : ""}</span>`;
      li.title = p.path;
      li.addEventListener("click", () => setProject(p.path));
      projectsList.appendChild(li);
    });
    if (!items || !items.length) {
      const li = document.createElement("li");
      li.innerHTML = `<span class="meta">No projects in ~/projects</span>`;
      projectsList.appendChild(li);
    }
  }

  function renderAttachments() {
    for (const id of ["attach-chips", "attach-chips-session"]) {
      const el = document.getElementById(id);
      if (!el) continue;
      el.innerHTML = "";
      state.attachments.forEach((path) => {
        const name = path.split(/[/\\]/).pop();
        const chip = document.createElement("span");
        chip.className = "attach-chip";
        chip.innerHTML = `<span>${escapeHtml(name)}</span><button type="button" aria-label="remove">×</button>`;
        chip.querySelector("button").onclick = () => {
          state.attachments = state.attachments.filter((p) => p !== path);
          renderAttachments();
        };
        el.appendChild(chip);
      });
    }
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
    if (snap.projects) renderProjects(snap.projects);
    if (snap.chat) renderChat(snap.chat);
    if (snap.activity) renderActivity(snap.activity);
    if (snap.diffs) renderDiffs(snap.diffs);
    if (typeof snap.plan_mode === "boolean") state.planMode = snap.plan_mode;
    if (snap.agent_mode) state.agentMode = snap.agent_mode;
    applyModeChips();
    if (snap.model_label) modelChip.textContent = snap.model_label + " ▾";
    if (snap.workspace) {
      const parts = snap.workspace.split(/[/\\]/);
      ctxProject.textContent = (parts[parts.length - 1] || snap.workspace) + " ▾";
      ctxProject.title = snap.workspace;
    }
    if (ctxBranch) ctxBranch.textContent = snap.branch || "—";
    if (Array.isArray(snap.attachments)) {
      state.attachments = snap.attachments.slice();
      renderAttachments();
    }
    if (Array.isArray(snap.slash_commands)) {
      state.slashCommands = snap.slash_commands;
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
    restFallback(msg);
  }

  function restFallback(msg) {
    if (msg.type === "submit") {
      fetch("/api/prompt", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          prompt: msg.prompt,
          plan_mode: msg.plan_mode,
          mode: msg.mode,
          attachments: msg.attachments || [],
        }),
      })
        .then((r) => r.json())
        .then(applySnapshot)
        .catch(console.error);
    } else if (msg.type === "accept_diff" || msg.type === "reject_diff") {
      fetch(
        `/api/diff/${encodeURIComponent(msg.id)}/${msg.type === "accept_diff" ? "accept" : "reject"}`,
        { method: "POST" }
      )
        .then((r) => r.json())
        .then(applySnapshot)
        .catch(console.error);
    } else if (msg.type === "new_agent") {
      fetch("/api/new_agent", { method: "POST" })
        .then((r) => r.json())
        .then(applySnapshot)
        .catch(console.error);
    } else if (msg.type === "cycle_mode") {
      fetch("/api/mode/cycle", { method: "POST" })
        .then((r) => r.json())
        .then(applySnapshot)
        .catch(console.error);
    } else if (msg.type === "set_mode") {
      fetch("/api/mode", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ mode: msg.mode }),
      })
        .then((r) => r.json())
        .then(applySnapshot)
        .catch(console.error);
    } else if (msg.type === "set_project") {
      fetch("/api/project", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path: msg.path }),
      })
        .then((r) => r.json())
        .then(applySnapshot)
        .catch(console.error);
    } else if (msg.type === "attach") {
      fetch("/api/attach", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path: msg.path }),
      })
        .then((r) => r.json())
        .then(applySnapshot)
        .catch(console.error);
    }
  }

  function submitFrom(el) {
    const prompt = (el.value || "").trim();
    if (!prompt) return;
    el.value = "";
    hideSlash();
    // slash handled server-side too; show session for feedback
    if (!prompt.startsWith("/")) setView("session");
    send({
      type: "submit",
      prompt,
      plan_mode: state.agentMode === "plan",
      mode: state.agentMode,
      attachments: state.attachments.slice(),
    });
    // local attach clear after send (server also clears)
    if (!prompt.startsWith("/")) {
      state.attachments = [];
      renderAttachments();
    }
  }

  function setProject(path) {
    send({ type: "set_project", path });
    closeProjectModal();
  }

  function openProjectModal() {
    projectModal.classList.remove("hidden");
    projectSearch.value = "";
    fillProjectModal("");
    projectSearch.focus();
  }
  function closeProjectModal() {
    projectModal.classList.add("hidden");
  }
  function fillProjectModal(filter) {
    const f = (filter || "").toLowerCase();
    projectModalList.innerHTML = "";
    state.projects
      .filter(
        (p) =>
          !f ||
          p.name.toLowerCase().includes(f) ||
          p.path.toLowerCase().includes(f)
      )
      .forEach((p) => {
        const li = document.createElement("li");
        li.innerHTML = `<strong>${escapeHtml(p.name)}</strong><span class="path">${escapeHtml(p.path)}</span>`;
        li.onclick = () => setProject(p.path);
        projectModalList.appendChild(li);
      });
  }

  // —— Slash autocomplete ——
  function hideSlash() {
    slashMenu.classList.add("hidden");
    slashMenuSession.classList.add("hidden");
    state.slashFor = null;
  }

  function showSlash(textarea, menu, filter) {
    state.slashFor = textarea;
    const q = filter.toLowerCase();
    const cmds = (state.slashCommands || []).filter(
      (c) => !q || c.name.starts_with(q) || c.name.includes(q)
    );
    menu.innerHTML = "";
    if (!cmds.length) {
      menu.classList.add("hidden");
      return;
    }
    state.slashIndex = 0;
    cmds.forEach((c, i) => {
      const li = document.createElement("li");
      if (i === 0) li.classList.add("active");
      li.innerHTML = `<span class="cmd">/${escapeHtml(c.name)}</span><span class="desc">${escapeHtml(c.description)}</span>`;
      li.onmousedown = (e) => {
        e.preventDefault();
        applySlashPick(textarea, c.name);
      };
      menu.appendChild(li);
    });
    menu.classList.remove("hidden");
  }

  function applySlashPick(textarea, name) {
    const v = textarea.value;
    const caret = textarea.selectionStart || v.length;
    const before = v.slice(0, caret);
    const after = v.slice(caret);
    const m = before.match(/(^|\s)\/(\S*)$/);
    if (m) {
      const start = before.lastIndexOf("/");
      textarea.value = before.slice(0, start) + "/" + name + " " + after;
    } else {
      textarea.value = "/" + name + " ";
    }
    hideSlash();
    textarea.focus();
  }

  function onComposerInput(textarea, menu) {
    const v = textarea.value;
    const caret = textarea.selectionStart || v.length;
    const before = v.slice(0, caret);
    const m = before.match(/(^|\s)\/([a-zA-Z0-9_-]*)$/);
    if (m) {
      showSlash(textarea, menu, m[2] || "");
    } else {
      menu.classList.add("hidden");
      state.slashFor = null;
    }
  }

  function onComposerKeydown(e, textarea, menu) {
    if (!menu.classList.contains("hidden")) {
      const items = [...menu.querySelectorAll("li")];
      if (e.key === "ArrowDown") {
        e.preventDefault();
        state.slashIndex = Math.min(state.slashIndex + 1, items.length - 1);
        items.forEach((li, i) => li.classList.toggle("active", i === state.slashIndex));
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        state.slashIndex = Math.max(state.slashIndex - 1, 0);
        items.forEach((li, i) => li.classList.toggle("active", i === state.slashIndex));
        return;
      }
      if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey && items.length)) {
        const active = items[state.slashIndex];
        if (active) {
          e.preventDefault();
          const name = active.querySelector(".cmd").textContent.replace(/^\//, "");
          applySlashPick(textarea, name);
          return;
        }
      }
      if (e.key === "Escape") {
        e.preventDefault();
        hideSlash();
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submitFrom(textarea);
    }
  }

  // —— Attach files ——
  function openFilePicker() {
    // Browser file picker (local path not available for security — use path prompt fallback)
    fileInput.onchange = () => {
      // File API only gives name, not full path in browser. Prompt for absolute path.
      const files = [...(fileInput.files || [])];
      if (!files.length) return;
      const names = files.map((f) => f.name).join(", ");
      const hint = prompt(
        `Browsers hide full paths. Enter absolute path(s) for: ${names}\n(or a single path)`,
        ""
      );
      if (hint && hint.trim()) {
        hint
          .split(/[\n,]/)
          .map((s) => s.trim())
          .filter(Boolean)
          .forEach((p) => {
            state.attachments.push(p);
            send({ type: "attach", path: p });
          });
        renderAttachments();
      } else {
        // Still show name chips so user sees something was selected
        files.forEach((f) => {
          const fake = f.name;
          if (!state.attachments.includes(fake)) state.attachments.push(fake);
        });
        renderAttachments();
        alert(
          "Attached by name only. For the agent to read the file, use absolute paths via the prompt: e.g. attach /Users/you/proj/file.rs — or enter path when asked."
        );
      }
      fileInput.value = "";
    };
    fileInput.click();
  }

  // Prefer path dialog for reliability with local agent
  function openAttachPathDialog() {
    const p = prompt("Absolute file path to attach:");
    if (p && p.trim()) {
      const path = p.trim();
      if (!state.attachments.includes(path)) state.attachments.push(path);
      renderAttachments();
      send({ type: "attach", path });
    }
  }

  // Events
  modeChip.addEventListener("click", () => send({ type: "cycle_mode" }));
  if (modeChipSession)
    modeChipSession.addEventListener("click", () => send({ type: "cycle_mode" }));

  $("#btn-new-agent").addEventListener("click", () => {
    send({ type: "new_agent" });
    setView("home");
  });
  $("#btn-submit-home").addEventListener("click", () => submitFrom(inputHome));
  $("#btn-submit-session").addEventListener("click", () =>
    submitFrom(inputSession)
  );
  $("#btn-attach").addEventListener("click", openAttachPathDialog);
  if ($("#btn-attach-session"))
    $("#btn-attach-session").addEventListener("click", openAttachPathDialog);

  ctxProject.addEventListener("click", openProjectModal);
  $("#project-modal-close").addEventListener("click", closeProjectModal);
  projectModal.addEventListener("click", (e) => {
    if (e.target === projectModal) closeProjectModal();
  });
  projectSearch.addEventListener("input", () => fillProjectModal(projectSearch.value));

  inputHome.addEventListener("input", () => onComposerInput(inputHome, slashMenu));
  inputHome.addEventListener("keydown", (e) =>
    onComposerKeydown(e, inputHome, slashMenu)
  );
  inputSession.addEventListener("input", () =>
    onComposerInput(inputSession, slashMenuSession)
  );
  inputSession.addEventListener("keydown", (e) =>
    onComposerKeydown(e, inputSession, slashMenuSession)
  );

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
      } catch (err) {
        console.error(err);
      }
    };
    ws.onopen = () => ws.send(JSON.stringify({ type: "hello" }));
    ws.onclose = () => setTimeout(connectWs, 1500);
  }

  fetch("/api/snapshot")
    .then((r) => r.json())
    .then(applySnapshot)
    .catch(() => {});
  connectWs();
})();
