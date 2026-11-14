# CrownOS Visual Context Generation Engine (VCGE): Research and Implementation Blueprint

Status: **DRAFT FOR REVIEW**. Do not write any implementation code until this blueprint is approved.
Date: 2026-11-08
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
   - VCGE adds **one new privileged protocol, `crownos_context_v1`**. It carries:
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
3. **Use the cheapest signal that answers the question.** The cost ladder runs:

   metadata → accessibility tree / CrownUI MCP tree → OCR on changed crops → embeddings → small-LLM summary per *activity segment* → VLM only for low-text visual content → cloud only with consent.

   No screenshot or input event becomes an LLM request by itself.
4. **Two processes, plus optional helpers.**
   - `crown-contextd` holds the keys, the store, policy, correlation, and the API.
   - `crown-context-inferd` is sandboxed, has no network and no database access, and hosts the models. NPU and GPU driver crashes cannot take down the store or the compositor.
   - A cloud relay starts only when the user has consented.
5. **One storage engine.**
   - A single SQLCipher-encrypted SQLite database: event log, FTS5, sqlite-vec int8 vectors, a bi-temporal fact and edge graph in plain tables, and episodes.
   - Optional thumbnails live in an encrypted media directory.
   - There is no graph database server and no separate vector service. Kuzu is archived, Cozo is dormant, SurrealDB is BSL-licensed, and LanceDB is too heavy.
6. **Memory is append-plus-invalidate, with provenance on everything.**
   - Raw episodes are immutable.
   - Facts, entities, and edges are bi-temporal, following Graphiti's model.
   - Every derived memory links back to its evidence.
   - Deleting evidence cascades into the memories derived from it.
   - Seven memory types map onto five storage shapes (see [03-memory.md](03-memory.md)).
7. **Portable inference with per-machine accelerators.**
   - ONNX Runtime (`ort`) runs all fixed-graph models: OCR, embeddings, detectors. The execution provider is chosen per host: OpenVINO-NPU, VitisAI, QNN, CUDA/TensorRT, or CPU.
   - llama.cpp (`llama-cpp-2`) runs all generative models: CUDA on NVIDIA, Vulkan elsewhere.
   - Models are a data-driven registry, not code.
8. **Privacy is structural.**
   - The database is encrypted at rest, with a TPM-sealed key held by one broker process.
   - Every API caller is authenticated by pidfd and holds explicit scopes.
   - Raw content requires fresh user authentication.
   - Exclusions fail closed.
   - Capture is visible through a crownbar indicator, plus a compositor-level pause hotkey that works even if VCGE hangs.
   - Every query and every cloud upload is audit-logged.
   - Keylogging is never done: the compositor exports only coarse per-window key-class counts and suppresses even those in password fields.

### Hardware reality check

These facts drive the plan.

| Target | What works on Linux today | Plan |
|---|---|---|
| **Snapdragon X1 (X Elite/Plus)** | Oryon CPU is strong. The Adreno GPU works via Mesa Turnip/Vulkan [R]. **Using the NPU on Linux needs out-of-tree patches and firmware extracted from Windows. It is not shippable** [R]. | CPU (XNNPACK/KleidiAI) for OCR and embeddings. Vulkan for the LLM. The QNN NPU path sits behind an experimental flag. |
| **Snapdragon X2** | Qualcomm "Early Developer Preview" (Sept 2026). FastRPC upstreaming is in progress. The ORT QNN EP ships aarch64 Linux wheels [V/R]. | Same as X1. Re-evaluate the NPU path when upstream FastRPC lands (vendor target: 2027). |
| **Intel Core Ultra (LNL/ARL/PTL)** | NPU driver v1.38 and OpenVINO 2026.4 are production [V]. The NPU suits encoders, not LLM decode [R]. | NPU for OCR and embeddings; Arc iGPU (Vulkan/OpenVINO) for the LLM. |
| **AMD Ryzen AI 300 / Strix Halo** | `amdxdna` is mainline (6.14+). Ryzen AI 1.9 VitisAI EP on Linux [V]. | NPU for encoders; Vulkan iGPU for the LLM. |
| **NVIDIA dGPU** | CUDA/TensorRT, the best supported path [V]. | Everything on GPU, but yield to games and fullscreen content. |
| **DGX Spark (GB10, aarch64, 128 GB)** | CUDA 13 aarch64 works. llama.cpp benchmarks are published. NVFP4 is immature on SM121 [V/R]. | Workstation tier, **and a LAN "home cloud" offload target** for CrownOS laptops. |

