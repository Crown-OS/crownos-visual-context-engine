# 01: Research

Tags: **[V]** verified from source or official docs · **[R]** reported by a secondary source · **[A]** assumption or design inference.
Snapshot date: 2026-11-08. Section 4 adds the approach research of 2026-11-23.

- Reference repos were cloned and read at these commits: screenpipe `32e9049`, aw-server-rust, awatcher, openrecall, sqlite-vec, Smithay `master@65aa86f`, niri `da0ab81`, cosmic-comp `46b26e6`.
- CrownOS repos were read in place under `/e/CrownOS/`.

---

## 1. Existing solutions

### 1.1 Comparison

| System | Status | Capture trigger and dedup | Text and vision | Storage | Retrieval | Privacy model | License |
|---|---|---|---|---|---|---|---|
| **Microsoft Recall** | Shipping on Copilot+ PCs [V] | A snapshot is taken when the screen differs from the previous one; the interval is unpublished [V] | Local OCR plus a "Semantic Index" of image and text vectors, on a 40+ TOPS NPU [V] | Encrypted snapshot store with a key per snapshot and an encrypted vector DB. Quota of 10–150 GB [V] | Natural-language search, executed inside a VBS enclave [V] | VBS enclave, TPM-sealed keys, Windows Hello gating. App, URL and private-browsing filters, a sensitive-info classifier, and DLP [V] | Proprietary |
| **Rewind / Limitless** | Acquired by Meta in December 2025; capture disabled 2025-12-19 [R] | Unknown | Apple Vision OCR [R] | SQLCipher DB plus `chunks/` of video (`frame`, `node`, `segment` tables) [R] | FTS [R] | SQLCipher with the key held by the app [R] | Proprietary, discontinued |
| **Screenpipe** | Very active. **License changed from MIT to a source-available commercial license** [V] | Event-driven: app switch, focus, click, typing pause, scroll stop, clipboard, visual change, 30 s idle. 200 ms debounce. Dedup is a downscaled Hellinger histogram plus a hash early exit [V] | The accessibility tree first. Then an OCR gate: text-region detection, crop hash, and OCR only on change. Tesseract CLI, Apple Vision or Windows OCR [V] | SQLite WAL with a single `frames` table and FTS5 external content. JPEG frames are compacted into MP4 after 10 minutes [V] | FTS5 only. **The unused OCR embedding table was dropped** [V] | Fail-closed window exclusion, incognito-title detection in 20+ locales, DRM pause, async PII redaction. The vault encrypts **only on manual lock** [V] | Source-available, commercial |
| **ActivityWatch** | Active [V] | Watcher heartbeats are merged when the data is equal and the gap is ≤ `pulsetime` [V] | None | SQLite `buckets` and `events(start, end, data JSON)` [V] | aw-query DSL [V] | Local only, unencrypted [V] | MPL-2.0 |
| **OpenRecall** | Dormant since September 2025 [V] | 3 s poll, skipped if global MSSIM ≥ 0.9 [V] | docTR OCR and MiniLM embeddings [V] | SQLite with embedding BLOBs and WebP files [V] | Brute-force cosine search over all rows in NumPy [V] | None [V] | AGPL-3.0 |
| **Mem0** | Active [V] | Extracts memories from conversations | Not applicable | Vector store plus an optional graph [V] | Hybrid semantic, BM25 and entity search [V] | Not applicable | Apache-2.0 |
| **Zep / Graphiti** | Active [V] | Episodes go into a bi-temporal knowledge graph [V] | Not applicable | Neo4j, FalkorDB or Neptune. Kuzu support was deprecated [V] | Cosine, BM25 and BFS, then RRF/MMR or a cross-encoder [V] | Not applicable | Apache-2.0 |
| **Letta (MemGPT)** | Active [V] | Not applicable | Not applicable | Core blocks, archival vectors, recall history, and MemFS (git-backed) [V] | Agent-driven [V] | Not applicable | Apache-2.0 |
| **LangMem / LangGraph store** | Active [V] | Not applicable | Not applicable | Namespaced KV with vector search [V] | Key lookup, semantic search, and filters [V] | Not applicable | MIT |

#### Mem0 and Graphiti on memory benchmarks [V]

