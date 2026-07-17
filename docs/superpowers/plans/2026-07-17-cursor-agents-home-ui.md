# Cursor Agents Home UI on Grok Build — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use **superpowers:subagent-driven-development** (recommended) to implement this plan task-by-task. Fresh implementer subagent per task + two-stage review (spec compliance → code quality). Steps use checkbox (`- [ ]`) syntax for tracking.
>
> Also load: `superpowers:test-driven-development` (per-task TDD), `superpowers:using-git-worktrees` (isolated branch), `superpowers:finishing-a-development-branch` (after all tasks).

**Goal:** Replace the wrong 3-column ratatui “IDE” with a **Cursor Agents Home** UX (agent sidebar + empty canvas + floating Composer), still driven by the real Grok Build agent runtime — structure-matching the live Cursor app Dante screenshotted.

**Architecture:** CLI owns agent process + local Axum control plane; browser renders Cursor-clone static UI over WebSocket events. Keep pure `CursorSession` reducers; demote ratatui to `--tui` fallback only.

**Tech Stack:** Rust 2024 workspace, existing `xai-grok-cursor-shell`, `axum` 0.8 + WS, `tower-http`, static `ui/` (HTML/CSS/JS), `RealGrokAgentDriver` ACP stdio, `xai-hunk-tracker` for accept/reject.

**Repo:** `/Users/danteperea/projects/grok-build-cursor-cli` · remote `dante-perea/grok-build-cursor-cli`  
**Branch (execute):** `feat/cursor-agents-home-ui` via git worktree skill  
**Spec/plan copy (execute):** also write `docs/superpowers/plans/2026-07-17-cursor-agents-home-ui.md` + design notes under `docs/superpowers/specs/`

---

## 0. Problem diagnosis (why current UI failed)

| Signal | Observation |
|--------|-------------|
| User Image #1 | Our shell: terminal, file tree, bordered Workspace/Chat/Activity/Diff |
| User Image #2 + live capture | Cursor: **Agents** shell — New Agent, repo history, **centered pill Composer**, Plan chip, model picker, no explorer |
| Product error | We cloned Grok Build / Claude-Code multipane mental model, not Cursor Agents |

**Do not** polish ratatui columns. **Do** rebuild the default surface as Agent Home.

Computer-use evidence already on disk:
- `.../assets/image-b64e08e3-*.jpg` (user Cursor)
- `.../assets/cursor-now-sm.png`, `cursor-explore-1.png` (live)
- Window bounds for cliclick: `0,33,1728,1084` · cliclick at `/opt/homebrew/bin/cliclick` (Quartz MCP path needs fix at execute)

---

## 1. Technical decisions (full analysis)

### TD-1 — UI host: Web (local) vs ratatui vs Tauri/Electron

| Option | Fidelity to Cursor | Ship speed | Ops cost | Decision |
|--------|-------------------|------------|----------|----------|
| **A. Local web UI + CLI server** | High (CSS cards, sidebar, floating composer) | Fast (axum already in workspace) | Browser open | **RECOMMENDED** |
| B. Redesign ratatui Agent Home | Low–medium (box drawing never matches Cursor) | Medium | Pure TTY | Fallback only (`--tui`) |
| C. Tauri/Electron shell | Highest | Slow (new stack, packaging) | Heavy | Out of scope |

**Decision A.** Cursor’s UI language is web-native. Axum + static `ui/` reuses `workspace.dependencies` (`axum`, `tower-http`). CLI remains the product binary.

### TD-2 — Transport: WebSocket vs SSE vs polling

| Option | Streaming agent events | Bidirectional | Complexity |
|--------|------------------------|---------------|------------|
| **WS** | Yes | Yes (prompt + control) | Medium |
| SSE + REST | Yes (server→client) | REST for prompt | Slightly simpler |
| Polling | Poor | REST | Reject |

**Decision:** **WebSocket `/ws`** for event stream + client commands (`submit`, `accept_diff`, `reject_diff`, `new_agent`). Optional REST `POST /api/prompt` for tests without WS.

### TD-3 — Server framework

**Decision: `axum` 0.8** (already workspace pin with `ws` feature). Serve `tower-http` `ServeDir` for `ui/`. No new heavy deps.

### TD-4 — State ownership

| Layer | Owns | Does not own |
|-------|------|--------------|
| `CursorSession` (Rust) | Layout mode, chat, composer, activity, diffs, busy | Pixels |
| `AgentHistoryStore` (new) | Sidebar sessions list (JSON under `~/.grok/cursor-cli/`) | Transcript blobs optional v1 |
| Browser | DOM, focus, optimistic typing | Authoritative agent truth |
| `RealGrokAgentDriver` | ACP process, line mapping | UI |

