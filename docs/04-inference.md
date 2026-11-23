# 04 — Models and Inference

Tags: **[V]** verified · **[V vendor]** verified source, vendor's own benchmark · **[R]** reported · **[A]** design decision / assumption to validate · **[A, derived]** our calculation, not measured.

Model availability and licences were checked on Hugging Face and GitHub on 2026-11-08 and again on 2026-11-23. **Every model below is a bake-off candidate. Nothing was measured on CrownOS hardware yet.** Vendor numbers are marked as vendor numbers. The benchmark workstream ([08 §Phase 0](08-implementation-plan.md#phase-0--foundations-and-de-risking-spikes)) confirms or replaces each pick.

Bake-off machine: Ryzen 7 7840HS (XDNA1 NPU), Radeon 780M (Vulkan), RTX 4050 Laptop (CUDA), 14 GiB RAM. There is no Intel NPU, so Intel numbers stay unmeasured.

## 10.1 Principles

1. **The model layer is swappable.** `context-infer` defines one trait per modality and a task layer on top. Callers ask for a task, never for a model. To swap a model, edit `registry.ron`. To add a runtime, add a backend crate ([§10.4](#104-model-layer-in-context-infer)) [A].
2. **Three backends, not five.** `ort` (ONNX Runtime) runs fixed-graph models. llama.cpp (`llama-cpp-2`) runs generative models. One OpenAI-compatible HTTP backend reaches everything else: vendor runtimes (OpenVINO GenAI/OVMS, FastFlowLM/Lemonade, TRT-LLM), a LAN node and cloud APIs. Vendor runtimes are never linked. This keeps closed NPU kernels out of our process and licence [A].
3. **Models are data.** `models/registry.ron` v2 lists models with capability tags and per-runtime artifacts, and maps each task to ranked candidates.
4. **Cheapest model that meets the quality bar**, measured on our own screen-text and eval corpus, not on leaderboards.
5. **Frames stay local.** Analysis runs at capture time ([05 §11.6](05-privacy-security.md#116-analyse-then-delete)). A frame goes only to the in-process backends of `crown-context-inferd`. It never goes to the HTTP backend: inferd does not link it [V]. The HTTP backend also refuses an image unless the server is on loopback [V].
6. **Resident small, load big on demand.** OCR and text embedding stay resident (< 400 MB on the low tier). The LLM loads on demand and unloads after 120 s idle. A capture-time VLM at 1–3 captures per minute never reaches that idle time, so on the NVIDIA tier it stays resident while captures are active and unloads when a game starts. Other tiers load it only on AC power [A].
7. **One multimodal model serves both "LLM" and "VLM" roles** where the family is natively multimodal (Qwen3.5 small series [V]). This halves resident memory.

## 10.2 Tasks and candidate models

| Task | When it runs | Bake-off candidates | Also in the bake-off | Licence | Size |
|---|---|---|---|---|---|
| OCR det+rec | Each capture, changed crops only | **PP-OCRv6** detector tiny + recogniser small, ONNX [V] | Tesseract 5 [V]; PP-OCRv5 mobile (RapidOCR packaging) [V] | Apache-2.0 | 1.5 M / 7.7 M params [V], < 300 MB RSS [A] |
| Text embedding | Segment, episode and entity creation; queries | **Qwen3-Embedding-0.6B** (MRL → 256 d) [V] | EmbeddingGemma-300m (**Gemma licence**) [V]; bge-m3 (MIT) [V]; all-MiniLM-L6-v2 (384 d) for the CPU tier | Apache-2.0 | 0.6 B (int8 ≈ 0.7 GB) |
| Reranker (interactive only) | Top-30 of interactive search | Qwen3-Reranker-0.6B [A] | bge-reranker-v2-m3 [A] | Apache / MIT | 0.6 B |
| Summarize + extract (JSON schema) | Segment close (batched), episode close, consolidation. Text only | **Qwen3.5-2B / 4B** (natively multimodal) [V] | Qwen3-4B-Instruct-2507, Phi-4-mini (MIT), Gemma 4 E2B/E4B (Apache) [V] | Apache-2.0 | 2B Q4 ≈ 1.8–2.5 GB incl. mmproj |
| Screen caption (VLM) | **At capture time**, only for low-text frames ([§10.6](#106-generative-call-design-keeps-llm-usage-rare)) | Same model as above (shared) | Qwen3-VL-2B [V]; SmolVLM2-500M for the CPU tier [V] | Apache-2.0 | — |
| UI element detection (`ui_detect`, Phase 8) | At capture time, dGPU tier only, apps with no a11y and low OCR coverage | OmniParser **icon_detect_v3** (YOLOv9, **MIT**) [V] | — | MIT | ~60 MB |
| Document OCR | At capture time, for a static document page the user dwells on | PaddleOCR-VL-1.6 (0.96 B) [V]; Granite-Docling-258M [R] | Florence-2-base (MIT) | Apache / MIT | — |
| Secret/PII NER | On all extracted text at capture time, and before any LAN or cloud request | Deterministic detectors + small LLM pass | — | — | — |

Excluded:
- Surya: weights under a modified OpenRAIL-M licence with a revenue cap [V].
- OmniParser v1/v2 `icon_detect`: AGPL [V].
- Moondream 3.x: non-standard licence [V].
- MiniCPM-V: licence metadata conflicts [R]. It is also not in llama.cpp's pre-quantised list [V].
- jina-clip-v2 and jina-embeddings-v5: CC-BY-NC [V].
- Llama-licence models as defaults (least permissive).

### 10.2.1 Approach comparison: getting everything out of a frame (2026-11-23)

| Approach | Speed (vendor or derived) | Accuracy | Verdict |
|---|---|---|---|
| PP-OCRv6 det tiny + rec small on `ort`, changed crops only | Full 1080p frame 0.2–0.8 s on CPU [V vendor]; crops take much less [A] | Recogniser 81.3 on the vendor set, which is not UI text [V vendor] | **Primary candidate** |
| Tesseract 5 | ~1.1 s per 1080p screenshot on CPU [R] | Beat PaddleOCR on one light screenshot (N=1) [R] | Bake-off candidate |
| Small VLM transcribes the whole window | 10–30 s per dense window on an iGPU, limited by decode speed [A, derived] | Drops or invents small text [R] | Rejected for text |
| Small VLM, short JSON caption, low-text frames only | ~1 s on an RTX GPU, 4–6 s on an iGPU [A, derived] | Good for images, video, canvas [R] | **Added, gated** |
| OmniParser `icon_detect_v3` | 0.6–0.8 s on a GPU [V vendor] | UI grounding, not text | dGPU-only option, later |

## 10.3 Hardware tiers and per-tier recommendations

T3 (Snapdragon) and CrownOS running on T5 (DGX Spark) need aarch64 packages. They are a **future track** and not MVP gates. A Spark on the LAN is in scope now, as an HTTP endpoint.

| | **T1 CPU-only** (≤ 16 GB, no usable accel) | **T2 x86 AI PC** (Intel LNL/ARL/PTL, AMD Ryzen AI 300 / Strix) | **T3 Snapdragon X (Linux)** — future track | **T4 NVIDIA dGPU** | **T5 DGX Spark** |
|---|---|---|---|---|---|
| What works on Linux | CPU | Intel NPU (OpenVINO, prod) [V]; AMD XDNA2 (VitisAI, prod w/ kernel ≥ 7.0 caveats) [V]; iGPU Vulkan [R] | Oryon CPU; Adreno Vulkan (Turnip) [R]; NPU **experimental only** [R] | CUDA/TensorRT [V] | CUDA 13 aarch64, 128 GB unified [V] |
| OCR | PP-OCRv6 tiny/small, ORT CPU (XNNPACK) | PP-OCRv6 small, ORT **OpenVINO-NPU** / **VitisAI** (static shapes) | PP-OCRv6 small, ORT CPU (XNNPACK/KleidiAI); QNN EP behind `experimental-qnn` | PP-OCRv6 small/medium, ORT CUDA/TensorRT | — (LAN node, no capture) |
| Text embedding | MiniLM-L6 int8, CPU | Qwen3-Emb-0.6B int8, NPU | Qwen3-Emb-0.6B int8, CPU | Qwen3-Emb-0.6B fp16, CUDA | — |
| LLM (text) | Qwen3.5-0.8B/2B Q4, CPU, **AC + idle only, batched** | Qwen3.5-2B/4B Q4, llama.cpp **Vulkan** (Arc/RDNA iGPU); optional FastFlowLM sidecar on XDNA2 | Qwen3.5-2B Q4_0, llama.cpp **Vulkan (Adreno)** | Qwen3.5-9B Q4_K_M / Q8, llama.cpp CUDA | Qwen3.5-27B-class / 30B-A3B MoE Q8 behind llama-server (FP8 via TRT-LLM optional; NVFP4 only after bench) |
| VLM caption (capture time) | Off by default. On AC: SmolVLM2 or Qwen3.5-0.8B, ≤ 64 tokens | Shared Qwen3.5-2B on the iGPU, on AC only | Shared model, on AC only | Shared Qwen3.5-4B on every low-text capture; optional on every capture | — |
| Reranker | off | on demand, iGPU | on demand, CPU | CUDA | — |
| Resident RAM target (inferd) | ≤ 350 MB (+1.2 GB when LLM loaded) | ≤ 900 MB (+2.5–3.5 GB) | ≤ 900 MB (+2.5 GB) | ≤ 1 GB host + ≤ 2 GB VRAM (+3–4 GB VRAM for the VLM) | — |
| LAN / cloud role (text only, consented) | Weekly consolidation, hard queries | Weekly consolidation, hard queries | Same as T2 | Rare | **Is the LAN target** for the user's laptops |

Reference numbers, all from other hardware:
- Llama-3.2-1B on Snapdragon X1: **Vulkan 37.8 tok/s vs NPU 11.7 tok/s** decode [R].
- DGX Spark llama.cpp: Gemma3-4B Q4_0 81 tok/s, Qwen3-30B-A3B Q8 61 tok/s decode [V].
- Intel NPU LLM decode: sources conflict. One says ~2 tok/s on 1B [R]. Others say ~16–21 tok/s for Qwen3 0.6B/1.7B and a 36.68 tok/s peak [R]. The bake-off decides. Until then the NPU runs encoders only.
- FastFlowLM on a Ryzen AI 7 350 XDNA2 NPU: Qwen3-4B decode 19.6 tok/s [V vendor].

## 10.4 Model layer (in `context-infer`)

### Crates

| Crate | Role | Unsafe code |
|---|---|---|
| `context-infer` | Types (`RgbImage`, `GenRequest`, `Output::{Text, Json(schema)}`), modality traits, task layer, `registry.ron` v2 loader, router. No runtime dependencies | Forbidden |
| `context-infer-ort` | `ort` runtime: OCR, embeddings, rerank, ASR. EPs: TensorRT, CUDA, OpenVINO (NPU/GPU), VitisAI, MIGraphX, CPU | Allowed (crate-level lint override) |
| `context-infer-llama` | llama.cpp runtime: `Generate` (with `mtmd` for vision), GGUF embeddings. Devices: CUDA, Vulkan, ROCm, CPU as dynamically loaded ggml modules | Allowed (crate-level lint override) |
| `context-infer-http` | OpenAI-compatible client: local sidecars (llama-server, Lemonade/FastFlowLM, OVMS), LAN node (Spark), cloud APIs | Forbidden |
| `crown-context-inferd` (bin) | crownos-ipc server. Hosts the ort and llama backends. Jobs carry images in memfds. Returns `Busy{retry_after}` under load. Unloads idle models. No network, no DB | — |

The workspace keeps `unsafe_code = "forbid"` everywhere else [V]. The two runtime crates hold the mmap and FFI code.

**inferd limits [V].** inferd maps only memfds sealed with `F_SEAL_WRITE|F_SEAL_SHRINK|F_SEAL_GROW` and refuses others. An image can have at most 16384 px per side and 64 MP. A job budget is cut to 10 minutes. Each model queues at most 32 jobs; more jobs get `Busy{retry_after}`. inferd reads the battery state from `/sys/class/power_supply` and skips supplies with `scope=Device`, such as a wireless mouse.

**Where the HTTP backend runs [A].** inferd has `PrivateNetwork=yes`, which also blocks localhost sidecars. So `crown-context-relayd` hosts `context-infer-http`. relayd gets text only, starts only when a sidecar, LAN or cloud endpoint is configured, and may reach only the configured addresses (`IPAddressAllow=`; whether a user unit can apply it is a Phase 0 spike [A]). Frames never reach relayd.

### Traits and task layer (illustrative, not final Rust)

```rust
pub trait Generate   { fn capabilities(&self) -> GenerateCapabilities; fn generate(&self, request: &GenRequest) -> Result<Generation, InferError>; }
pub trait Embed      { fn space(&self) -> &EmbedSpace; fn embed(&self, texts: &[String], purpose: EmbedPurpose) -> Result<Embeddings, InferError>; }
pub trait Rerank     { fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<f32>, InferError>; }
pub trait Ocr        { fn recognize(&self, image: &RgbImage) -> Result<Vec<TextLine>, InferError>; }
pub trait Transcribe { fn transcribe(&self, samples: &[i16]) -> Result<String, InferError>; }

pub trait ContextTasks {
    fn recognize_text(&self, image: &RgbImage, job: Job) -> Result<Vec<TextLine>, InferError>;
    fn describe_screen(&self, screen: &RgbImage, app_name: &str, window_title: &str, job: Job) -> Result<ScreenDescription, InferError>;
    fn summarize_segments(&self, segments: &[SegmentDigest], job: Job) -> Result<Vec<SegmentSummary>, InferError>;
    fn embed_text(&self, texts: Vec<String>, purpose: EmbedPurpose, job: Job) -> Result<EmbeddedTexts, InferError>;
}
```

- The tasks are `recognize_text`, `embed_text`, `rerank`, `summarize_segments`, `describe_screen` and `transcribe`. `ui_detect` is a later task (Phase 8). `generate` is a capability trait under the tasks, not a task.
- The task layer never names a model. The JSON schema of each output comes from its Rust type (`schemars`). Each backend lowers it: GBNF or llguidance in llama.cpp, `response_format` over HTTP.
- Errors: `InferError::{Unavailable, Timeout, Backend, Schema}` (`thiserror`).
- FFI calls run on a dedicated worker thread per loaded model.

### `registry.ron` v2

```ron
(
    version: 2,
    models: {
        "qwen3.5-2b": (capabilities: [Text, Vision], licence: "Apache-2.0", context: 32768,
                       artifacts: [Gguf(model: "...", mmproj: Some("..."), quant: "Q4_K_M", sha256: "..."),
                                   Remote(name: "qwen3.5-2b")]),
        "pp-ocrv6-det-tiny": (capabilities: [Ocr], licence: "Apache-2.0",
                              artifacts: [Onnx(path: "...", shapes: [[1, 3, 640, 640]], sha256: "...")]),
    },
    tasks: {
        "describe_screen": [(model: "qwen3.5-2b", runtimes: [Llama([Cuda, Vulkan, Cpu])], min_tier: T2)],
        "summarize_segments": [(model: "qwen3.5-2b", runtimes: [Llama([Cuda, Vulkan, Cpu]), Http(Lan)], min_tier: T1)],
    },
)
```

A user override file can add or re-rank candidates. The registry in the repo is still v1 and lists older models [V]. W2 replaces it.

### Router

The router is part of the `context-infer` library and runs inside each backend host [A]:
- inferd runs it over the `Ort` and `Llama` candidates;
- relayd runs it over the `Http` candidates, because inferd has no network.

contextd's scheduler does not rank models. For a text-only task it only picks local (inferd) or remote (relayd), and falls through to the other host on `Unavailable` or `Busy`. Vision tasks always go to inferd.

**Swap limits.** A model or an in-process runtime can be swapped freely. A runtime that needs the network must live in relayd. Vision input never leaves the machine.

1. **Candidates** are (model × runtime × device) for the task.
2. **Filter** by probe result, policy and tier. The probe checks runtimes and devices without loading a model, as crownos-voice does [V]. Policy covers battery, consent for remote endpoints, and "frames never go to `Http`".
3. **Rank** by a cached micro-benchmark in `$XDG_CACHE_HOME/crownos/context/backends.ron`, keyed by driver and runtime versions.
4. **Circuit breaker per (runtime, device):** closed → open on error or watchdog timeout → half-open after a cooldown. A deadline miss falls through to the next candidate. An NPU driver crash kills only inferd.
5. **Embedding space:** a change of `EmbedSpace` starts a re-index job of the stored text. Vector spaces are never mixed.

### Approach comparison: runtimes (2026-11-23)

| Approach | Fit |
|---|---|
| `ort` 2.0.0-rc.15 (`load-dynamic`) [V] | Fixed graphs: OCR, embeddings, rerank, ASR. CUDA/TensorRT, OpenVINO, VitisAI, MIGraphX, CPU |
| `llama-cpp-2` 0.1.161 (`dynamic-backends`, `mtmd`) [V] | The only Rust path with CUDA + Vulkan + ROCm + CPU, VLMs (Qwen3-VL, Qwen3.5, Gemma 3, SmolVLM2), raw RGB input and JSON-schema grammar. suja already uses it [V] |
| mistral.rs [V] | Fast on CUDA. No Vulkan or ROCm, so no iGPU tier |
| onnxruntime-genai [R] | No Rust bindings. Weak VLM coverage (Qwen3-VL only through a workaround) |
| OpenAI-compatible HTTP (llama-server, Lemonade/FastFlowLM, OVMS, Spark, cloud) | One adapter covers sidecars, LAN and cloud. Overhead not measured; estimate < 1 ms per request [A], so OCR and embeddings stay in-process |

### Approach comparison: abstraction designs (2026-11-23)

| Design | What VCGE takes |
|---|---|
| Android ML Kit GenAI / AICore [R] | A task API for callers over a generic generator; BUSY back-pressure |
| rig (Rust) [R] | One trait per capability, implemented per provider |
| crownos-voice `voice-abi` [V] | Probe without model load, probe cache, explicit EP fallback lists |
| Apple Foundation Models guided generation [V] | Output schema derived from types; the backend enforces it |

### Facts that shape the runtime packaging

- `llama-cpp-2` 0.1.161 has a `dynamic-backends` feature. It sets `GGML_BACKEND_DL=ON` and `GGML_CPU_ALL_VARIANTS=ON` and exposes `load_backends_from_path` [V]. The `cuda`, `vulkan` and `rocm` features can be combined [V]. So one inferd binary plus optional `libggml-{cuda,vulkan,rocm,cpu-*}.so` packages is possible. A real CUDA + Vulkan co-build is not tested yet [A].
- `llama-cpp-2` `mtmd` takes raw RGB (`MtmdBitmap::from_image_data`) and has `json_schema_to_grammar` [V]. Frames need no JPEG or base64 step.
- `llama-cpp-sys-2` has **no OpenVINO build option** [V]. Intel generative work uses Vulkan or an OVMS/OpenVINO GenAI sidecar.
- **There is no CrownOS-wide `ort` pin** [V]. crowndictator uses `=2.0.0-rc.15` with static CUDA. crownos-voice uses `=2.0.0-rc.12` with `load-dynamic`. VCGE starts on rc.15 with `load-dynamic`. Moving the other two to the same pin is a follow-up.
- `ort` with `load-dynamic` loads the system `onnxruntime-crownos[-cuda|-openvino|-vitisai]` packages, as crownos-voice does [V].
- `crownos-config/src/system/ai.rs` mixes devices and runtimes in one enum [V]. Split `Device` from `Runtime` and add `Vulkan`, `TensorRt`, `Rocm` and `Remote`.
- **NPU compile cache:** ORT EPContext models / OpenVINO cache dir. Compile at install or at first idle, never on the capture path.
- **Static shapes for NPUs:** OCR det tiles 640×640 / 960×960; rec height 48, width buckets 320/640/960; batched rec lines.
- **Structured output differs per backend** (GBNF, llguidance, `response_format`). Each backend runs a conformance test with every task schema.

## 10.5 Pre-processing and hardware acceleration

- Capture buffers: shm through compositor GPU readback (already implemented [V]), into a per-capture memfd ([05 §11.6](05-privacy-security.md#116-analyse-then-delete)). Models read sealed copies of regions, not the capture memfd [V]. Per-capture CPU work is limited to damaged tiles (xxh3 tile hashes, ~GB/s) and crops.
- inferd maps each region copy read-only and hands the slice to `ort` (IoBinding with input buffers we own) or to `mtmd`. Frames are never encoded to JPEG or PNG.
- dmabuf zero-copy is not used. On hybrid laptops the compositor renders on the iGPU, so a dGPU import needs a copy anyway, and our CPU code cannot zero device buffers [A]. Revisit only if capture rises to about 1 per second.

## 10.6 Generative call design (keeps LLM usage rare)

| Call | Trigger | Input (bounded) | Output (JSON schema, grammar-enforced) | Budget |
|---|---|---|---|---|
| `summarize_segments` | Every ~10 min or 8 closed segments, whichever first; only segments ≥ 60 s active or importance ≥ 0.4 | Per segment: app, title, url/file, label candidates, ≤ 600 tokens of deduped text | `{summary, activity_label, entities[{name,kind,alias}], facts[{s,p,o,confidence}], open_items[]}` per segment | ≤ 60 calls/day T1, ≤ 200 T2–T5 |
| `summarize_episode` | Episode close | segment summaries + top entities | `{title, summary, project_guess, outcome}` | ≤ 40/day |
| `describe_screen` | **At capture time**, when a11y + OCR give < ~200 characters or image regions cover > ~40 % of the window, and at least 50 % of the window changed. Skipped on battery. On T4, optionally every capture | A sealed copy of the whole frame (≤ 1280 px long edge [A]) + metadata | `{content_kind, caption, visible_entities[]}`, ≤ 128 tokens | ≤ 6/h and ≤ 50/day on T1–T3; T4 limited only by the gate. When the budget is empty, the frame keeps its OCR text only |
| `resolve_entity` | Ambiguous merge only | two entity cards | `{same: bool, confidence}` | rare |
| Consolidation prompts | Idle + AC | episodes/facts of a project or day | profile / rollup docs | nightly |

## 10.7 LAN and cloud through the HTTP backend

LAN and cloud endpoints are ordinary `Http` runtimes in the registry. relayd's router ranks them; contextd picks local or remote per text task. The user decides which endpoints exist. All are off by default.

- **LAN node** (a DGX Spark or workstation running llama-server, vLLM or TRT-LLM serve): paired once with mutual TLS (key exchange through CrownOS device pairing, for example crownlink) [A].
- **Cloud API** (provider-agnostic OpenAI-compatible adapter).
- **Eligible tasks:** text only. Weekly or project consolidation over long context, complex multi-episode questions.
- **Never sent:** raw DB, pixels, anything from excluded or sensitive windows, clipboard secrets. A "what is on my screen" request takes a fresh capture and runs the local VLM.
- **Pipeline:** select minimal text → deterministic detectors (emails, phones, Luhn cards, IBAN, API keys/JWT/private keys, high-entropy tokens, file paths under `$HOME` → placeholders) → small-LLM NER pass for names/orgs (optional per consent level) → reversible placeholder map kept locally → relayd re-verifies that no detector fires → send → re-hydrate locally → audit (payload hash, size, endpoint, purpose).
- **Consent:** per category (`consolidation`, `qa`) and per endpoint (LAN node or provider); the first request of each category shows the exact payload; monthly token/cost budget; kill switch in the indicator menu.
