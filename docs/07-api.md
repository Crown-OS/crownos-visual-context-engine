# 07 — IPC and API Specifications

Tags: **[V]** verified · **[A]** design decision.

## 13.1 Transports

| Interface | Transport | Peers |
|---|---|---|
| `crownos_context_v1` (version 2), `crownos_capture_privacy_v1` | Wayland (custom XML in crownos-protocols) | crownpositor ↔ any granted client, many at once, each with its own scope; any app → compositor (privacy) |
| `ext-image-copy-capture-v1`, `ext-foreign-toplevel-list-v1` | Wayland (standard, already implemented [V]) | crownpositor ↔ contextd |
| **Context API** `crownos-visual-context` | **crownos-ipc**: Unix socket `$XDG_RUNTIME_DIR/crownos/crownos-visual-context.sock`, 20-byte header, postcard payloads, ≤ 16 fds via SCM_RIGHTS, Request/Reply/Notify/Subscribe/Event kinds, signature-hashed selectors [V]. Peer identity: `SO_PEERPIDFD` → `AppKey`, opt-in `SO_PASSPIDFD` per message, scope check per request ([05 §11.3](05-privacy-security.md#113-caller-identity-and-scopes)) | contextd ↔ any granted app: shell, agents, CLI, Inspector, crownbar |
| Inference API `crownos-visual-context-infer` | crownos-ipc; bulk payloads (frames, text batches) in **sealed memfds** | contextd ↔ inferd. Any process of the same user can connect (crownos-ipc `SameUid`, socket `0600`); inferd checks no `AppKey` [V]. Received memfds must carry `F_SEAL_WRITE` [V] |
| Relay API `crownos-visual-context-relay` | crownos-ipc; text only | contextd ↔ relayd, which hosts the OpenAI-compatible HTTP backend (sidecars, LAN, cloud) |
| Agent bridge | MCP over Unix socket (rmcp, as `crownui/mcp` [V]) | agents ↔ crown-context-mcp ↔ Context API |
| Upstream services | crownos-ipc clients of `crownotify` (+ new `NotificationPosted`), `crownos-clipboard`; zbus for AT-SPI, UPower, power-profiles | contextd |

### 13.1.1 `crownos_context_v1` version 2

Version 1 allowed one binder, and that binder could pause everyone [V]. Version 2 opens the global to any app and moves the decision to the grant ([05 §11.3](05-privacy-security.md#113-caller-identity-and-scopes)). The interface names stay the same; the XML is `crownos-protocols/protocols/crownos-context-v1.xml` with the version raised to 2.

```
crownos_context_manager_v1                (global, version 2, any client)
  ── new events ──
  revoked                                  # grant removed; the object is inert from now on
  ── changed requests ──
  set_paused(uint paused)                  # pauses only this client's stream,
                                           # unless the client holds `engine`
crownos_context_window_v1
  flag_sensitive(uint reason, uint duration_ms)
                                           # open to every bound client; rate-limited;
                                           # it can only add privacy
```

Rules:
- Many clients can bind at once. The compositor resolves each client's `AppKey` at accept, checks it again at bind, and keeps a `Bound` entry with its scope.
- With no grant, the bind is parked and the user gets the first-use prompt. The prompt waits at most 120 s. After a deny that the user did not remember, the app is not asked again for 10 minutes. contextd does the same for Context API calls [V].
- Only `exe:/usr/bin/crown-contextd` can hold `engine`, and only while root controls the file. The compositor refuses `engine` to a process that started with `LD_PRELOAD` or `LD_LIBRARY_PATH` [V]. For a host process this is consent, not enforcement ([05 §11.3](05-privacy-security.md#113-caller-identity-and-scopes)).
- Events are filtered per client. A `metadata` client gets no `title` and no `selection` events.
- On a config reload, the compositor recomputes every scope. A client that lost its grant gets `revoked`.

Wire contract lives in crate `context-protocol` (lib, `default-features = false`, no daemon deps) — the convention used by crownos-clipboard / crownotify / voice-protocol [V]. Postcard is positional: **new fields are only appended**; breaking changes bump the service name suffix (`…-v2`).

## 13.2 Shared types (IDL — illustrative, not final Rust)

```
Timestamp     = i64 µs since epoch (UTC); responses also carry the local tz offset
Id<T>         = u64 newtype per table (SegmentId, EpisodeId, EntityId, FactId, MemoryRef …)
TimeRange     { start: Timestamp, end: Timestamp }
AppRef        { app_key, app_id, name }
Provenance    { source_kind: crownui|a11y|ocr|vlm|llm|compositor|user, source_id, t, snippet? }
Scored<T>     { item: T, score: f32, confidence: f32, provenance: [Provenance] }
Redaction     = summary_only | snippets | raw          # max level is set by caller scope
```

## 13.3 Context API (`crownos-visual-context`, v1)

The wire format stays v1. Only the authorizer changes: each request is checked against the caller's scopes.

| Method | Kind | Scope | Request → Response |
|---|---|---|---|
| `status()` | req | any scope | → `{ state: observing\|paused\|degraded, pause_reason?, tier, backends, capture_active, queue_depths }` |
| `current_context()` | req | `metadata` (titles, `doc_key` and entities need `text`) | → `CurrentContext` |
| `search(SearchQuery)` | req | `text` (`raw` for raw) | → `SearchResults` |
| `timeline(TimelineQuery)` | req | `text` | → `Timeline` |
| `episode(EpisodeId)` | req | `text` | → `Episode` with segments |
| `project(ProjectRef)` | req | `text` | → `ProjectMemory` |
| `resume_context(ProjectRef)` | req | `text` | → `ResumeContext` ("where you left off") |
| `entity(EntityRef)` | req | `text` | → `EntityCard` (aliases, facts valid now, neighbors ≤ 2 hops, recent episodes) |
| `context_pack(ContextPackRequest)` | req | `text` | → `ContextPack` (for agent prompt injection) |
| `procedures(ProcedureFilter)` | req | `text` | → `[Procedure]` |
| `preferences(scope)` | req | `text` | → `[Preference]` |
| `explain(MemoryRef)` | req | `text` | → evidence chain |
| `remember(MemoryAssertion)` | req | `memory_write` | → `MemoryRef` (user/agent-asserted fact/preference/note; source=user\|agent) |
| `correct(MemoryRef, Correction)` | req | `memory_write` | → new `MemoryRef` (old invalidated, never overwritten) |
| `forget(ForgetRequest)` | req | `memory_delete` | → `{ deleted_counts, cascaded_counts }` |
| `pause(duration?)` / `resume()` | notify | `pause` | — |
| `set_policy(PolicyPatch)` | req | `policy_admin` | → `Policy` (also persisted to `visual_context.ron`) |
| `export(ExportRequest)` | req | `raw` | fd → memfd/file with JSONL (open format; text and metadata only, no images) |
| `subscribe(Topics)` | subscribe | per topic: `metadata` for state topics, `text` for topics that carry titles or text | events below |

**Events:** `ContextChanged(CurrentContext)` · `SegmentClosed{segment_id, app, doc_key, label}` · `EpisodeSummarized{episode_id, title}` · `ProjectActivated{project}` · `ProcedureDetected{procedure}` · `MemoryCreated{memory_ref, kind}` · `CaptureStateChanged{state}` · `PolicyChanged`.
Slow subscribers drop events, never stall the service (crownos-ipc semantics [V]); `ContextChanged` is coalesced (latest wins, ≤ 4 Hz). A `metadata` subscriber gets `ContextChanged` with titles and text removed. When a grant is revoked, contextd drops the peer and its subscriptions.

## 13.4 Examples (JSON rendering of postcard messages, for readability)

**Current context** (caller with `text` scope; a `metadata` caller gets no `title`, `doc_key`, episode title or entities)

```json
// → current_context()
{
  "t": 1793102400000000,
  "focus": { "app": {"app_key":"code-oss","name":"Code - OSS"},
             "title": "event_driven.rs — crownpositor",
             "doc_key": "file:/home/u/crownpositor/compositor/src/context/mod.rs",
             "project": {"id": 42, "name": "crownpositor"} },
  "activity": { "label": "coding", "confidence": 0.91, "since": 1793101800000000 },
  "episode": { "id": 9812, "title": "Implementing context protocol fan-out", "confidence": 0.74 },
  "recent_entities": [ {"id": 7, "kind":"repository", "name":"crownpositor"},
                       {"id": 311, "kind":"document", "name":"crownos-context-v1.xml"} ],
  "open_documents": 6,
  "observing": true
}
```

**Historical search** — "where did I see the smithay damage tracker example?"

```json
// → search
{ "text": "smithay damage tracker example", "time": {"relative":"last_7_days"},
  "kinds": ["segment","episode"], "limit": 5, "redaction": "snippets" }
// ←
{ "results": [
  { "item": { "kind":"segment", "segment_id": 88123, "app":"firefox",
              "title":"niri/src/screencasting/pw_utils.rs at main · YaLTeR/niri",
              "url":"https://github.com/YaLTeR/niri/blob/main/src/screencasting/pw_utils.rs",
              "t_start": 1792950000000000, "episode_id": 9790,
              "snippet":"…damage_tracker.damage_output(1, &elements)… skip frame if None…",
              "frame_ref": {"frame_id": 551002} },
    "score": 0.83, "confidence": 0.95,
    "provenance": [{"source_kind":"a11y","source_id":"text_line:4410021"}] } ],
  "took_ms": 61 }
```

**Timeline** — "what was I working on yesterday?"

```json
// → timeline
{ "range": {"relative":"yesterday"}, "granularity": "episode", "min_importance": 0.3 }
// ←
{ "day_summary": "Mostly crownpositor capture work; reviewed two PRs in crownos-ipc; 40 min email.",
  "episodes": [
    { "id": 9790, "t_start": "09:12", "t_end": "11:40", "title": "Researching per-window capture",
      "label": "research_browsing", "project": "crownpositor", "apps": ["firefox","code-oss"] },
    { "id": 9795, "t_start": "13:05", "t_end": "13:45", "title": "Email triage", "label": "email" } ] }
```

**Context pack for an agent**

```json
// → context_pack
{ "task": "Open a PR for the context protocol", "token_budget": 1500,
  "include": ["working","project","facts","preferences","episodes"] }
// ←
{ "tokens": 1312,
  "sections": {
    "working": "Focused: code-oss, crownpositor/compositor/src/context/mod.rs (coding, 25 min).",
    "project": "crownpositor — Smithay compositor. Branch-per-feature; PRs reviewed by the lead maintainer (fact 1201, conf .88).",
    "facts": ["crownpositor pins smithay git rev e3d461a (valid since 2026-09-25)"],
    "preferences": ["Commits without co-author lines (user-confirmed)"],
    "episodes": ["9790 Researching per-window capture (yesterday)"] },
  "provenance_refs": [1201, 9790] }
```

**Forget**

```json
// → forget
{ "selector": { "time": {"last_minutes": 15} }, "cascade": true }
// ←
{ "deleted": {"segments": 3, "text_lines": 412, "frames": 9, "events": 57},
  "cascaded": {"facts_removed": 2, "facts_rescored": 5, "episodes_resummarized": 1} }
```

**Remember (user/agent-asserted memory)**

```json
// → remember
{ "kind": "preference", "scope": {"project": 42}, "key": "pr_target_branch",
  "value": "main", "source": "user" }
// ← { "memory_ref": {"kind":"preference","id": 77}, "confidence": 1.0 }
```

## 13.5 Inference API (`crownos-visual-context-infer`, internal)

| Method | Payload | Result |
|---|---|---|
| `capabilities()` | — | tiers, runtimes and devices per task, loaded models |
| `recognize_text(OcrJob)` | memfd: sealed copy of the changed crop (XRGB8888) [V] | lines `{text, bbox, conf}` per crop |
| `describe_screen(CaptionJob)` | memfd: sealed copy of the whole frame + app name + window title [V] | `ScreenDescription {content_kind, caption, visible_entities[]}` or `SchemaFailure` |
| `embed_text(batch)` | ≤ 32 texts + purpose (query or document) | int8[256] vectors + space id |
| `rerank(query, docs)` | ≤ 30 docs | scores |
| `summarize_segments(batch)` | ≤ 8 segment digests (text only) | one `SegmentSummary` per segment, or `SchemaFailure` |
| `unload(task?)` | — | — |

The methods are tasks of the task layer ([04 §10.4](04-inference.md#104-model-layer-in-context-infer)); none of them names a model. `generate` is a capability under the tasks, not a method here. relayd serves the same text-only tasks (`embed_text`, `rerank`, `summarize_segments`) through the HTTP backend; it never serves `recognize_text` or `describe_screen`. Every job carries `{job_id, priority: interactive|normal|batch, deadline}`; inferd may reject with `Busy{retry_after}`. A capture sends its model calls within a 4 s analysis budget and retries a busy call once inside it [V]. A frame that waits in the queue past its 60 s TTL is dropped and counted as `frame_dropped_unanalysed`.

## 13.6 Changes to existing CrownOS interfaces

| Repo | Change |
|---|---|
| crownos-protocols | `crownos-context-v1.xml` version 2 (many binders, per-client scope, `revoked`); `crownos_capture_privacy_v1.xml` |
| crownpositor | `compositor/src/context/` module; emit plugin-host window events; sensitivity veto in capture handlers; input buckets; pause hotkey; `AppKey` at accept and bind; `ContextState.bound: Vec<Bound>` with per-client event filtering; revoke live clients on config reload; first-use prompt through `PendingPrompts` + portal; consent dialogs ignore synthetic input; blank sensitive windows in output screencopy and screencast; set `org.a11y.Status.IsEnabled` at session start |
| crownos-ipc | `SO_PEERPIDFD` instead of `pidfd_open`; opt-in `SO_PASSPIDFD` sender check per message; pass the peer identity into generated handlers |
| xdg-desktop-portal-crownos | `AccessDialog` returns the choices (remember, scope); ship it in the ISO |
| crownotify | `event NotificationPosted { id, app_name, desktop_entry, summary, body, category, urgency, t }` (appended field set) |
| crownos-config | `visual_context` and new `context_access` sections; add `agents`, `visual_context`, `context_access` to the by-name API; split `Device` from `Runtime` in `AiConfig` and add `Vulkan`, `TensorRt`, `Rocm`, `Remote` |
| crownbar | Context engine indicator + pause menu (subscribes to `CaptureStateChanged`) |
| crownos-settings | New **Privacy** group: *Screen context* (enable, exclusions, private titles, retention, pause hotkey) and *App access* (grants per app, scope toggles, revoke, audit) |
| crownos-computer-use | Move `atspi` from 0.29 to 0.30, so both share one cache design |
| crownos-pkgbuilds | `crownos-visual-context` (+ `-cuda`, `-vulkan`, `-rocm` ggml modules). aarch64 builds and `onnxruntime-crownos-qnn` are a future track |