**Decision:** Browser is dumb view. All agent truth from server events → same `bind_events` / `reduce` path as today.

### TD-5 — Agent home vs active session (views)

**Decision: two view modes in one SPA**

1. `home` — Cursor Agents home (default)
2. `session` — after first submit or history click: transcript + activity strip + diff panel; composer docks bottom

### TD-6 — History model (v1 YAGNI)

**Decision:** File-backed `SessionMeta { id, title, workspace, updated_at, source: Local }` only. No cloud sync. Title = first prompt truncated. No full transcript replay in v1 (active session only in memory); history click starts **new** view with title label (or reload if we persist messages later).

*Rationale:* Sidebar presence matches Cursor chrome; durable transcript is phase 2.

### TD-7 — Visual fidelity bar

**Decision: structure + interaction parity**, not pixel-perfect Cursor branding.

Must match:
- Left agent rail (New Agent, history groups)
- Centered floating composer on home
- Plan mode chip + model label (model can be static “Grok” / config string)
- Dark flat theme, soft radii

May approximate:
- Icons, fonts, exact spacing, proprietary Fable models, Cloud agents

### TD-8 — Plan mode chip

**Decision:** UI chip toggles `plan_mode: bool` on session; when true, prefix system note or pass metadata on submit (`[plan mode] ...`) into prompt for Grok agent. No separate Grok plan-mode protocol required for v1.

### TD-9 — Legacy multipane TUI

**Decision:** Keep behind `--tui`. Default is web Agents home. Prevents losing headless dump tests overnight; dump-layout schema **changes** to Agent Home regions.

### TD-10 — Subagent-driven development (execution process)

**Decision: superpowers SDD (recommended)**

Per task:
1. Orchestrator extracts **full task text** + scene-setting context (subagent never reads plan file alone)
2. **Implementer** subagent: TDD → implement → test → commit → self-review
3. **Spec reviewer** subagent: matches task acceptance only (no extras)
4. **Code quality reviewer** subagent: only after spec ✅
5. Fix loops until both ✅; mark todo complete; next task
6. No parallel implementers (merge conflicts)
7. After all tasks: final whole-diff review + finishing-a-development-branch

Worktree: `feat/cursor-agents-home-ui` isolated from main until green.

### TD-11 — Test strategy

| Layer | How |
|-------|-----|
| Unit | Existing session/driver tests; new history store tests |
| Integration | `axum` `oneshot` / hyper client: POST prompt + fixture agent → events contain ProposedEdit |
| UI structure | `--dump-layout` JSON asserts Agent Home regions (`new_agent`, `history`, `floating_composer`, `plan_chip`) |
| Visual | Computer-use side-by-side checklist (execute-time, not flake CI) |

---

## 2. File map (create / modify)

### Create
- `crates/codegen/xai-grok-cursor-shell/ui/index.html`
- `crates/codegen/xai-grok-cursor-shell/ui/styles.css`
- `crates/codegen/xai-grok-cursor-shell/ui/app.js`
- `crates/codegen/xai-grok-cursor-shell/src/server.rs` — Axum app, WS, static
- `crates/codegen/xai-grok-cursor-shell/src/history.rs` — session list store
- `crates/codegen/xai-grok-cursor-shell/src/layout_home.rs` — Agent Home dump schema (pure)
- `crates/codegen/xai-grok-cursor-shell/tests/http_agent_e2e.rs`
- `docs/superpowers/plans/2026-07-17-cursor-agents-home-ui.md` (copy of this plan at execute)
- `docs/superpowers/specs/2026-07-17-cursor-agents-home-design.md` (TD summary + screenshots refs)

### Modify
- `Cargo.toml` (crate): add `axum`, `tower-http`, `futures`, `bytes` as needed from workspace
- `src/lib.rs` — export server/history/layout_home
- `src/main.rs` — default web serve; `--tui`; `--port`; open browser
- `src/app.rs` — extract shared `drive_real_agent` for server; keep dump async
- `src/session.rs` — `ViewMode { Home, Session }`, plan_mode flag, history hooks
- `README.md` — Agents home screenshots description + launch

### Reuse (do not rewrite)
- `agent_driver.rs` — `RealGrokAgentDriver`, `map_agent_line_all`, `apply_change_to_disk`
- `agent_bridge.rs` — `bind_events`, `AgentRuntimeEvent`
- `session.rs` reduce/effects
- `tests/fixtures/fake-grok-agent.sh`

---

## 3. Wire protocol (client ↔ server)

### Client → Server (WS JSON)

