# 04 — Models and Inference

Tags: **[V]** verified · **[R]** reported · **[A]** design decision / assumption to validate.
Model availability/licences were checked against Hugging Face / GitHub on 2026-11-08 by the research pass; **every model choice below is provisional until the Phase 0 bake-off** on real target hardware.

## 10.1 Principles

1. **Two runtimes, not five.** `ort` (ONNX Runtime) for every fixed-graph model; llama.cpp (`llama-cpp-2`) for every generative model. Vendor LLM runtimes (OpenVINO GenAI, FastFlowLM/Lemonade, QAIRT Genie, TRT-LLM) are optional **out-of-process OpenAI-compatible sidecars**, never linked — keeps closed NPU kernels out of our process and licence [A].
2. **Models are data.** `models/registry.ron` lists task → candidates (file, sha256, licence, input shapes, quantization, min tier, preferred EPs). Swapping a model never needs code.
3. **Cheapest model that meets the quality bar**, measured on our own screenshot/eval corpus — not leaderboard numbers.
4. **Resident small, load big on demand.** OCR + text embedding stay resident (< 400 MB on low tier); LLM/VLM load on demand, unload after idle (default 120 s; 0 on battery).
5. **One multimodal generative model serves both "LLM" and "VLM" roles** where the family is natively multimodal (Qwen3.5 small series [V]), halving resident memory.

## 10.2 Tasks and candidate models

| Task | When it runs | Primary candidate | Proven fallback | Licence | Size |
|---|---|---|---|---|---|
| OCR det+rec | Per accepted capture, changed crops only | **PP-OCRv6** tiny/small ONNX (Jun 2026) [V] | PP-OCRv5 mobile (RapidOCR packaging) [V] | Apache-2.0 | 2–35 M params, < 100 MB RSS |
| Text embedding | Segment/episode/entity creation; queries | **Qwen3-Embedding-0.6B** (MRL → 256 d) [V] | all-MiniLM-L6-v2 (384 d) for CPU tier; EmbeddingGemma-2 (0.74 B, Apache) [V] to evaluate | Apache-2.0 | 0.6 B (int8 ≈ 0.7 GB) |
| Image embedding (optional) | Keyframes of low-text segments | **SigLIP2-base** [V] | nomic-embed-vision-v1.5 [V] | Apache-2.0 | 375 M / 93 M |
| Reranker (interactive only) | Top-30 of interactive search | Qwen3-Reranker-0.6B [A] | bge-reranker-v2-m3 [A] | Apache / MIT | 0.6 B |
| Summarize + extract (JSON schema) | Segment close (batched), episode close, consolidation | **Qwen3.5-2B / 4B** (natively multimodal) [V] | Qwen3-4B-Instruct-2507, Phi-4-mini (MIT), Gemma 4 E2B/E4B (Apache) [V] | Apache-2.0 | 2B Q4 ≈ 1.8–2.5 GB incl. mmproj |
| VLM describe | Low-text, important segments; explicit "what's on screen" | Same model as above (shared) | Qwen3-VL-2B/4B [V], SmolVLM2-500M for CPU tier [V] | Apache-2.0 | — |
| UI element detection (Phase 8) | Apps with no a11y and low OCR coverage | OmniParser **icon_detect_v3** (YOLOv9, **MIT**) [V] | — | MIT (avoid AGPL v1 detector) | ~60 MB |
| Document OCR (on demand) | Static document page the user dwells on | PaddleOCR-VL-1.6 (0.96 B) [V] | Florence-2-base (MIT) | Apache / MIT | — |
| Secret/PII NER (cloud redaction) | Before any cloud upload | Deterministic detectors + small LLM pass | — | — | — |

Excluded: jina-clip-v2 / jina-embeddings-v5 (CC-BY-NC) [V]; OmniParser v1/v2 `icon_detect` (AGPL) [V]; Moondream 3.x (non-standard licence) [V]; Llama licence models as defaults (least permissive).

## 10.3 Hardware tiers and per-tier recommendations

