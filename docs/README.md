# CrownOS Visual Context Generation Engine (VCGE): Research and Implementation Blueprint

Status: **DRAFT, REVISED 2026-11-23**. The user answered the open decisions on 2026-11-23 (see [Decisions](#decisions-2026-11-23)).
Date: 2026-11-08. Revised: 2026-11-23.
Scope: research report, architecture, and a phased implementation plan.

## How to read this

| # | Deliverable | File |
|---|---|---|
| 1 | Executive summary | this file |
| 2 | Existing solution comparison | [01-research.md §1](01-research.md#1-existing-solutions) |
| 3 | Linux and Smithay integration research | [01-research.md §2](01-research.md#2-linux-wayland-and-smithay-integration) |
| 4–7 | Architecture, diagrams, components, data flows | [02-architecture.md](02-architecture.md) |
| 8–9 | Memory taxonomy, schemas, database and indexing | [03-memory.md](03-memory.md) |
| 10 | Models and inference, per-hardware matrix | [04-inference.md](04-inference.md) |
| 11 | Privacy and security architecture | [05-privacy-security.md](05-privacy-security.md) |
| 12 | Performance targets and resource profiles | [06-performance.md](06-performance.md) |
| 13 | IPC and API specifications | [07-api.md](07-api.md) |
| 14–19 | Crate structure, phases, dependencies, testing, risks, MVP and roadmap | [08-implementation-plan.md](08-implementation-plan.md) |

Every factual claim carries a confidence tag:

| Tag | Meaning |
|---|---|
| **[V]** | Verified from source code, official docs, or a manifest. This includes the CrownOS repos, which were read directly. |
| **[R]** | Reported by a credible secondary source and not re-verified. |
| **[A]** | Architectural assumption or design decision. It must be validated by a benchmark or spike, listed in [08 §Phase 0](08-implementation-plan.md#phase-0--foundations-and-de-risking-spikes). |
| **[V vendor]** | Read in a primary source, but the number comes from the vendor's own benchmark. |
| **[A, derived]** | Our own calculation from published numbers. Not measured. |

---

## 1. Executive summary

VCGE is a CrownOS subsystem, not an application. It observes the desktop through the compositor, the accessibility bus, and system services. It then builds an encrypted, inspectable memory of what the user does, and serves that memory to CrownOS agents and the shell through one typed API.

### The design in eight decisions

1. **The compositor is the sensor and the enforcement point. It never does inference.**
   - crownpositor already implements most of what VCGE needs [V]:
     - `ext-image-copy-capture-v1` (shm and dmabuf, damage-gated)
     - `crownos_screencast_v1`, which emits per-frame damage rects
     - `ext-foreign-toplevel-list`
     - ext-data-control
     - idle-notify
     - text-input-v3
     - security-context privilege gating
   - VCGE adds **one new protocol, `crownos_context_v1`**. Since version 2, any app with a grant may bind it. It carries:
     - focus
     - pid and pidfd
     - window sensitivity
     - aggregated input-activity buckets
     - clipboard metadata
     - capture pause and veto
   - The compositor refuses capture of sensitive windows, so privacy is enforced before a pixel ever leaves it.
2. **Capture on events, not on a clock.** Captures are triggered by:
   - focus or title change
   - a typing pause
   - scroll stop
   - copy
   - a damage burst that has since settled
   - a long idle backstop

   Pixels are pulled one frame at a time through `ext-image-copy-capture`, cropped to the window, and limited to damaged tiles. Expected volume: 0.5–3 captures per minute of active work, never a video stream.

   **Analyse, then delete.** Each capture is analysed at capture time. VCGE keeps the extracted text and metadata, then deletes the pixels at once. No pixel goes to disk. See [05 §11.6](05-privacy-security.md#116-analyse-then-delete).
3. **Use the cheapest signal that answers the question.** The cost ladder runs:

   metadata → accessibility tree / CrownUI MCP tree → OCR on changed crops → short VLM caption, at capture time, only for frames with little text → text embeddings → small-LLM summary per *activity segment* → LAN or cloud, text only, only with consent.

   No screenshot or input event becomes an LLM request by itself.
4. **Two processes, plus optional helpers.**
   - `crown-contextd` holds the keys, the store, policy, correlation, and the API.
   - `crown-context-inferd` is sandboxed, has no network and no database access, and hosts the local models. NPU and GPU driver crashes cannot take down the store or the compositor.
   - `crown-context-relayd` hosts the OpenAI-compatible HTTP backend (local sidecars, a LAN node, cloud). It starts only when the user has configured such a backend, and it gets text only.
5. **One storage engine.**
   - A single SQLCipher-encrypted SQLite database: event log, FTS5, sqlite-vec int8 vectors, a bi-temporal fact and edge graph in plain tables, and episodes.
   - No pixels are stored, so there is no media directory.
   - There is no graph database server and no separate vector service. Kuzu is archived, Cozo is dormant, SurrealDB is BSL-licensed, and LanceDB is too heavy.
6. **Memory is append-plus-invalidate, with provenance on everything.**
   - Raw episodes are immutable.
   - Facts, entities, and edges are bi-temporal, following Graphiti's model.
   - Every derived memory links back to its evidence.
   - Deleting evidence cascades into the memories derived from it.
   - Seven memory types map onto five storage shapes (see [03-memory.md](03-memory.md)).
7. **A swappable model layer.**
   - `context-infer` defines one trait per modality: `Generate`, `Embed`, `Rerank`, `Ocr`, `Transcribe`. A task layer sits on top. Callers ask for a task and never name a model.
   - Backend crates implement the traits: `context-infer-ort` (OCR, embeddings, rerank), `context-infer-llama` (LLM and VLM on CUDA, Vulkan, ROCm or CPU), `context-infer-http` (OpenAI-compatible sidecars, LAN and cloud).
   - `models/registry.ron` v2 lists models with capability tags and per-runtime artifacts. A router picks a model, runtime and device per task from a probe and a cached benchmark.
   - To swap a model, edit the registry. To add a runtime, add a backend crate. See [04 §10.4](04-inference.md#104-model-layer-in-context-infer).
8. **Privacy is structural.**
   - The database is encrypted at rest, with a TPM-sealed key held by one broker process.
   - Any app may use `crownos_context_v1` and the Context API. Each caller gets an `AppKey` and holds only the scopes the user granted in `context_access.ron` (crownos-settings → Privacy). For sandboxed apps this is enforcement. For unsandboxed host apps it is consent, not enforcement, because they already have screencopy.
   - Raw content requires fresh user authentication.
   - Exclusions fail closed.
   - Capture is visible through a crownbar indicator, plus a compositor-level pause hotkey that works even if VCGE hangs.
   - Every query and every cloud upload is audit-logged.
   - Keylogging is never done: the compositor exports only coarse per-window key-class counts and suppresses even those in password fields.

### Hardware reality check

These facts drive the plan.

| Target | What works on Linux today | Plan |
|---|---|---|
| **Snapdragon X1 (X Elite/Plus)** (future track) | Oryon CPU is strong. The Adreno GPU works via Mesa Turnip/Vulkan [R]. **Using the NPU on Linux needs out-of-tree patches and firmware extracted from Windows. It is not shippable** [R]. | CPU (XNNPACK/KleidiAI) for OCR and embeddings. Vulkan for the LLM. The QNN NPU path sits behind an experimental flag. |
| **Snapdragon X2** (future track) | Qualcomm "Early Developer Preview" (Sept 2026). FastRPC upstreaming is in progress. The ORT QNN EP ships aarch64 Linux wheels [V/R]. | Same as X1. Re-evaluate the NPU path when upstream FastRPC lands (vendor target: 2027). |
| **Intel Core Ultra (LNL/ARL/PTL)** | NPU driver v1.38 and OpenVINO 2026.4 are production [V]. The NPU suits encoders, not LLM decode [R]. | NPU for OCR and embeddings; Arc iGPU (Vulkan/OpenVINO) for the LLM. |
| **AMD Ryzen AI 300 / Strix Halo** | `amdxdna` is mainline (6.14+). Ryzen AI 1.9 VitisAI EP on Linux [V]. | NPU for encoders; Vulkan iGPU for the LLM. |
| **NVIDIA dGPU** | CUDA/TensorRT, the best supported path [V]. | Everything on GPU, but yield to games and fullscreen content. |
| **DGX Spark (GB10, aarch64, 128 GB)** | CUDA 13 aarch64 works. llama.cpp benchmarks are published. NVFP4 is immature on SM121 [V/R]. | Now: a **LAN "home cloud" target** through the HTTP backend (it runs its own OS and llama-server). Later: a CrownOS workstation tier. |

**aarch64 is a future track.** CrownOS packages build for x86_64 only. There are no aarch64 PKGBUILDs in crownos-pkgbuilds [V]. CrownOS on Snapdragon or on DGX Spark needs an aarch64 build and packaging track. It is not an MVP gate ([08 §Risks](08-implementation-plan.md#18-major-engineering-risks-and-mitigations)). A Spark on the LAN needs no CrownOS aarch64 package, because VCGE reaches it over HTTP.

### What already exists in CrownOS and is reused

| Need | Existing piece | Change required |
|---|---|---|
| Window list, title, app_id | crownpositor `ext-foreign-toplevel-list` [V] | none |
| Focus, pid, sensitivity, input buckets | crownos_plugin_host_v1 declares window events but **never emits them** [V] | new `crownos_context_v1` protocol plus a shared window-event fan-out in the compositor |
| Per-window, damage-gated capture | `ext-image-copy-capture-v1` and `crownos_screencast_v1` [V] | add a sensitivity veto and a VCGE capture indicator |
| IPC | `crownos-ipc`: Unix socket, postcard, SO_PEERCRED, memfd fd passing, typed `protocol!` macro [V] | read the peer with `SO_PEERPIDFD`, add an opt-in `SO_PASSPIDFD` sender check, and pass the peer identity to handlers |
| Config | `crownos-config`: RON sections, inotify subscribe, `/etc/crownos/ai.ron` backends [V] | add `visual_context` and `context_access` sections; split `Device` from `Runtime` in the AI backends and add `Vulkan`, `TensorRt`, `Rocm`, `Remote` |
| Secrets | crownos-secrets-manager (separate component) | interface only: it flags a window sensitive before it types the first key of a secret, so VCGE never captures it |
| Clipboard | crownos-clipboard service, which honours the secret hint [V] | none (source-app metadata comes from the compositor instead) |
| Notifications | crownotify (fdo server) [V] | add a `NotificationPosted` event |
| pid → app resolution | crownos-process-monitor `monitor-system/src/apps/resolver.rs` [V] | reuse as a library |
| Inference daemon template | crownos-voice: ort `load-dynamic`, socket-activated user unit [V] | reuse the pattern |
| Semantic UI trees for CrownUI apps | crownui MCP (`ui://tree`) plus the `crownos_agent_access_v1` token [V] | consume as a first-class text source |

### Delivery

The work runs as parallel workstreams (access control, model layer, capture, accessibility, benchmarks) mapped onto ten phases (0–9). The **MVP is Phases 0–5 plus the search half of Phase 7**. It delivers:

- current context
- a searchable activity timeline with episode summaries
- capture-time text extraction, with a short VLM caption for low-text frames where the hardware allows it
- full privacy controls and per-app access control
- an inspector CLI

Semantic, project, procedural, and preference memory, UI element detection, and agent integration follow. See [08-implementation-plan.md](08-implementation-plan.md).

### Decisions (2026-11-23)

The user answered the five open decisions of the first review.

| # | Question | Decision | Where it is applied |
|---|---|---|---|
| a | Pixel retention | **Analyse each capture at capture time, extract everything, then delete the pixels at once.** No thumbnails, no media directory, no image embeddings, no visual search, no VLM at segment close, no re-analysis of images after a model change. On battery, OCR still runs at capture time; only the VLM is skipped. | [02 §7](02-architecture.md#7-event-and-data-flow-diagrams), [05 §11.6](05-privacy-security.md#116-analyse-then-delete), [06 §12.3](06-performance.md#123-adaptive-processing-scheduler-policy) |
| b | Accessibility bus | **Session-wide.** `org.a11y.Status.IsEnabled` is set at session start, before apps launch. Reads are event-driven, then `GetMatches` (Qt, ATK bridge) or `GetItems` (GTK4) or a bounded concurrent walk. Chromium gets a call allowlist. GTK4 ignores `IsEnabled` and emits all events once any listener exists. | [01 §2.5](01-research.md#25-accessibility-at-spi2), [01 §4.2](01-research.md#42-b-session-wide-accessibility) |
| c | Cloud offload order | **Replaced by a modular, swappable model layer.** Traits per modality, a task layer that never names a model, `registry.ron` v2, a router with probe, cached benchmark and circuit breakers. Backends: ort, llama.cpp, OpenAI-compatible HTTP. LAN and cloud stay opt-in through the HTTP backend. | [04 §10.4](04-inference.md#104-model-layer-in-context-infer) |
| d | Who may use `crownos_context_v1` | **Any app, with access control** in crownos-settings and crownos-config (`context_access.ron`). `AppKey` identity, scopes, first-use consent through the portal, live revocation, protocol version 2 with many binders. Consent for unsandboxed host apps, enforcement for sandboxed apps. | [05 §11.3](05-privacy-security.md#113-caller-identity-and-scopes), [07](07-api.md) |
| e | aarch64 packaging | **Later.** A future track, not an MVP gate. | [08 §19](08-implementation-plan.md#19-mvp-scope-and-long-term-roadmap) |

**Related component:** crownos-secrets-manager stores passwords and API keys and lets agents use them by name. VCGE uses it as an interface only: the secrets daemon has the compositor flag the target window sensitive before the first key, so VCGE never captures an injected secret.
