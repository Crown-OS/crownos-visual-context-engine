# crownos-visual-context-engine

A memory of the user's own work, built on their own machine from what is on screen: which window
had focus, on which document, for how long, with how much typing, and later what the text said.
Nothing leaves the machine; nothing is stored unencrypted; the user can pause it, exclude apps,
and forget any stretch of time.

The design is in [`docs/`](docs/README.md): research, architecture, memory model, inference,
privacy, performance, API and the phased plan. This README is the map of the code.

| Crate | Phase | What it is |
| --- | --- | --- |
| `crates/context-core` | 1 | Observations, windows, segments, episodes, ids, time |
| `crates/context-protocol` | 1 | The `crownos-visual-context` crownos-ipc contract |
| `crates/context-policy` | 1 | `visual_context.ron`, the observe/metadata/drop gate, secret detectors |
| `crates/context-collect` | 1 | The compositor bridge (`crownos_context_v1`), the AT-SPI text reader, window table, app resolution |
| `crates/context-store` | 5 | SQLCipher store: events, segments, episodes, FTS5, forget with cascade |
| `crates/context-engine` | 4 | Heartbeat merge, segmenter, correlator, classifier, working memory |
| `crates/context-capture` | 2 | Capture scheduling and frame processing (not started) |
| `crates/context-infer` | 3 | Task API, capability traits, `models/registry.ron` and the router |
| `crates/context-infer-ort` | 3 | ONNX Runtime backend: PP-OCR and text embeddings |
| `crates/context-infer-llama` | 3 | llama.cpp backend: generation with images and JSON schemas, GGUF embeddings |
| `crates/context-infer-http` | 3 | OpenAI-compatible client for servers on this machine, the LAN or a cloud |
| `crates/context-shm` | 3 | Sealed read-only memfds for images |
| `crates/crown-context-inferd` | 3 | The model worker (`crownos-visual-context-infer`) |
| `crates/crown-contextd` | 1–5 | The daemon |
| `crates/crown-context` | 1–5 | The CLI: `status`, `tail`, `timeline`, `search`, `forget`, `pause` |
| `eval` | 0 | Synthetic traces of a working day, from a seed |

```sh
cargo test --workspace
cargo run -p context-eval -- generate --scenario short --out /tmp/day.json
cargo run -p crown-contextd -- --replay /tmp/day.json --speed 600 --state-dir /tmp/context-state
crown-context tail                 # context changes and closed segments as they happen
crown-context timeline --episodes  # today, grouped
crown-context search damage tracker
crown-context forget --last-minutes 15
```

On a live desktop the daemon binds `crownos_context_v1`, which crownpositor shows only to the
executables listed in `~/.config/crownos/agents.ron` (`trusted_executables`); a development build
has to add its own path there. Observation is off until `enabled: true` is set in
`~/.config/crownos/visual_context.ron`; a replay ignores that switch.

Sibling CrownOS repositories are path dependencies one directory up.

## Building

The llama.cpp backend compiles llama.cpp, so a build needs `cmake`, `clang` and a C++
compiler. The `vulkan` feature of `context-infer-llama` and `crown-context-inferd` also needs
`vulkan-headers`, `shaderc` (`glslc`) and `spirv-headers`; `cuda` needs the CUDA toolkit.
ONNX Runtime is not linked: `crown-context-inferd` loads `/usr/lib/libonnxruntime.so` (or
`$ORT_DYLIB_PATH`) at run time.

llama-cpp-sys-2 builds llama.cpp as shared libraries and leaves dangling `lib*.so` links in
the target directory, on which a later run of its build script panics. Run
`just clean-llama-links` before a build (`just check` does). The shared CI workflow has no
pre-build step, so CI cannot run the cleanup: a clean CI checkout builds llama.cpp once, and
only a second run of its build script in the same target directory meets the links.
The package installs libllama, libggml and the ggml device
modules in `/usr/lib/crown-context`, where `crown-context-inferd` looks for them.