**Prerequisite outside VCGE:** CrownOS packages build for x86_64 only. There are no aarch64 PKGBUILDs in crownos-pkgbuilds [V]. Snapdragon and DGX Spark support needs an aarch64 build and packaging track ([08 §Risks](08-implementation-plan.md#18-major-engineering-risks-and-mitigations)).

### What already exists in CrownOS and is reused

| Need | Existing piece | Change required |
|---|---|---|
| Window list, title, app_id | crownpositor `ext-foreign-toplevel-list` [V] | none |
| Focus, pid, sensitivity, input buckets | crownos_plugin_host_v1 declares window events but **never emits them** [V] | new `crownos_context_v1` protocol plus a shared window-event fan-out in the compositor |
| Per-window, damage-gated capture | `ext-image-copy-capture-v1` and `crownos_screencast_v1` [V] | add a sensitivity veto and a VCGE capture indicator |
| IPC | `crownos-ipc`: Unix socket, postcard, SO_PEERCRED, memfd fd passing, typed `protocol!` macro [V] | none; VCGE follows it |
| Config | `crownos-config`: RON sections, inotify subscribe, `/etc/crownos/ai.ron` backends [V] | add a `visual_context` section; add `Qnn`, `TensorRt`, and `Vulkan` to the AI backends |
| Clipboard | crownos-clipboard service, which honours the secret hint [V] | none (source-app metadata comes from the compositor instead) |
| Notifications | crownotify (fdo server) [V] | add a `NotificationPosted` event |
| pid → app resolution | crownos-process-monitor `monitor-system/src/apps/resolver.rs` [V] | reuse as a library |
| Inference daemon template | crownos-voice: ort `load-dynamic`, socket-activated user unit [V] | reuse the pattern |
| Semantic UI trees for CrownUI apps | crownui MCP (`ui://tree`) plus the `crownos_agent_access_v1` token [V] | consume as a first-class text source |

### Delivery

The work runs as ten phases (0–9). The **MVP is Phases 0–5 plus the search half of Phase 7**. It delivers:

- current context
- a searchable activity timeline with episode summaries
- full privacy controls
- an inspector CLI

Semantic, project, procedural, and preference memory, multimodal reasoning, and agent integration follow. See [08-implementation-plan.md](08-implementation-plan.md).

### Open decisions for the reviewer

1. **Pixel retention default.** Recommended: thumbnails at 1280 px max edge, kept 7 days, encrypted. Alternatives are "text only, no pixels" and "full frames, 30 days".
2. **Accessibility bus.** Recommended: enable `org.a11y.Status.IsEnabled` session-wide when VCGE is on. This has a global CPU and memory cost for every toolkit app. The alternative is a per-app allowlist with Chromium/Electron launch flags.
3. **Cloud offload targets.** Recommended: a user-owned DGX Spark on the LAN first, third-party APIs second, both opt-in.
4. **Compositor privilege identity.** Should `crownos_context_v1` be restricted to a pidfd-verified `crown-contextd` executable, or only to "unsandboxed clients"? Recommended: pidfd plus exe path, revisited when `crownos_session_v1` (the compositor running as its own user) lands.
5. **aarch64 packaging.** Is it in scope for the same milestone? This is required for Snapdragon and Spark.
