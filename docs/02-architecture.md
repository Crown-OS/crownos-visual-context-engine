# 02 — Recommended System Architecture

Tags: **[V]** verified · **[R]** reported · **[A]** design decision / assumption to validate.

## 4. Architecture overview

Design rules:

1. **Observe ≠ understand.** Observation is cheap, synchronous, lossless for metadata. Understanding is asynchronous, budgeted, batched per *activity segment*, and may be deferred or skipped.
2. **The compositor never waits on VCGE.** All compositor→VCGE traffic is fire-and-forget Wayland events; capture is client-pulled.
3. **Fewest processes that give real isolation:** a key-holding broker (`crown-contextd`) and a sandboxed, network-less, DB-less inference worker (`crown-context-inferd`). Everything else is optional.
4. **One store, one writer.** SQLCipher SQLite, single writer task, read pool.
5. **Privacy is enforced at the earliest point that can enforce it** — compositor for pixels, policy gate for every observation, broker for every query.

### 4.1 Process topology

```mermaid
flowchart LR
  subgraph COMP["crownpositor (calloop, GLES)"]
    CTX["context module<br/>window fan-out · sensitivity · input buckets · pause"]
    CAP["ext-image-copy-capture<br/>(existing)"]
    FTL["ext-foreign-toplevel-list<br/>(existing)"]
  end

  subgraph D["crown-contextd (user service, broker)"]
    WB["Compositor Bridge<br/>(calloop wayland thread)"]
    COL["Collectors<br/>a11y · CrownUI tree · notify · clipboard · proc/project · files · power"]
    POL["Policy Gate"]
    FP["Frame Pipeline<br/>tile diff · crop · thumbnail"]
    COR["Correlator + Segmenter<br/>+ Activity Classifier"]
    WM["Working Memory"]
    MM["Memory Manager"]
    SCH["Job Scheduler<br/>budgets · power · priority"]
    RET["Retrieval Engine"]
    ST[("SQLCipher store<br/>events · FTS5 · vec · graph")]
    API["Context API<br/>crownos-ipc 'crownos-visual-context'"]
    AUD["Audit log"]
  end

  subgraph I["crown-context-inferd (sandboxed, no net, no DB)"]
    REG["Model registry"]
    ORT["OrtBackend<br/>OpenVINO · VitisAI · QNN · CUDA/TRT · CPU"]
    LLM["LlamaBackend<br/>CUDA · Vulkan · CPU"]
  end

  RELAY["crown-context-relayd<br/>(optional, consented cloud)"]
  MCP["crown-context-mcp<br/>(optional agent bridge)"]
  CLI["crown-context CLI / Inspector UI"]
  AGENTS["CrownOS agents · shell · launcher"]
  BAR["crownbar indicator"]

  CTX -- crownos_context_v1 --> WB
  FTL -- ids/title/app_id --> WB
  CAP -- frames + damage --> WB
  WB --> POL
  COL --> POL
  POL --> COR
  POL --> FP
  COR --> WM
  COR --> MM
  FP --> SCH
  MM --> SCH
  SCH -- memfd jobs (crownos-ipc) --> I
  I -- results --> SCH
  SCH --> MM
  MM --> ST
  WM --> API
  RET --> ST
  API --> RET
  API --> AUD
  SCH -. redacted payload .-> RELAY
  API --- MCP --- AGENTS
  API --- CLI
  API -- capture state --> BAR
  WB -- flag_sensitive / pause --> CTX
```

### 4.2 Why this shape (alternatives rejected)