- **Mem0 paper** (arXiv [2504.19413](https://arxiv.org/abs/2504.19413)):
  - An LLM chooses ADD, UPDATE, DELETE or NOOP against the top 10 similar memories.
  - LOCOMO J scores: Mem0 66.9, Zep 66.0, full context 72.9.
  - Zep disputes these numbers [R].
  - **In April 2026, Mem0 moved to "ADD-only extraction; nothing is overwritten" plus hybrid entity-linked retrieval** [V, README].
- **Graphiti paper** (arXiv [2501.13956](https://arxiv.org/abs/2501.13956)):
  - Episode, entity and community tiers.
  - Bi-temporal edges: `t_valid`/`t_invalid` for event time, and `created`/`expired` for transaction time. A contradiction closes the old edge instead of deleting it.
  - Retrieval combines BM25, vectors and BFS, then reranks.
  - LongMemEval_S: 71.2 vs 60.2 for full context, at about 1.6k vs 115k context tokens.

#### Per-system notes

**Microsoft Recall**
- Docs: [manage-recall](https://learn.microsoft.com/en-us/windows/client-management/manage-recall) and the [security architecture blog post](https://blogs.windows.com/windowsexperience/2024/09/27/update-on-recall-security-and-privacy-architecture/).
- The 2024 preview stored OCR text in a plaintext SQLite file (`ukg.db`), which was exposed by "TotalRecall" [R].
- The redesign moved the keys and the index into a VBS enclave and made the UI untrusted.
- April 2026, "TotalRecall Reloaded" [R]: DLL injection into the *consumer UI process* (AIXHost.exe) reads results after the user authenticates with Hello. The enclave itself held.
- **Lesson:** protect the consumer as well as the store.

**Screenpipe** ([repo](https://github.com/screenpipe/screenpipe))
- Notable files:
  - `crates/screenpipe-engine/src/event_driven_capture.rs` (capture triggers, 200 ms debounce, 30 s idle backstop that relaxes to 300 s on battery)
  - `screenpipe-screen/src/frame_comparison.rs`
  - `screenpipe-capture/src/ocr_gate.rs`
- Dedup history:
  - Downscaling by 6 made scrolling through text invisible to dedup.
  - Dropping SSIM in favour of a Hellinger histogram saved 40–50% CPU.
  - Dedup state is committed only after a durable write.
- Storage history: it took about 120 migrations to reach a frames-centric schema.
- Linux:
  - It uses grim or portal ScreenCast; neither uses compositor damage.
  - It has **no VAAPI path**, so video encoding falls back to x264 in software.
  - Toggling portal streams per frame makes KDE's recording indicator flash (#7758).
- Resources: the docs claim 5–10% CPU and 0.5–3 GB RAM. Issues report pathological cases, such as a battery drain from repeated 7-day summary queries (#7364).

**ActivityWatch**
- Heartbeat merge is in `aw-transform/src/heartbeat.rs`.
- Wayland watchers:
  - `aw-watcher-window-wayland` supports only wlr-foreign-toplevel.
  - awatcher covers wlr, COSMIC, KDE and GNOME.

**Rewind**
- The "3,750×" compression claim is long-GOP video of mostly static screens compared against raw RGBA [R].
- Its schema split is worth copying: `segment` (app/window interval) → `frame` (pointer into video) → `node` (text span plus box).

**Letta**
- Sleep-time compute ([arXiv 2504.13171](https://arxiv.org/abs/2504.13171)): background agents consolidate memory between interactions.

### 1.2 Patterns adopted

1. **Event-driven capture** (screenpipe). VCGE does better because it *is* the compositor: it uses real per-surface damage, not histogram diffs.
2. **Accessibility first, OCR second, OCR only on changed text crops** (screenpipe). The CrownUI MCP tree is used ahead of both.
3. **Heartbeat merge** for the focus/app/title/URL timeline (ActivityWatch). A day of activity is a few thousand rows.
4. **Segment → frame → text-span schema** (Rewind), with FTS5 external content from day one (screenpipe's hard-won end state).
5. **Immutable episodes, async extraction, bi-temporal invalidation, hybrid retrieval with RRF** (Graphiti, Mem0 v3, LangMem). This convergence is consistent across all serious memory systems.
6. **Consolidation in idle time** (Letta sleep-time compute), on AC power.
7. **Recall's trust split.** A broker holds the keys, the UI is untrusted, raw queries need fresh authentication, and window-level redaction is used instead of dropping whole frames.
8. **Fail-closed exclusions** plus localized private-browsing detection (screenpipe), *enforced in the compositor*.

### 1.3 Pitfalls avoided

| Pitfall | Seen in | VCGE countermeasure |
|---|---|---|
| Plaintext DB or encryption only on manual lock | Recall 2024, screenpipe vault | SQLCipher always on; key held only by `crown-contextd` |
| Injectable consumer process | TotalRecall Reloaded | Raw-content scopes require fresh auth per session; pidfd caller verification; rate limits; audit log |
| Fixed-interval polling and full-frame OCR | OpenRecall, old screenpipe | Damage- and event-gated capture; OCR only on changed text crops |
| Embedding every frame | screenpipe (dropped the table), OpenRecall (O(N) scan) | Embed deduplicated *segments* only |
| LLM-driven destructive updates | original Mem0 | Append plus invalidate; nothing is silently overwritten |
| Unbounded background queries draining the battery | screenpipe #7364 | Budgeted scheduler; cached derived views; power-aware |
| Dependence on dormant or acquired stores | Kuzu (archived 2025-10, Apple), Cozo (dormant), Rewind (shut down) | SQLite as source of truth; rebuildable indexes; open export |
| Copying code under a restrictive license | screenpipe (commercial), OpenRecall (AGPL) | Ideas only. MPL/Apache sources used at file level with attribution |

---

## 2. Linux, Wayland and Smithay integration

### 2.1 State of crownpositor (the live CrownOS compositor) [V]

**Basics**
- Repo: `/e/CrownOS/crownpositor`, a workspace containing `compositor`, `config`, `protocols`, `lockscreen` and others.
- `/e/CrownOS/Compositor` is an unrelated macOS app, and `crowndesktop/compositor` is archived.
- Versions:
  - Smithay 0.7.0, patched to git rev `e3d461a` (for ext-data-control fixes)
  - calloop 0.14.4, tokio 1.53, zbus 5.19
  - GLES renderer only. Backends: udev/DRM/GBM/libinput/libseat, plus winit.

**Event loop** (`compositor/src/lib.rs:21-80`)
- One calloop loop. A per-iteration reconcile runs `update_keyboard_focus`, `refresh_foreign_toplevels`, redraws, then `render_offscreen` (capture).
- Async work goes to a 2-worker tokio pool, and completions return over a calloop channel (`utils/runtime.rs`).
- **Focus has a single choke point:** `update_keyboard_focus()` in `state/actions.rs:177`. It is the only caller of `set_focus`.

**Privilege model**
- A client is privileged unless it connected through a `wp_security_context_v1` listener (`utils/privilege.rs`).

**Capture**
- `ext-image-capture-source` (output and toplevel) plus `ext-image-copy-capture-v1` (shm via GPU readback, and dmabuf).
- `zwlr_screencopy_v1`.
- `crownos_screencast_v1`: a dmabuf ring, NV12 or RGB, **per-frame damage rects**, `max_fps` cap, syncobj.
- Rendering happens only when the source is damaged and a ring slot is free.
- Window captures keep receiving commits while the window is off-screen (`State::damage_window_captures`).

**Other protocols already present**
- ext-foreign-toplevel-list (title and app_id)
- ext-data-control and wlr-data-control
- idle-notify, session-lock
- text-input-v3, input-method-v2
- ext-workspace, content-type, presentation, security-context
- XWayland via xwayland-satellite, so X11 windows appear as ordinary toplevels

**Gaps that matter for VCGE**
- No client-visible focus state:
  - wlr-foreign-toplevel-management is absent.
  - `crownos_plugin_host_v1` declares `window_focused`, but the compositor **never emits** it (`protocols/src/crownos_plugin_host/mod.rs:58,97`).
- No pid in any protocol.
- AT-SPI state is commented out.
- crownotify has no "notification posted" event.
- The compositor has no IPC server. Its whole external API is Wayland globals, which is fine for VCGE.

### 2.2 Smithay building blocks [V]

**Versions**
- Smithay's crates.io release is still 0.7.0 (2025-06).
- `master` has heavy unreleased changes, such as `Dispatch2` and capture modules.
- niri and cosmic-comp pin git revisions. **Pin a rev.**

**Hooks VCGE relies on**

| Need | Smithay API | Reference implementations |
|---|---|---|
| Active window | `SeatHandler::focus_changed`. In crownpositor, `update_keyboard_focus()` is the natural point. | niri `WindowFocusChanged` IPC event |
| Title and app_id | `XdgShellHandler::{title_changed, app_id_changed}`, `XdgToplevelSurfaceData` | niri `ipc/server.rs` |
| Stable window ids | `ForeignToplevelListState::new_toplevel_with_identifier`, `xdg_toplevel_tag` | niri `protocols/foreign_toplevel.rs` |
| Capture | `wayland::image_copy_capture` and `image_capture_source` (on master; crownpositor already has them). `capture_constraints() -> None` vetoes a source. `Frame::success(.., damage)` | cosmic-comp `handlers/image_copy_capture/render.rs`; niri `handlers/image_copy_capture.rs` |
| Damage without rendering | `OutputDamageTracker::damage_output(age, elements)` returns `None` when nothing changed, so the frame is skipped | niri `screencasting/pw_utils.rs` |
| Per-element damage | `Element::damage_since(scale, commit)` | |
| Password fields | `TextInputActivation::activated(Some((hint, ContentPurpose::Password\|Pin)))` | cosmic-comp `handlers/text_input.rs` |
| Idle | `IdleNotifierState` | |
| Sandboxed clients | `SecurityContext { sandbox_engine, app_id, instance_id }` | niri `get_credentials_for_client`, cosmic-comp `client_not_sandboxed` filter |
| PID | `Client::get_credentials()` (SO_PEERCRED). `SO_PEERPIDFD` (Linux 6.5+) can be read from the stream before `insert_client` to get a race-free pidfd [A] | |
| Clipboard metadata | `SelectionHandler::new_selection` (source, mime types, client) | |
| Content type | `wp_content_type_v1` (none, photo, video, game) | Use it to skip OCR on video and game surfaces [A] |

### 2.3 How reference compositors expose context [V]

- **niri:**
  - JSON event stream on `$XDG_RUNTIME_DIR/niri.*.sock`. Events include `WindowOpenedOrChanged`, `WindowFocusChanged` and `CastsChanged`. `Window` carries `pid`, `app_id`, `title`, `workspace_id` and `focus_timestamp`.
  - Implements the Mutter ScreenCast D-Bus API and owns its PipeWire streams.
- **cosmic-comp:**
  - No socket IPC. Shell components are privileged Wayland clients using cosmic-protocols, gated by security-context.
  - Screencast goes through ext-image-copy-capture.
- **Hyprland:** `socket2` line events such as `activewindowv2` and `screencast`.

**Conclusion [A]:**
- Engine-grade consumers should be **privileged Wayland clients**. That gives object lifetimes tied to toplevel handles, fd passing, and no second server inside the compositor.
- PipeWire and the portal stay for third-party screencasts only. They add a daemon hop, consent UX, and video semantics that VCGE does not need.

### 2.4 Input observation without keylogging

**Platform facts**
- Wayland gives no client global input monitoring [V]. Only the compositor sees libinput events (`smithay::backend::input::InputEvent`).
- The `InputCapture` portal is barrier-triggered and consent-based. It is not a passive monitor [V].

**Design [A]:** the compositor aggregates input in-process and emits buckets every 2 s per focused toplevel.

| Field | Content |
|---|---|
| `text_keys` | count of printable key presses (identity never recorded) |
| `edit_keys` | Backspace and Delete |
| `nav_keys` | arrows, Page Up/Down, Home, End |
| `chords` | modifier combinations, as a count only |
| `clicks` | per button class |
| `scroll` | ticks, plus `scroll_stopped` |
| `pointer_travel` | distance bucket |
| `gestures`, `touch` | counts |
| derived edges | `typing_started` and `typing_paused` (1.5 s gap) |

Rules:
- No key identity, no per-event timestamps. Per-event timing enables keystroke-dynamics inference.
- Text-class counts are zeroed while the focused text-input purpose is Password or Pin, while the session is locked, or while the window is sensitive.
- The cost is one counter increment per event, and the raw stream never leaves the compositor.
- **Typed text is never collected from key events.** Where the user explicitly allows it per app, committed text comes from the accessibility text of focused editable fields, never from password-role nodes.

### 2.5 Accessibility (AT-SPI2)

**Platform facts**
- The `atspi` crate 0.30.0 (odilia-app) is zbus-based, async, and mature [V].
- Qt6, Chromium and Firefox build accessibility trees when `org.a11y.Status.IsEnabled` is true [V]. Chromium reads it only at startup [V].
- GTK4 ignores `IsEnabled`. It always connects to the bus and emits every event once any listener is registered [V].
- Chromium and Electron also accept the `ACCESSIBILITY_ENABLED` environment variable [V]. A bare `--force-renderer-accessibility` forces the "complete" mode, so VCGE does not use it [V].
- Newton, the Wayland-native accessibility work, is not production ready; the last status post is from 2024 [V].

**Coverage**
- Good: GTK3/4, Qt5/6, Firefox, Chromium/Electron (with the flag).
- Poor: most terminals, Flutter, Java without the bridge, and games [R].

**Strategy [A]** (decision b, 2026-11-23; comparison in [§4.2](#42-b-session-wide-accessibility))
- Set `IsEnabled=true` session-wide at session start, before any app launches.
- Event-driven. Subscribe only to focus, `window:activate`, `document:load-complete`, accessible-name changes and `object:text-changed`.
- Fetch text with `Collection.GetMatches` (Qt6, ATK bridge), `Cache.GetItems` (GTK4), or else a concurrent bounded walk (≤ 2,000 nodes, 200 ms per app). Never walk whole trees one call at a time: each call is a D-Bus round trip, and browser pages can hold 10⁴–10⁵ nodes.
- Chromium call allowlist: role, name, state, children, `Text.GetText(0,-1)`, document attributes. Never line or word boundaries, extents, `GetAttributes` or `GetRelationSet`, because they raise Chromium's accessibility mode.
- Filter by the SHOWING/VISIBLE states, so off-screen text is not collected.
- Skip `Role::PasswordText` nodes. A password-role focus also notifies the compositor so it marks the window sensitive.
- Map an accessibility app to a compositor window by **pid plus title**. Wayland gives no global coordinates, so extents are unreliable.
- Browser URL sources, in order: the `DocURL` document attribute [R, verify per version], then an optional native-messaging extension, then the title.
- **CrownUI apps** expose `ui://tree` over MCP, gated by `crownos_agent_access_v1` [V]. This is better than AT-SPI or OCR, so it is used first.

### 2.6 System signals

| Signal | Source | Notes |
|---|---|---|
| Notifications | crownotify, which is the fdo server [V] | Add `event NotificationPosted { id, app_name, desktop_entry, summary, body, category, urgency }`. D-Bus `BecomeMonitor` is a fallback only. |
| Clipboard | Compositor `new_selection` metadata (source window, mime types, secret hint). Content via the crownos-clipboard service `get(id)` only for allowed sources [V] | crownos-clipboard already drops `x-kde-passwordManagerHint: secret` entries [V]. |
| Process and app identity | pidfd → `/proc/<pid>/cgroup` (`app-<launcher>-<appid>-<n>.scope`), plus crownos-process-monitor's resolver [V] | Read lazily on focus change. |
| Terminal cwd and project | Terminal pid → shell → `tpgid` from `/proc/<shell>/stat` → `/proc/<tpgid>/cwd` → nearest `.git` [A] | Gives the project root without OSC 7. |
| Files | `~/.local/share/recently-used.xbel` via inotify; `/proc/<pid>/fd` of the focused app on focus change [A] | Unprivileged fanotify is limited to inode marks, with no pid [V]. Whole-home monitoring is not proposed. |
| Idle and lock | Compositor idle-notify and session lock [V] | All capture pauses. |
| Power and thermal | UPower, power-profiles-daemon, `/proc/pressure/{cpu,memory,io}` (PSI), NVML for dGPU [A] | Feeds the scheduler budgets. |

### 2.7 Private-browsing detection [V/A]

- **Firefox:** the title suffix "— Mozilla Firefox Private Browsing" is historically the only signal [V]. Whether private windows get a distinct app_id on Wayland is unverified [A].
- **Chromium:** no reliable title signal on Linux [A]. Detect it with an optional native-messaging extension (`inIncognitoContext`) or accessibility window naming.
- **Policy:** detection is layered and fails closed. The layers are the extension signal, then per-locale title rules, then user deny patterns. The verdict feeds the **compositor's capture veto**, so a private window is never rendered for VCGE.

### 2.8 Boundary decision

| In the compositor (crownpositor). Non-blocking, allocation-free on hot paths, no inference. | In VCGE services |
|---|---|
| Window event fan-out: focus, map/unmap, title, app_id, geometry, workspace, fullscreen, content-type | Correlation, segmentation, classification |
| pid and pidfd per client; security-context identity | Accessibility worker (`atspi`), CrownUI MCP reader |
| **Sensitivity state machine and capture veto** | Capture client, tile diffing, OCR, embeddings, LLM, VLM |
| Input-activity bucket aggregation | Store, retrieval, API, policy engine, audit |
| Clipboard selection metadata | Notification, clipboard content, process and file workers |
| Global pause hotkey and capture indicator state | Consolidation |

The compositor-to-VCGE transport is **one privileged Wayland connection** carrying:
- `ext-foreign-toplevel-list` (ids)
- `ext-image-copy-capture-v1` with toplevel and output sources (pull-based frames with damage)
- the new `crownos_context_v1` (everything else)

Details are in [02-architecture.md §5](02-architecture.md#5-compositor-bridge-crownos_context_v1) and [07-api.md](07-api.md).

---

## 3. Hardware and inference research summary

The full matrix is in [04-inference.md](04-inference.md). Key verified points:

**Qualcomm (Snapdragon X)**
- The ORT QNN EP is now a Qualcomm-maintained plugin EP ([onnxruntime-qnn](https://github.com/onnxruntime/onnxruntime-qnn)), v2.7.0 for QAIRT 2.51, with aarch64 Linux wheels [V].
- The Snapdragon X1 NPU runs on Linux only with a FastRPC patch and firmware extracted from Windows ([write-up](https://github.com/hastyheat-com/x1e-yoga-slim7x-npu-linux)) [R].
- On the same machine, Llama-3.2-1B decode ran at **11.7 tok/s on the NPU vs 37.8 tok/s on Adreno Vulkan** [R]. The GPU path is both faster and shippable.
- llama.cpp `ggml-hexagon` hangs on mainline because DSPSIGNAL support is missing [R].
- X2: Qualcomm's Linux developer preview arrived in September 2026; the Ubuntu-certified target is 2027 [R].

**Intel**
- [linux-npu-driver](https://github.com/intel/linux-npu-driver/releases) v1.38 and OpenVINO 2026.4.1 support LNL, ARL and PTL NPUs [V].
- NPU LLM decode is weak (about 2 tok/s on 1B), so use the NPU for static-shape encoders [R].

**AMD**
- `amdxdna` is mainline since 6.14 [V].
- [Ryzen AI 1.9 on Linux](https://ryzenai.docs.amd.com/en/latest/linux.html) is NPU-only, through the ORT VitisAI EP [V].
- FastFlowLM, via [Lemonade](https://lemonade-server.ai/flm_npu_linux.html), runs LLMs on XDNA2. Its kernels are closed source but redistributable [V].

**NVIDIA**
- `onnxruntime-gpu` 1.31.1 targets CUDA 13, with x86_64 and aarch64 builds [V].
- DGX Spark llama.cpp numbers ([bench](https://github.com/ggml-org/llama.cpp/blob/master/benches/dgx-spark/dgx-spark.md)): Gemma3-4B Q4_0 decodes at 81 tok/s, Qwen3-30B-A3B Q8_0 at 61 tok/s [V].
- NVFP4 can be slower than FP8 on SM121 [R].

**Rust stack**
- `ort` 2.0.0-rc.15 (ORT 1.31) [V]. There is no CrownOS-wide pin: crowndictator uses rc.15 with static CUDA, crownos-voice uses rc.12 with `load-dynamic` [V]. VCGE starts on rc.15 with `load-dynamic`.
- `llama-cpp-2` 0.1.161, with the `dynamic-backends` and `mtmd` features [V].
- `fastembed`, `oar-ocr` (PP-OCR on ORT), `image_hasher`, `usearch` [V].
- mistral.rs: CUDA-only alternative. Candle: no Vulkan or NPU [V].

---

## 4. 2026-11-23 approach research

The user answered the open decisions on 2026-11-23. Three research passes compared approaches for each answer. This section summarises them. **No number here was measured on CrownOS hardware.** Vendor numbers and our own estimates are marked.

### 4.1 a. Analyse each capture, then delete it

| Approach | Speed | Accuracy | Verdict |
|---|---|---|---|
| PP-OCRv6 det tiny + rec small on `ort`, changed crops only | Full 1080p frame 0.2–0.8 s on CPU [V vendor] | Recogniser 81.3 on the vendor set, not UI text [V vendor] | Primary candidate |
| Tesseract 5 | ~1.1 s per 1080p screenshot [V] | Beat PaddleOCR on one light screenshot (N=1) [V] | Bake-off candidate |
| Small VLM transcribes everything | 10–30 s per dense window on an iGPU [A, derived] | Drops or invents small text [R] | Rejected for text |
| Small VLM, short JSON caption on low-text frames | ~1 s on RTX, 4–6 s on an iGPU [A, derived] | Good for images, video, canvas [R] | Added, gated |
| OmniParser `icon_detect_v3` | 0.6–0.8 s on a GPU [V vendor] | UI grounding, not text | dGPU-only option, later |

Findings:
- A VLM that transcribes a window is limited by decode speed, so its time grows with the amount of text. OCR scales with the number of lines [A].
- No public OCR benchmark on UI or screen text was found. The `bench-ocr` CER gate is the only real decider.
- Excluded by licence: Surya (OpenRAIL-M with a revenue cap), Moondream 3, the OmniParser v1/v2 detector (AGPL). MiniCPM-V has conflicting licence metadata [R].
- Frame memory: a memfd sealed with `F_SEAL_WRITE` fails while a writable mapping exists; use `F_SEAL_FUTURE_WRITE|SHRINK|GROW` [R, man page]. `mlock` does not keep pages out of the hibernation image, so a logind `PrepareForSleep` inhibitor is needed [R]. The kernel default memlock limit (~8 MiB) is below one 1440p frame (14.7 MB) [R].
- dmabuf zero-copy brings no gain at ≤ 3 captures per minute, and our CPU code cannot zero device buffers [A].

Sources: [PP-OCRv6 docs](https://www.paddleocr.ai/v3.7.0/en/version3.x/algorithm/PP-OCRv6/PP-OCRv6.html) · [PP-OCRv6 paper](https://arxiv.org/html/2606.13108) · [RapidOCR + PP-OCRv6 timing](https://dev.to/kiarina/testing-japanese-and-english-ocr-with-pp-ocrv6-small-and-rapidocr-39pd) · [oar-ocr](https://docs.rs/crate/oar-ocr) · [Windrecorder OCR benchmark](https://github.com/yuka-friends/Windrecorder/blob/main/__assets__/third_party_ocr_engine_benchmark_reference.md) · [Surya licence](https://raw.githubusercontent.com/datalab-to/surya/HEAD/README.md) · [Qwen3.5-2B](https://huggingface.co/Qwen/Qwen3.5-2B) · [Qwen3-VL-2B GGUF](https://huggingface.co/Qwen/Qwen3-VL-2B-Instruct-GGUF) · [OmniParser v2](https://huggingface.co/microsoft/OmniParser-v2.0) · [llama.cpp Vulkan results](https://github.com/ggml-org/llama.cpp/discussions/10879) · [llama.cpp issue #22582 (82 s CPU image encode)](https://github.com/ggml-org/llama.cpp/issues/22582) · [screenpipe](https://github.com/screenpipe/screenpipe) · [memfd_secret(2)](https://man7.org/linux/man-pages/man2/memfd_secret.2.html) · [CUDA external memory interop](https://docs.nvidia.com/cuda/cuda-driver-api/group__CUDA__EXTRES__INTEROP.html)

### 4.2 b. Session-wide accessibility

| Approach | Cost | Who uses it |
|---|---|---|
| Periodic bounded tree walk | One D-Bus round trip per property; walks off-screen content | screenpipe [V] |
| Event-driven + focused-subtree fetch | Near zero when idle; a sequential 2,000-node fetch takes 0.4–1.6 s [A, derived], so requests must run concurrently | Orca, Odilia [V] |
| `Collection.GetMatches` | One call; the app filters in-process | Qt6, ATK bridge (GTK3, Chromium); not GTK4 [V] |
| `Cache.GetItems` | One call per app; large for big apps | GTK4, Qt, ATK bridge [V] |
| OCR only | Inference cost, no app overhead | baseline |

Findings:
- GTK4 ignores `IsEnabled` and emits all events once any listener exists [V]. Qt6 limits events to the registered types [V]. Chromium reads `IsEnabled` only at startup and raises its mode by which calls a client makes [V]. Firefox caches the whole tree in the parent process since version 113 [V].
- Pick: event-driven, then `GetMatches` / `GetItems` / a concurrent bounded walk by toolkit, with a Chromium call allowlist.
- No published CPU or RAM cost per toolkit on Linux. The `bench-a11y` protocol measures it (gate: ≤ 5 % CPU, ≤ 10 % RSS above "off").
- `odilia-cache` pins an old `atspi`; copy its design instead of depending on it [V].

Sources: [Chromium ax_mode.h](https://chromium.googlesource.com/chromium/src/+/lkgr/ui/accessibility/ax_mode.h) · [accessibility_switches.cc](https://chromium.googlesource.com/chromium/src/+/HEAD/ui/accessibility/accessibility_switches.cc) · [browser_accessibility_state_impl_auralinux.cc](https://chromium.googlesource.com/chromium/src/+/HEAD/content/browser/accessibility/browser_accessibility_state_impl_auralinux.cc) · [Chromium accessibility performance](https://developer.chrome.com/blog/chromium-accessibility-performance) · [GTK a11y source](https://gitlab.gnome.org/GNOME/gtk/-/tree/main/gtk/a11y) · [Qt AT-SPI source](https://github.com/qt/qtbase/tree/dev/src/gui/accessible/linux) · [Firefox Platform.cpp](https://hg.mozilla.org/mozilla-central/file/tip/accessible/atk/Platform.cpp) · [Firefox 113 a11y](https://blog.mozilla.org/en/uncategorized/firefox-accessibility-boost/) · [at-spi2-core #85](https://gitlab.gnome.org/GNOME/at-spi2-core/-/issues/85) · [at-spi2-core #139](https://gitlab.gnome.org/GNOME/at-spi2-core/-/issues/139) · [Orca source](https://gitlab.gnome.org/GNOME/orca/-/tree/main/src/orca) · [atspi crate](https://docs.rs/crate/atspi/latest) · [odilia-cache](https://docs.rs/odilia-cache/0.3.0/src/odilia_cache/lib.rs.html)

### 4.3 c. A swappable model layer

| Approach | Fit |
|---|---|
| `ort` 2.0.0-rc.15 (`load-dynamic`) | Fixed graphs: OCR, embeddings, rerank, ASR on CUDA/TensorRT, OpenVINO, VitisAI, MIGraphX, CPU [V] |
| `llama-cpp-2` 0.1.161 (`dynamic-backends`, `mtmd`) | Only Rust path with CUDA + Vulkan + ROCm + CPU, VLMs, raw RGB input and JSON-schema grammar [V] |
| mistral.rs | Fast on CUDA; no Vulkan or ROCm, so no iGPU tier [V] |
| onnxruntime-genai | No Rust bindings; weak VLM coverage [R] |
| OpenAI-compatible HTTP | One adapter covers sidecars, LAN and cloud [A] |

Findings:
- `llama-cpp-2` 0.1.161 `dynamic-backends` sets `GGML_BACKEND_DL` and `GGML_CPU_ALL_VARIANTS` [V]. `llama-cpp-sys-2` has no OpenVINO option [V].
- Abstraction pattern: ML Kit's task API over a generic generator, rig-style capability traits, and the crownos-voice probe pattern.
- Embeddings (MTEB v2 mean, from the model cards): Qwen3-Embedding-0.6B 64.33 multilingual; EmbeddingGemma-300m 61.15 (Gemma licence); bge-m3 59.56 (MIT) [V vendor].
- Intel NPU LLM decode figures conflict (2 vs 16–36 tok/s) [R]. The bake-off decides.
- No public numbers were found for the candidate VLMs on the target hardware. The bake-off measures them.

Sources: [llama.cpp multimodal](https://raw.githubusercontent.com/ggml-org/llama.cpp/HEAD/docs/multimodal.md) · [llama-cpp-2 features](https://docs.rs/crate/llama-cpp-2/latest/features) · [ort releases](https://github.com/pykeio/ort/releases) · [mistral.rs](https://github.com/EricLBuehler/mistral.rs) · [ORT GenAI Qwen3-VL issue](https://github.com/microsoft/onnxruntime-genai/issues/1989) · [llama.cpp OpenVINO backend](https://raw.githubusercontent.com/ggml-org/llama.cpp/HEAD/docs/backend/OPENVINO.md) · [FastFlowLM Qwen3](https://fastflowlm.com/docs/benchmarks/qwen3_results/) · [Phoronix Lemonade Vulkan vs ROCm](https://www.phoronix.com/review/lemonade-vulkan-rocm/3) · [Intel Qwen3](https://www.intel.com/content/www/us/en/developer/articles/technical/accelerate-qwen3-large-language-models.html) · [ML Kit GenAI](https://developers.google.com/ml-kit/genai) · [rig](https://docs.rs/rig_core) · [Apple guided generation](https://developer.apple.com/documentation/foundationmodels/generating-swift-data-structures-with-guided-generation.md) · [Qwen3-Embedding card](https://huggingface.co/Qwen/Qwen3-Embedding-0.6B) · [EmbeddingGemma card](https://huggingface.co/google/embeddinggemma-300m)

### 4.4 d. Any app, with access control

| Identity approach | Can a same-user process fake it? | Verdict |
|---|---|---|
| `/proc/<pid>/exe` path string (current) | Probably yes: fork-then-exec, mount namespaces [R, not tested] | Weak |
| `SO_PEERPIDFD` + exe `(dev, ino)` of a root-owned path | Closes PID reuse and mount namespaces, not fork-then-exec [R] | Medium |
| Per-message `SO_PASSPIDFD` on crownos-ipc | Defeats fork-then-exec; only where we own the wire [R] | Use for the Context API |
| `wp_security_context_v1` app id | No: the sandbox engine vouches [V] | Strong |
| Compositor- or systemd-spawned connection | No: identity is "we spawned it" [A] | Strong, for first-party daemons |
| Portal PermissionStore | Host app ids are self-claimed; D-Bus round trip per call [V] | Store only |

Findings:
- The Context API authorizes by uid only; crownos-ipc handlers do not get the peer; `crownos_context_v1` v1 allows one binder that can pause everyone; revocation does not cut live sessions; output screencopy and screencast ignore window sensitivity [V, code read].
- Any unsandboxed app already has screencopy, so for host apps access control is consent, not enforcement. Sandboxed apps are enforced.
- Precedents: macOS TCC keys grants to the code signature; Android MediaProjection asks per session; KWin hashes the executable after a path-based design was bypassed [R].

Sources: [LKML on /proc/pid/exe](https://lore.kernel.org/lkml/20200318115637.qffmwvtr4uqanyvc@wittgenstein/T/) · [SO_PEERPIDFD](https://lore.kernel.org/all/20230317055555.33192-1-kuniyu@amazon.com/T/) · [SO_PASSPIDFD patch](https://lore.kernel.org/lkml/20230901205145.10640-A-hca@linux.ibm.com/T/) · [security-context-v1](https://wayland.app/protocols/security-context-v1) · [PermissionStore](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.impl.portal.PermissionStore.html) · [systemd desktop environments](https://systemd.io/DESKTOP_ENVIRONMENTS/) · [KWin D22571](https://phabricator.kde.org/D22571) · [Yama](https://docs.kernel.org/admin-guide/LSM/Yama.html) · [Android MediaProjection](https://developer.android.com/media/grow/media-projection) · [Windows GraphicsCaptureAccess](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscaptureaccess?view=winrt-20348)