| | **T1 CPU-only** (≤ 16 GB, no usable accel) | **T2 x86 AI PC** (Intel LNL/ARL/PTL, AMD Ryzen AI 300 / Strix) | **T3 Snapdragon X (Linux)** | **T4 NVIDIA dGPU** | **T5 DGX Spark** |
|---|---|---|---|---|---|
| What works on Linux | CPU | Intel NPU (OpenVINO, prod) [V]; AMD XDNA2 (VitisAI, prod w/ kernel ≥ 7.0 caveats) [V]; iGPU Vulkan [R] | Oryon CPU; Adreno Vulkan (Turnip) [R]; NPU **experimental only** [R] | CUDA/TensorRT [V] | CUDA 13 aarch64, 128 GB unified [V] |
| OCR | PP-OCRv6 tiny, ORT CPU (XNNPACK) | PP-OCRv6 small, ORT **OpenVINO-NPU** / **VitisAI** (static shapes) | PP-OCRv6 small, ORT CPU (XNNPACK/KleidiAI); QNN EP behind `experimental-qnn` | PP-OCRv6 medium, ORT CUDA/TensorRT | same as T4 |
| Text embedding | MiniLM-L6 int8, CPU | Qwen3-Emb-0.6B int8, NPU | Qwen3-Emb-0.6B int8, CPU | Qwen3-Emb-0.6B fp16, CUDA | same |
| Image embedding | off | SigLIP2-base, NPU/iGPU | off by default | SigLIP2, CUDA | same |
| LLM/VLM | Qwen3.5-0.8B/2B Q4, CPU, **AC + idle only, batched** | Qwen3.5-2B/4B Q4, llama.cpp **Vulkan** (Arc/RDNA iGPU); optional FastFlowLM sidecar on XDNA2 | Qwen3.5-2B Q4_0, llama.cpp **Vulkan (Adreno)** | Qwen3.5-9B Q4_K_M / Q8, llama.cpp CUDA | Qwen3.5-27B-class / 30B-A3B MoE Q8, llama.cpp CUDA (FP8 via TRT-LLM sidecar optional; NVFP4 only after bench) |
| Reranker | off | on demand, iGPU | on demand, CPU | CUDA | CUDA |
| Resident RAM target (inferd) | ≤ 350 MB (+1.2 GB when LLM loaded) | ≤ 900 MB (+2.5–3.5 GB) | ≤ 900 MB (+2.5 GB) | ≤ 1 GB host + ≤ 2 GB VRAM (+6–9 GB VRAM) | no practical limit; keep ≤ 24 GB |
| Cloud/LAN offload role | Main path for VLM + weekly consolidation (consented) | Weekly consolidation, hard queries | Same as T2 | Rare | **Is the offload target** for the user's laptops |

Reference latencies: Llama-3.2-1B on Snapdragon X1 — **Vulkan 37.8 tok/s vs NPU 11.7 tok/s** decode [R]; DGX Spark llama.cpp — Gemma3-4B Q4_0 81 tok/s, Qwen3-30B-A3B Q8 61 tok/s decode [V]; Intel NPU LLM decode ~2 tok/s on 1B [R] (hence encoders-only on NPU).

## 10.4 Backend abstraction (in `context-infer`)

```
trait InferenceBackend      # fixed-graph tasks
  OrtBackend { ep_preference: [ExecutionProvider] }
     Intel:      OpenVINO(NPU) → OpenVINO(GPU) → CPU
     AMD XDNA2:  VitisAI → CPU
     NVIDIA:     TensorRT → CUDA → CPU
     Snapdragon: QNN (experimental flag) → CPU(XNNPACK)
     any:        CPU
trait GenerativeBackend     # LLM/VLM with JSON-schema grammar
  LlamaBackend { ggml backend: CUDA | Vulkan | CPU }   # dynamically loaded ggml backends
  SidecarBackend { OpenAI-compatible URL }            # OpenVINO GenAI / Lemonade / TRT-LLM / LAN Spark
```