```json
{ "type": "submit", "prompt": "…", "plan_mode": true }
{ "type": "new_agent" }
{ "type": "accept_diff", "id": "edit-1" }
{ "type": "reject_diff", "id": "edit-1" }
{ "type": "select_session", "id": "uuid" }
```

### Server → Client

```json
{ "type": "snapshot", "view": "home|session", "layout": {…}, "history": […], "chat": […], "activity": […], "diffs": […], "status": "…" }
{ "type": "event", "event": { /* AgentRuntimeEvent */ } }
{ "type": "error", "message": "…" }
```

After each reduce, server may push full `snapshot` (v1 simplicity) or delta; **prefer full snapshot** for fewer bugs.

---

## 4. Agent Home dump schema (verification)

`grok-build-cursor-cli --dump-layout` prints JSON:

```json
{
  "product": "cursor-agents-home",
  "regions": ["sidebar_new_agent", "sidebar_history", "floating_composer", "plan_chip", "model_chip", "project_context"],
  "not_regions": ["file_tree_primary", "three_column_ide"],
  "view": "home"
}
```

CI asserts `product == cursor-agents-home` and absence of primary file-tree IDE.

---

## 5. Subagent-driven task breakdown

Orchestrator rules:
- One implementer at a time
- Each task prompt includes: goal, files, steps, acceptance, reuse APIs, forbidden extras
- Model: mechanical tasks → fast; server/UI integration → standard; review → strongest available

---

### Task 1: Agent Home layout model (pure Rust) + tests

**Files:**
- Create: `src/layout_home.rs`
- Modify: `src/lib.rs`
- Test: unit tests in `layout_home.rs`

**Acceptance:** `HomeLayoutSnapshot` lists Cursor home regions; `is_cursor_agents_home() == true`; explicitly not multipane IDE.

- [ ] **Step 1:** Failing test `default_home_snapshot_is_cursor_agents_home`
- [ ] **Step 2:** Implement `HomeLayoutSnapshot { regions, view, show_file_tree_primary: false }`
- [ ] **Step 3:** Tests pass; commit `feat(cursor-shell): Agent Home layout snapshot model`

---

### Task 2: Session history store

**Files:**
- Create: `src/history.rs`
- Modify: `src/lib.rs`
- Test: `history` unit tests with `tempfile`

**Acceptance:** create/list/update session metas on disk; title from first prompt; YAGNI no cloud.

- [ ] **Step 1:** Failing tests for add + list + persist reload
- [ ] **Step 2:** Implement `AgentHistoryStore` JSON file
- [ ] **Step 3:** Pass + commit `feat(cursor-shell): local agent history store`

---

### Task 3: Session view mode + plan_mode in reducer

**Files:**
- Modify: `src/session.rs`
- Test: session unit tests

**Acceptance:** default `ViewMode::Home`; `ComposerSubmit` → `ViewMode::Session` + history record effect; `plan_mode` flag on submit prefixes prompt or metadata.

- [ ] **Step 1:** Failing tests for view transition + plan_mode
- [ ] **Step 2:** Minimal reducer changes + `SessionEffect::RecordHistory { title }`
- [ ] **Step 3:** Pass + commit

---

### Task 4: Axum static + health endpoint

**Files:**
- Create: `src/server.rs` (skeleton), `ui/index.html` (placeholder shell with `data-testid`s)
- Modify: `Cargo.toml`, `lib.rs`, `main.rs`
- Test: `tests/http_agent_e2e.rs` — `GET /` 200, `GET /api/health` 200

**Acceptance:** `cargo run -p xai-grok-cursor-shell -- --port 0` serves UI; no browser required for test.

- [ ] **Step 1:** Failing HTTP test
- [ ] **Step 2:** Axum router + `ServeDir` for `ui/`
- [ ] **Step 3:** Pass + commit

---

### Task 5: Cursor Agents Home static UI (structure)

**Files:**
- Create/overwrite: `ui/index.html`, `ui/styles.css`, `ui/app.js` (UI only; mock WS later ok)

**Acceptance:** Opened in browser looks like Cursor home structure: left New Agent + history placeholders, center floating composer, Plan chip, model chip, project context row. **No file tree.** Use dark theme, rounded composer card. `data-testid` on all regions.

- [ ] **Step 1:** Build HTML structure matching inventory
- [ ] **Step 2:** CSS layout (sidebar ~240px, centered composer max-width ~600px)
- [ ] **Step 3:** Manual open `ui/index.html` or via server; commit `feat(cursor-shell): Cursor Agents home static UI`

---

### Task 6: WebSocket bridge + RealGrokAgentDriver