| Alternative | Why rejected |
|---|---|
| Inference inside the compositor | Any stall/crash freezes the desktop; GPU contention with rendering. |
| Single monolithic daemon incl. models | NPU/GPU driver faults (e.g. cDSP crash on SIGKILL [R]) would take down the key holder and store; can't sandbox network/DB away from model code. |
| One microservice per component (OCR svc, vision svc, memory svc, …) | IPC overhead, N lifecycles, N failure modes for a single-user desktop. No isolation benefit beyond the two-process split. |
| PipeWire / portal for frames | Continuous-video semantics, consent dialogs, daemon hop; no per-window sensitivity veto; KDE-style indicator flashing [V screenpipe #7758]. |
| Python ML service | ORT + llama.cpp cover every model from Rust; Python adds ~300 MB+ RSS and a second packaging story. Python is used **only offline** in `tools/` for model export/quantization. |
| Graph DB / vector DB servers | Personal-scale (10⁴–10⁶ edges/vectors) fits SQLite; see [03-memory.md §9](03-memory.md#9-database-and-indexing). |

---

## 5. Compositor bridge: `crownos_context_v1`

New XML in `crownos-protocols/protocols/crownos_context_v1.xml`; server impl in `crownpositor/compositor/src/context/`. Privileged (no security-context) **and** pidfd-verified VCGE executable [A, reviewer decision #4]. Single binding at a time.

```
crownos_context_manager_v1                       (global, privileged)
  ── events ──
  window(new_id crownos_context_window_v1, string foreign_toplevel_identifier)
  focus(object<crownos_context_window_v1>? window, uint time_ms)
  session(uint state)                     # active | idle | locked
  input_activity(object window, uint interval_ms,
                 uint text, uint edit, uint nav, uint chords,
                 uint clicks, uint scroll_ticks, uint pointer_travel_bucket,
                 uint gestures, uint flags)        # flags: typing_paused, scroll_stopped
  selection(uint kind, object window?, string mime_types, uint size_hint, uint flags)
                                          # kind: clipboard|primary; flags: secret
  paused(uint paused, uint origin)        # origin: hotkey | engine | lock
  ── requests ──
  set_paused(uint paused)
  destroy

crownos_context_window_v1
  ── events ──
  process(fd pidfd, int pid)              # pidfd via SO_PEERPIDFD/pidfd_open
  sandbox(string engine, string app_id, string instance_id)
  placement(int x, int y, int w, int h, string output, uint workspace)
  state(uint flags)                       # fullscreen|maximized|minimized|visible|activated
  content_type(uint type)                 # wp_content_type: none|photo|video|game
  sensitivity(uint flags)                 # password_input|private_browsing|excluded|
                                          # app_requested|engine_flagged
  done
  closed
  ── requests ──
  flag_sensitive(uint reason, uint duration_ms)   # engine-side detectors feed the veto
  destroy

crownos_capture_privacy_v1                       (global, any client)
  get_surface_privacy(new_id, wl_surface)  →  set_excluded(uint excluded)
                                          # app-level analogue of WDA_EXCLUDEFROMCAPTURE
```

Compositor implementation notes [A]:

- **Window fan-out:** one internal `WindowEvent` enum produced at the existing choke points (`update_keyboard_focus`, `title_changed/app_id_changed`, map/unmap, `refresh_foreign_toplevels`) and fanned out to both `crownos_context_v1` **and the already-declared `crownos_plugin_host_v1` window events** (fixing that gap for crownplugind for free).
- **Sensitivity state machine** per toplevel: OR of `text-input purpose ∈ {Password, Pin}`, private-browsing rule match, policy exclusion (pushed by contextd as rule set — see below), `crownos_capture_privacy_v1`, engine flag (with expiry), session locked.
- **Capture veto:** VCGE's `ext-image-copy-capture` sessions for a sensitive window fail frames (`failed` reason `stopped`) and `capture_constraints` returns `None` for excluded sources; output-source capture by VCGE is disabled by default (window-only capture).
- **Exclusion rules** (app_id globs, title regexes per locale) are read by the compositor from `~/.config/crownos/visual_context.ron` via `crownos-config::subscribe_typed` so the veto works **even when contextd is not running or is compromised**.
- **Input buckets:** counters in the input path; emission timer every 2 s only when non-zero; text counters zeroed while sensitive.
- **Pause hotkey** (default `Super+Shift+.`, configurable) toggles compositor-side pause: no context events, all VCGE captures fail; indicator updated.
- **Cost budget:** < 50 µs per event emission, zero allocation in the input path, no blocking I/O. Verified by a benchmark in Phase 1.

---

## 6. Component responsibilities and boundaries

All in Rust (edition 2024, toolchain pinned with the rest of CrownOS). `thiserror` in libraries, `anyhow` in binaries, `tracing` logging (journald).

| # | Component | Lives in | Responsibilities | Inputs → Outputs | Interfaces | Failure handling | Resources (target) |
|---|---|---|---|---|---|---|---|
| C0 | **Compositor context module** | crownpositor | Window fan-out, pid/pidfd, sensitivity + veto, input buckets, selection metadata, pause | Smithay hooks → Wayland events | `crownos_context_v1`, `crownos_capture_privacy_v1` | Never blocks; if client is slow, Wayland buffers; client disconnect = all VCGE sessions dropped | < 50 µs/event, ~0 RAM |
| C1 | **Compositor Bridge** | contextd (dedicated calloop thread) | Bind globals, maintain window table keyed by foreign-toplevel id, own capture sessions (focused window only), forward typed `Observation`s | Wayland → bounded channel | crate `context-collect::compositor` | Reconnect with backoff; on reconnect rebuild window table; missing global → degrade to metadata-less mode and report status | 1 thread, < 5 MB |
| C2 | **Accessibility Extractor** | contextd (tokio task) | Event-driven AT-SPI subscription, focused-subtree text, URL (`DocURL`), password-role detection → `flag_sensitive` | a11y events → `A11yText`, `SensitiveHint` | `atspi` crate | Per-app timeouts (200 ms), node budget, circuit-breaker per app that misbehaves | < 10 MB |
| C3 | **CrownUI Tree Reader** | contextd | For CrownUI apps with agent-access grant, read `ui://tree` via MCP socket | MCP → `A11yText` (structured) | `crownui/mcp` client | Missing grant → skip | small |
| C4 | **System Collectors** | contextd | Notifications (crownotify `NotificationPosted`), clipboard content for allowed sources (crownos-clipboard `get`), process/app/project resolution (pidfd→cgroup→desktop entry; terminal cwd→git root), recent files (`recently-used.xbel`), power/PSI/thermal/NVML | IPC/proc/inotify → `Observation` | crownos-ipc clients, `monitor-system` resolver | Each collector independent; failures logged + status flag | < 10 MB |
| C5 | **Policy Gate** | contextd (inline) | Every observation and every derived artifact passes: exclusions, sensitivity, per-app text/clipboard/notification permissions, secret detectors (regex+entropy), retention class assignment | `Observation` → `Admitted<Observation>` or drop + counter | crate `context-policy` | Fail **closed**: unknown policy state = drop | µs per item |
| C6 | **Frame Pipeline** | contextd (`spawn_blocking`, semaphore 1) | Capture scheduling per trigger, damage-restricted tile hashing, volatile-region masking, crop of changed text regions, thumbnail encode (JPEG, 1280 px), OCR job emission | trigger + frame + damage → `OcrJob`, `Thumbnail` | crate `context-capture` | Frame failure/veto → record "capture_denied" metric only; never retry sensitive | ≤ 1 capture / 2 s hard cap; < 15 ms CPU per capture excl. OCR |
| C7 | **Correlator + Segmenter + Classifier** | contextd | Sliding 10-min event window; joins (input→damage→capture cause, copy→source window, notification→app), heartbeat-merged **activity segments**, task-level **episodes**, activity labels (rules + embedding centroids) | `Admitted<Observation>` → `SegmentEvent`, `EpisodeEvent`, `WorkingMemory` updates | crate `context-engine` | Pure, deterministic, replay-testable; state snapshotted every 60 s | < 20 MB |
| C8 | **Working Memory** | contextd (in-RAM) | Current focus, segment, episode guess, recent entities (LRU 64), open documents, last N segment summaries | C7 → API | API `current_context`, subscriptions | Snapshot/restore on restart | < 5 MB |
| C9 | **Memory Manager** | contextd | Persist segments/episodes; request derivations (embed, summarize, extract); entity resolution; fact upsert with bi-temporal invalidation; provenance; importance/confidence; deletion cascade | events + job results → store rows | crate `context-engine::memory` | Idempotent writes keyed by content hash; jobs retried with backoff, expired by deadline | — |
| C10 | **Job Scheduler** | contextd | Priority queue with deadlines; per-tier token buckets (captures, OCR, embed, LLM, VLM, cloud) scaled by power/thermal/PSI/fullscreen-game; batching; dispatch to inferd/relayd | jobs → inferd requests | crownos-ipc + memfd | Back-pressure: coalesce (latest wins per window), drop expired, never unbounded | — |
| C11 | **Consolidation Worker** | contextd job set (runs in inferd for model work) | Idle+AC: day/episode rollups, project memory refresh, fact dedup/invalidation, procedure mining, preference updates, retention pruning, re-embedding after model change, vacuum | store → store | internal | Checkpointed, resumable, abortable on user activity | runs only when idle |
| C12 | **Retrieval Engine** | contextd | Hybrid search (FTS5 BM25 + vector KNN + graph expansion + temporal filters) fused by RRF, optional rerank, context packing with token budget and caller-scope redaction | query → ranked results / `ContextPack` | crate `context-engine::retrieve` | Timeouts per stage, partial results flagged | p95 < 150 ms (no rerank) |
| C13 | **Store** | contextd | Schema/migrations, single writer task (coalesced transactions), read pool, media files | rows ↔ SQLite | crate `context-store` | WAL, `synchronous=NORMAL`, integrity check at startup, auto-backup of key metadata | — |
| C14 | **Context API** | contextd | Typed crownos-ipc service; caller identity (SO_PEERCRED + pidfd → exe + cgroup app), scope checks, rate limits, audit | requests → replies/events | [07-api.md](07-api.md) | Per-connection limits (crownos-ipc: 32 conns/uid, 64 subs) [V] | — |
| C15 | **Inference Worker** | `crown-context-inferd` | Model registry, backend selection per host, model load/unload on idle, task execution (ocr, embed_text, embed_image, ui_detect, summarize, extract, vlm_describe, rerank) | job (memfd payload) → result | crownos-ipc `crownos-visual-context-infer` | Crash → systemd restart; contextd requeues in-flight jobs once; per-backend circuit breaker falls back to CPU | see [06](06-performance.md) |
| C16 | **Cloud Relay** (optional) | `crown-context-relayd` | Consent check, re-verify redaction, provider adapters (OpenAI-compatible incl. LAN DGX Spark; vendor APIs), audit every upload | redacted payload → response | crownos-ipc; HTTPS | Disabled unless consent; network restricted by `IPAddressAllow` | ~10 MB, on demand |
| C17 | **Agent bridge** (optional) | `crown-context-mcp` | Expose API as MCP tools/resources for CrownOS agents with the agent's scopes | MCP ↔ API | rmcp (as crownui) | Stateless | small |
| C18 | **CLI / Inspector** | `crown-context`, later CrownUI app in crownos-settings | Inspect, search, explain provenance, forget, pause, export, benchmark | — | API | — | — |

**Lifecycle:** `crown-contextd.socket` + `.service` as **systemd user units** (pattern of `crown-voiced` [V]) started at graphical session; `crown-context-inferd` socket-activated by contextd's first job and exits after configurable idle (default 10 min) to release model memory; relayd socket-activated only if cloud consent exists. contextd stops all capture on `session=locked` and on `paused`.

---

## 7. Event and data-flow diagrams

### 7.1 Observation → memory (end-to-end)

```mermaid
flowchart TD
  A[Compositor events<br/>focus/title/input/selection] --> G{Policy Gate}
  B[a11y / CrownUI tree text] --> G
  C[notify / clipboard / proc / files] --> G
  G -- dropped --> X[(counters only)]
  G --> E[Event log row<br/>heartbeat-merged]
  G --> K[Correlator]
  K -->|capture trigger| F[Frame Pipeline]
  F -->|changed text crops| O[OCR job]
  O --> T[Window text state<br/>line-level delta]
  B --> T
  T --> S[Activity Segment<br/>open/extend/close]
  K --> S
  S -->|close| J1[embed segment text]
  S -->|close & important| J2[summarize + extract<br/>JSON schema, small LLM]
  S -->|low text coverage & important| J3[VLM describe keyframe]
  J1 & J2 & J3 --> M[Memory Manager]
  M --> EP[(Episodic)]
  M --> SEM[(Entities/Facts/Edges<br/>bi-temporal)]
  M --> WMEM[Working memory]
  EP & SEM -->|idle + AC| CONS[Consolidation]
  CONS --> PRJ[(Project)]
  CONS --> PROC[(Procedural)]
  CONS --> PREF[(Preference)]
```

### 7.2 Capture sequence (one trigger)

```mermaid
sequenceDiagram
  participant C as crownpositor
  participant B as Bridge (contextd)
  participant P as Policy
  participant F as Frame Pipeline
  participant I as inferd
  C->>B: input_activity(win, typing_paused)
  B->>P: trigger(win, TypingPause)
  P-->>B: allowed (not sensitive, budget ok, not video/game)
  B->>C: ext_image_copy_capture_frame.capture() on win session
  alt window sensitive
    C-->>B: failed(stopped)
  else
    C-->>B: ready + damage[rects]
    B->>F: frame (shm), damage
    F->>F: hash 32px tiles inside damage, mask volatile regions
    F->>I: OCR(det+rec) on changed crops (memfd)
    I-->>F: lines + boxes + confidences
    F->>B: TextDelta(win, +lines, -lines), thumbnail?
  end
```

### 7.3 Capture triggers and gating [A]

| Trigger | Source | Settle delay | Notes |
|---|---|---|---|
| Focus change | compositor `focus` | 400 ms (skip if focus leaves sooner — alt-tab flicker) | Always 1 capture per new segment |
| Title / URL change | foreign-toplevel / a11y | 400 ms | New document/page |
| Typing pause | `input_activity.flags.typing_paused` | 1.5 s after last text key | Only if text keys > 0 in segment |
| Scroll stop | `scroll_stopped` | 500 ms | Line cache dedups re-OCR of shifted lines |
| Copy | `selection(clipboard)` | 0 | Captures source window context of the copy |
| Damage settled | damage area > 3 % of window, then quiet 750 ms | — | Catches content arriving without input (page load, build output) |
| Idle backstop | timer | 60 s AC / 300 s battery | Only if damage occurred since last capture |

Gates (all must pass): not paused · window not sensitive/excluded · `content_type ∉ {video, game}` · window not fullscreen-video heuristic (continuous damage > 70 % for > 5 s) · capture bucket has a token · system not under PSI pressure (`cpu some avg10 > 40 %` defers).

### 7.4 Segment and episode model

```mermaid
stateDiagram-v2
  [*] --> Open: focus on (window, doc_key) ≥ 2 s
  Open --> Open: heartbeat (input or damage) within pulse 60 s
  Open --> Closed: doc_key change / focus away > 30 s / idle > 120 s / lock
  Closed --> [*]
```

- **doc_key** = first available of: CrownUI document id · a11y `DocURL` · file path (title/`/proc/<pid>/fd`) · terminal cwd/git root · normalized title (strip counters, unread badges, app suffix).
- **Episode (task)** = chain of segments joined when gap < 10 min **and** (same project **or** shared entities **or** cosine(segment embeddings) > 0.75); closes after 20 min without a joining segment. Episodes are the unit of episodic memory and of LLM summarization above segment level.
- Focus flickers < 2 s become `transition` events (used by procedural mining), not segments.

### 7.5 Activity classification [A]

Two-stage, no per-event LLM:

1. **Rules/priors:** desktop-entry `Categories`, app_id prior table, URL domain category list, terminal command class (from title), content_type.
2. **Embedding nearest-centroid** over (app, title, domain, top OCR/a11y keywords) against label prototypes (taxonomy below + user corrections as new prototypes). Margin = confidence.
3. If margin < threshold, the label is (re)assigned by the segment-summary LLM call that happens anyway.

Taxonomy v1: `coding, code_review, terminal_ops, reading_docs, research_browsing, writing, spreadsheet, design, communication_chat, email, meeting, media_consumption, gaming, admin_settings, file_management, shopping, finance, learning, other`.
