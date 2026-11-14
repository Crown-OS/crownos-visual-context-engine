# 07 — IPC and API Specifications

Tags: **[V]** verified · **[A]** design decision.

## 13.1 Transports

| Interface | Transport | Peers |
|---|---|---|
| `crownos_context_v1`, `crownos_capture_privacy_v1` | Wayland (custom XML in crownos-protocols) | crownpositor ↔ contextd; any app → compositor (privacy) |
| `ext-image-copy-capture-v1`, `ext-foreign-toplevel-list-v1` | Wayland (standard, already implemented [V]) | crownpositor ↔ contextd |
| **Context API** `crownos-visual-context` | **crownos-ipc**: Unix socket `$XDG_RUNTIME_DIR/crownos/crownos-visual-context.sock`, 20-byte header, postcard payloads, SO_PEERCRED auth, ≤ 16 fds via SCM_RIGHTS, Request/Reply/Notify/Subscribe/Event kinds, signature-hashed selectors [V] | contextd ↔ shell, agents, CLI, Inspector, crownbar |
| Inference API `crownos-visual-context-infer` | crownos-ipc; bulk payloads (images, text batches) in **sealed memfds** | contextd ↔ inferd (only contextd's uid+exe allowed) |
| Relay API `crownos-visual-context-relay` | crownos-ipc | contextd ↔ relayd |
| Agent bridge | MCP over Unix socket (rmcp, as `crownui/mcp` [V]) | agents ↔ crown-context-mcp ↔ Context API |
| Upstream services | crownos-ipc clients of `crownotify` (+ new `NotificationPosted`), `crownos-clipboard`; zbus for AT-SPI, UPower, power-profiles | contextd |

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

| Method | Kind | Scope | Request → Response |
|---|---|---|---|
| `status()` | req | any granted | → `{ state: observing\|paused\|degraded, pause_reason?, tier, backends, capture_active, queue_depths }` |
| `current_context()` | req | `context.current` | → `CurrentContext` |
| `search(SearchQuery)` | req | `context.search` (`raw` for raw) | → `SearchResults` |
| `timeline(TimelineQuery)` | req | `context.search` | → `Timeline` |
| `episode(EpisodeId)` | req | `context.search` | → `Episode` with segments |
| `project(ProjectRef)` | req | `context.search` | → `ProjectMemory` |
| `resume_context(ProjectRef)` | req | `context.search` | → `ResumeContext` ("where you left off") |
| `entity(EntityRef)` | req | `context.search` | → `EntityCard` (aliases, facts valid now, neighbors ≤ 2 hops, recent episodes) |
| `context_pack(ContextPackRequest)` | req | `context.search` | → `ContextPack` (for agent prompt injection) |
| `procedures(ProcedureFilter)` | req | `context.search` | → `[Procedure]` |
| `preferences(scope)` | req | `context.search` | → `[Preference]` |
| `explain(MemoryRef)` | req | `context.search` | → evidence chain |
| `remember(MemoryAssertion)` | req | `memory.write` | → `MemoryRef` (user/agent-asserted fact/preference/note; source=user\|agent) |
| `correct(MemoryRef, Correction)` | req | `memory.write` | → new `MemoryRef` (old invalidated, never overwritten) |
| `forget(ForgetRequest)` | req | `memory.delete` | → `{ deleted_counts, cascaded_counts }` |
| `pause(duration?)` / `resume()` | notify | `policy.admin` or shell | — |
| `set_policy(PolicyPatch)` | req | `policy.admin` | → `Policy` (also persisted to `visual_context.ron`) |
| `export(ExportRequest)` | req | `context.raw` | fd → memfd/file with JSONL + media (open format) |
| `subscribe(Topics)` | subscribe | per topic | events below |

**Events:** `ContextChanged(CurrentContext)` · `SegmentClosed{segment_id, app, doc_key, label}` · `EpisodeSummarized{episode_id, title}` · `ProjectActivated{project}` · `ProcedureDetected{procedure}` · `MemoryCreated{memory_ref, kind}` · `CaptureStateChanged{state}` · `PolicyChanged`.
Slow subscribers drop events, never stall the service (crownos-ipc semantics [V]); `ContextChanged` is coalesced (latest wins, ≤ 4 Hz).

## 13.4 Examples (JSON rendering of postcard messages, for readability)

**Current context**

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
                       {"id": 311, "kind":"document", "name":"crownos_context_v1.xml"} ],
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
              "frame_ref": {"frame_id": 551002, "has_thumbnail": true} },
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
| `capabilities()` | — | tiers, backends per task, loaded models |
| `ocr(OcrJob)` | memfd: grayscale/RGB crops + boxes, lang hints | lines `{text, bbox, conf}` per crop |
| `embed_text(batch)` | ≤ 32 texts | int8[256] vectors + space id |
| `embed_image(batch)` | memfd images | vectors |
| `rerank(query, docs)` | ≤ 30 docs | scores |
| `generate(GenerateJob)` | prompt template id + variables + JSON schema + optional image memfd | schema-valid JSON or `SchemaFailure` |
| `unload(task?)` | — | — |

Every job carries `{job_id, priority: interactive|normal|batch, deadline}`; inferd may reject with `Busy{retry_after}`.

## 13.6 Changes to existing CrownOS interfaces

| Repo | Change |
|---|---|
| crownos-protocols | Add `crownos_context_v1.xml`, `crownos_capture_privacy_v1.xml` |
| crownpositor | `compositor/src/context/` module; emit plugin-host window events; sensitivity veto in capture handlers; input buckets; pause hotkey |
| crownotify | `event NotificationPosted { id, app_name, desktop_entry, summary, body, category, urgency, t }` (appended field set) |
| crownos-config | `visual_context` section; extend `AiConfig` backends with `Qnn`, `TensorRt`, `Vulkan` |
| crownbar | Context engine indicator + pause menu (subscribes to `CaptureStateChanged`) |
| crownos-settings | Privacy pane (rules, retention, grants, cloud consent) — Phase 7+ |
| crownos-pkgbuilds | `crownos-visual-context` (+ `-cuda`, `-vulkan` ggml backends), `onnxruntime-crownos-qnn`; **aarch64 builds** |
