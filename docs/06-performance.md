# 06: Performance targets and resource profiles

Every number here is a **target [A]**. The Phase 0 benchmarks and the Phase 5 8-hour soak validate them.

Some reference points from the field:
- Screenpipe documents 5–10% CPU and 0.5–3 GB RAM [V].
- Screenpipe users report 10–30% CPU and up to 4 GB RAM [R].
- Recall requires 16 GB of RAM and a 40+ TOPS NPU [V].

VCGE aims well below Screenpipe because of three choices:
- It uses compositor damage instead of polling and diffing the screen.
- It prefers accessibility text over OCR.
- It calls the LLM once per activity segment, not once per frame.

## 12.1 Global targets (measured on a T2 laptop, 8 hours of mixed work)

| Metric | Target | How it is measured |
|---|---|---|
| Compositor overhead from the context module | < 50 µs per emitted event, **0 dropped frames**, p99 frame-time delta < 0.2 ms | Presentation-feedback jitter, with and without VCGE |
| contextd CPU | < 0.5% of one core on average; bursts < 5% | `perf stat`, cgroup `cpu.stat` |
| contextd RSS | < 80 MB (with no models, since models live in inferd) | cgroup `memory.current` |
| inferd average CPU, active work | < 2% of total CPU on 8+ cores (OCR on NPU/GPU tiers) | cgroup |
| inferd resident memory | see the [tier table](#122-tier-profiles) | cgroup |
| GPU usage | < 3% on average; 0 while a fullscreen game or video plays | NVML / `fdinfo` |
| Battery impact | < 3% extra drain per hour compared with VCGE off | RAPL + UPower over a 2-hour scripted workload |
| Captures | 0.5–3 per minute during active work, hard cap 1 every 2 s, 0 when idle | VCGE metrics |
| OCR invocations | ≤ captures; typically 0.3–2 per minute (crop + line cache) | metrics |
| OCR latency | p95 < 300 ms per capture (changed crops only) on T2–T5, < 800 ms on T1 | metrics |
| LLM calls | ≤ 200 per day (T2–T5), ≤ 60 per day (T1); batched | metrics |
| VLM calls | ≤ 6 per hour and ≤ 50 per day | metrics |
| Event → working-memory update | p99 < 50 ms | trace spans |
| Capture → text searchable | p95 < 5 s on AC; ≤ 2 min on battery (deferred) | trace spans |
| Segment close → episodic memory with summary | p95 < 2 min on AC | trace spans |
| Hybrid search, interactive | p95 < 150 ms without rerank, < 400 ms with rerank (1 year of data) | criterion + synthetic corpus |
| Startup, contextd | < 300 ms to socket ready; < 1 s to first context event | systemd-analyze, traces |
| Startup, inferd | < 1.5 s with OCR and embedding warm on CPU; NPU graphs loaded from cache < 2 s | traces |
| Storage | 15–50 MB per day text-only; 80–250 MB per day with thumbnails; default quota 10 GB | `du`, DB stats |

## 12.2 Tier profiles

| Profile | Tiers | Capture cadence | OCR | Embeddings | LLM/VLM | inferd memory (resident / peak) |
|---|---|---|---|---|---|---|
| **Low-end** | T1 (CPU only, 8–16 GB) | Triggers only; idle backstop 120 s | PP-OCRv6 tiny, CPU, 1 thread | MiniLM, CPU | Batched when on AC and idle; no local VLM; cloud or LAN only with consent | 350 MB / 1.6 GB |
| **AI PC (integrated)** | T2 Intel/AMD, T3 Snapdragon | Full trigger set; idle backstop 60 s | NPU (Intel/AMD) or CPU (Snapdragon) | NPU or CPU | 2–4B on the integrated GPU via Vulkan; VLM shares the same model | 900 MB / 4.5 GB |
| **Dedicated GPU** | T4 NVIDIA | Full; yields to games | CUDA/TRT | CUDA | 9B on CUDA; unloaded when a game starts or VRAM is under pressure | 1 GB RAM + 2 GB VRAM / +9 GB VRAM |
| **Workstation and LAN node** | T5 DGX Spark | Full | CUDA | CUDA | 27–35B class; also serves offload for paired laptops | ≤ 24 GB |

## 12.3 Adaptive processing (scheduler policy)

Each resource tier gets its own token bucket: captures, OCR, embedding, LLM, VLM and cloud. Each bucket's refill rate is the profile's base rate multiplied by a set of factors:

| Signal | Effect |
|---|---|
| On battery | ×0.3. LLM, VLM and consolidation are deferred until AC power; idle backstop rises to 300 s |
| `power-profiles-daemon` set to `power-saver` | ×0.2. Only metadata and accessibility text are collected; captures are limited to focus changes |
| CPU PSI `some avg10` > 40%, or memory PSI > 10% | Pause OCR, LLM and VLM; queues coalesce |
| Thermal headroom low (the vendor zone passes its trip point) | ×0.5 |
| A fullscreen surface, or `content_type` = game or video | Stop captures for it; GPU jobs paused; LLM model unloaded if it is on the same GPU |
| User idle more than 5 minutes, on AC | Consolidation window opens. It is abortable within 100 ms when input returns |
| Screen locked | Everything stops except consolidation, if configured |
| The user explicitly queries | Interactive lane: bypasses the buckets, at the highest priority |

**Coalescing rules:**
- For captures, the latest request wins per window.
- OCR jobs older than their deadline are dropped. The next capture covers the same content.
- Segment summarization batches up to 8 segments into one call.
- Embeddings are batched up to 32.

## 12.4 Benchmark suite (built in Phase 0, extended every phase)

| Benchmark | Tool | Gate |
|---|---|---|
| `bench-ocr`: a corpus of 500 screenshot crops (code, docs, chat, terminal, UI, CJK, dark mode). Measures character error rate and latency per execution provider and per tier | criterion + a custom harness | CER ≤ 3% on UI and code text; latency within the targets above |
| `bench-embed`: retrieval recall@10 on a synthetic "day in the life" corpus, plus throughput per EP | harness | recall@10 ≥ 0.85 |
| `bench-llm`: JSON validity rate, extraction F1 against a gold set, tokens/s per backend | harness | ≥ 99% valid JSON; entity F1 ≥ 0.7 |
| `bench-compositor`: frame-time jitter with the context module on vs off, input event latency | presentation-time traces | no regression beyond the targets |
| `soak-8h`: scripted workload (browser, editor, terminal, chat, video), replayed from traces plus real apps | headless compositor plus nested session | every target in §12.1 |
| `bench-search`: 1-year synthetic store (110k segments, 5M text lines) | criterion | p95 targets |
| `bench-power`: 2 hours on battery, VCGE on vs off | RAPL / UPower | < 3% extra drain per hour |