- **Selection:** read `/etc/crownos/ai.ron` (`AiConfig` ranked backends [V]; extend enum with `Qnn`, `TensorRt`, `Vulkan`), probe devices, then a **first-run micro-benchmark** per (task, EP) cached in `$XDG_CACHE_HOME/crownos/context/backends.ron`, keyed by driver/runtime versions — same idea as crownos-voice's probe cache [V].
- **NPU compile cache:** ORT EPContext models / OpenVINO cache dir; compile at install or first idle, never on the capture path.
- **Static shapes for NPUs:** OCR det tiles 640×640 / 960×960; rec height 48, width buckets 320/640/960; batched rec lines.
- **Runtime packaging:** follow crownos-voice: `ort` with `load-dynamic` against system `onnxruntime-crownos[-cuda|-openvino|-vitisai|-qnn]` packages [V]; bump the CrownOS-wide `ort` pin together (`=2.0.0-rc.12` today [V]; rc.15 / ORT 1.31 current [V]). llama.cpp ggml backends packaged as optional `.so`s [A — verify `llama-cpp-2` supports `GGML_BACKEND_DL`; fallback: per-backend inferd builds].
- **Circuit breakers:** a backend that errors/hangs (watchdog per call) is disabled for the session and the task falls back to the next EP; NPU driver crashes only kill inferd.

## 10.5 Pre-processing and hardware acceleration

- Capture buffers: shm via compositor GPU readback (already implemented [V]) for MVP; per-capture CPU work is limited to damaged tiles (xxh3 tile hashes, ~GB/s) and crop → negligible.
- Optional dmabuf path (Phase 8, only if profiling shows CPU readback cost matters on T1/T3): import dmabuf into Vulkan (`ash`) compute for downscale + grayscale + tile hash, read back only changed crops.
- Thumbnails: CPU JPEG at 1280 px (~5 ms) [A]; VA-API/NVJPEG only if profiling justifies.

## 10.6 Generative call design (keeps LLM usage rare)

| Call | Trigger | Input (bounded) | Output (JSON schema, grammar-enforced) | Budget |
|---|---|---|---|---|
| `summarize_segment_batch` | Every ~10 min or 8 closed segments, whichever first; only segments ≥ 60 s active or importance ≥ 0.4 | Per segment: app, title, url/file, label candidates, ≤ 600 tokens of deduped text | `{summary, activity_label, entities[{name,kind,alias}], facts[{s,p,o,confidence}], open_items[]}` per segment | ≤ 60 calls/day T1, ≤ 200 T2–T5 |
| `summarize_episode` | Episode close | segment summaries + top entities | `{title, summary, project_guess, outcome}` | ≤ 40/day |
| `vlm_describe` | Segment close if `text_coverage` < threshold and importance ≥ 0.5, or explicit request | 1 keyframe (≤ 1024 px) + metadata | `{description, visible_entities[], content_kind}` | ≤ 6/h, ≤ 50/day (T2+); T1 → cloud only if consented |
| `resolve_entity` | Ambiguous merge only | two entity cards | `{same: bool, confidence}` | rare |
| Consolidation prompts | Idle + AC | episodes/facts of a project or day | profile / rollup docs | nightly |

## 10.7 Cloud and LAN offload

Preference order: **(1) user's own DGX Spark / workstation on LAN** (OpenAI-compatible endpoint: llama-server, vLLM, TRT-LLM serve), paired once with mutual TLS (key exchange via CrownOS device pairing, e.g. crownlink) [A]; **(2) third-party API** (provider-agnostic adapter, e.g. Anthropic, OpenAI, Google).

- **Eligible tasks:** weekly/project consolidation over long context, complex multi-episode questions, VLM when local tier can't (T1) or local confidence is low **and** user asked.
- **Never sent:** raw DB, pixels (unless the user explicitly asks about the current screen and approves that request), anything from excluded/sensitive windows, clipboard secrets.
- **Pipeline:** select minimal text → deterministic detectors (emails, phones, Luhn cards, IBAN, API keys/JWT/private keys, high-entropy tokens, file paths under `$HOME` → placeholders) → small-LLM NER pass for names/orgs (optional per consent level) → reversible placeholder map kept locally → relay re-verifies no detector fires → send → re-hydrate locally → audit (payload hash, size, provider, purpose).
- **Consent:** per category (`consolidation`, `qa`, `vision`) and per destination (LAN node vs provider); first upload of each category shows the exact payload; monthly token/cost budget; kill switch in the indicator menu.