**Files:**
- Modify: `src/server.rs`, `src/app.rs` (share driver helpers), `ui/app.js`
- Test: `tests/http_agent_e2e.rs` with fixture agent + `--require-agent` path via env `GROK_AGENT_BIN`

**Acceptance:** Client `submit` → server runs `RealGrokAgentDriver.submit_prompt` → WS pushes events → snapshot shows chat/activity/diffs. **No** `simulate_representative_turn` on require-agent path.

- [ ] **Step 1:** Failing integration test (WS or REST submit + fixture)
- [ ] **Step 2:** Implement WS handler + broadcast
- [ ] **Step 3:** Pass + commit

---

### Task 7: Diff accept/reject via API/WS

**Files:**
- Modify: `server.rs`, `ui/app.js`
- Test: e2e write temp file, reject restores old_text

**Acceptance:** Same as `apply_change_to_disk` behavior; UI updates decision badges.

- [ ] TDD + commit

---

### Task 8: CLI default path + dump-layout + open browser

**Files:**
- Modify: `main.rs`, `README.md`
- Test: binary `--dump-layout` JSON product field

**Acceptance:**
```
grok-build-cursor-cli --dump-layout
# → product: cursor-agents-home
grok-build-cursor-cli --port 9876
# → serves + opens browser (skip open if `--no-open`)
```

- [ ] TDD dump-layout; commit

---

### Task 9: Computer-use inventory + parity evidence

**Files:**
- Create: session assets `cursor-inventory/*`, `{SCRATCH}/cursor-parity.md`
- Fix: mac-computer-use click via cliclick absolute coords if needed

**Acceptance:** Checklist: New Agent, history, floating composer, plan chip, active session after prompt — all documented vs our UI screenshots.

- [ ] Capture Cursor states
- [ ] Capture our UI
- [ ] Write parity md; commit assets if in-repo, else scratch only

---

### Task 10: Final review + ship

**Orchestrator:**
- [ ] Dispatch final code-quality review on whole branch
- [ ] `cargo test -p xai-grok-cursor-shell` full green
- [ ] Dual launch `--dump-layout` + HTTP smoke
- [ ] Push branch; finishing-a-development-branch (PR or merge per Dante)

---

## 6. SDD orchestration checklist (controller)

```
[ ] Create worktree feat/cursor-agents-home-ui
[ ] TodoWrite all 10 tasks
[ ] For each task:
      [ ] Dispatch implementer (full task text + context + forbidden scope)
      [ ] Answer questions if any
      [ ] Spec reviewer
      [ ] Code quality reviewer
      [ ] Fix loops
      [ ] Mark complete
[ ] Final reviewer
[ ] Finish branch
```

**Implementer context bundle (every task):**
- Repo path, crate path, TD-1..TD-11 one-liners
- “Do not restore 3-column IDE as default”
- Reuse list: driver, bind_events, apply_change_to_disk
- Fixture path for agent tests

---

## 7. Verification plan (gate complete)

1. **Structure:** `--dump-layout` → `product=cursor-agents-home`, regions include floating_composer + sidebar_new_agent; **not** file_tree_primary.
2. **Visual:** Browser UI side-by-side with Cursor Agents home (computer-use) — structure match.
3. **Agent:** Fixture agent via RealGrokAgentDriver produces tool + ProposedEdit in UI/API.
4. **Diffs:** accept/reject disk apply tests green.
5. **require-agent:** missing binary → error, no fake diffs.
6. **Regression:** existing pure reducer tests still pass; `--tui` still builds if kept.

---

## 8. Risks & mitigations

| Risk | Mitigation |
|------|------------|
| Scope creep to full VS Code | YAGNI: home + session + diffs only |
| Browser not available in CI | dump-layout + HTTP tests; no browser in CI |
| WS flakiness | Prefer snapshot over fragile deltas; fixture agent short timeout |
| Subagent drift back to multipane | Spec reviewer checklist item “default is Agents home” |
| cliclick/Quartz broken | Use `/opt/homebrew/bin/cliclick` absolute coords from window bounds |

---

## 9. Out of scope (explicit)

- Cloud agents, Automations product, Customize screens (sidebar links can be no-op stubs)
- Full transcript persistence / multi-tab agent grid
- Pixel-perfect Cursor icons/fonts
- Replacing Grok Build pager itself

---

## 10. Success definition (Dante)

Opening `grok-build-cursor-cli` no longer shows a terminal file-explorer IDE. It shows an **Agents home** that a human compares to Cursor Image #2 and recognizes: left agent rail, empty canvas, floating “Plan and design…” composer, Plan + model chips. Typing a prompt drives real Grok Build tools/edits into session + diff review.
