# 05 — Privacy and Security Architecture

Tags: **[V]** verified · **[R]** reported · **[A]** design decision / assumption to validate.

## 11.1 Threat model

| Adversary / failure | Example | Primary controls |
|---|---|---|
| Malware running as the user | Reads DB file; injects into a consumer UI (TotalRecall-style [R]) | SQLCipher + key only in contextd memory; key wrapped by TPM/login secret; raw scope needs fresh auth; `AppKey` caller check; rate limits; audit. Limit: unsandboxed same-user code already has screencopy, so for host apps access control is consent, not enforcement ([§11.3](#113-caller-identity-and-scopes)) |
| Sandboxed (Flatpak) app | Tries to bind `crownos_context_v1` or call the Context API | `AppKey::Sandboxed` comes from `wp_security_context_v1`, which the app cannot fake. Denied by default (`sandboxed_default: Deny`). A grant gives only its scopes |
| Agent consents for the user | A prompt-injected agent clicks "Allow" in a consent dialog | Consent dialogs ignore `crownos_input`, virtual-keyboard and agent-cursor input |
| Over-collection | Passwords, private windows, banking, DMs stored | Compositor veto, fail-closed policy gate, secret detectors, per-app permissions, no keylogging |
| Injected secret | An agent types a password through crownos-secrets-manager | The secrets daemon has the compositor flag the window sensitive before the first key and for 30 s after. Capture of that window fails |
| Frame leak | Pixels reach swap, a core dump, the hibernation image or a forked child | The frame lives only in a per-capture memfd. Models read sealed copies of regions. The frame is zeroed and closed before any model runs ([§11.6](#116-analyse-then-delete)) |
| Model hallucination becoming "memory" | LLM invents a fact | Provenance + confidence; facts derived only from observed evidence; user correction API; low-confidence expiry |
| Cloud leakage | Sensitive text uploaded | Off by default; text only; consent per category; redaction + relayd re-verification; audit |
| Device theft / offline disk | Disk image | Full-disk encryption expected + SQLCipher; no pixels on disk; TPM-sealed key bound to user login |
| VCGE bug / crash | Hang starves desktop | No inference in compositor; compositor never waits on VCGE; cgroup CPU/IO/memory limits |
| Shoulder surfing of memory UI | Inspector open | Inspector behind fresh auth; auto-lock after 5 min |

Out of scope: root/kernel compromise, compositor compromise (same trust as the display server), hardware attacks on TPM.

## 11.2 Control map (requirement → mechanism)

| Requirement | Mechanism | Where |
|---|---|---|
| **Local-first** | All models local by default; inferd `PrivateNetwork=yes`; relayd (the HTTP backend) not started until the user configures an endpoint | systemd units, C15/C16 |
| **Per-app capture permissions** | `visual_context.ron` rules per app_key: `capture: none\|metadata\|text\|frames`, `clipboard_content`, `notification_body`, `typed_text_from_a11y`. `frames` means frames are taken and analysed, then deleted | Read by compositor (veto) **and** contextd (gate) |
| **Per-app access to the context** | `context_access.ron` grants per `AppKey` and scope; first-use consent; live revocation ([§11.3](#113-caller-identity-and-scopes)) | Compositor + contextd, edited in crownos-settings |
| **Sensitive app exclusion (defaults)** | Password managers (KeePassXC, Bitwarden, 1Password, GNOME Secrets), polkit agent, lock screen, crownos-settings privacy pages, authenticator apps, system key dialogs; user-extensible | Compositor veto (pixels) + policy gate (metadata reduced to "excluded app, duration") |
| **Password / auth field detection** | text-input-v3 `ContentPurpose::Password\|Pin` [V]; AT-SPI `PasswordText` role → `flag_sensitive(window, 60 s)`, set again on each read while the field stays [V]; OCR/a11y line patterns (`[sudo] password for`, `Password:`, `Enter passphrase`, OTP prompts) [A, not built]. contextd also marks the window sensitive locally for the same time, so a capture that is already due does not read it [V] | Compositor + C2 + C6 |
| **Secret detection on extracted text** | `context-policy::find_secrets` runs on all text from a capture before it is stored. A hit drops the text and flags the window sensitive for 10 minutes. contextd also marks the window locally, so the flag holds when the compositor does not take it. The frame record keeps `secret_seen` [V] | C5, C6 |
| **Private browsing / incognito** | Firefox title suffix (localized table) [V]; optional native-messaging extension for Chromium/Firefox incognito flags; user patterns; unknown browser state on a window that previously matched → keep excluded (fail closed) | Compositor rules + C2 |
| **Sensitive windows in other captures** | Output screencopy and screencast blank sensitive windows, as per-window capture already does | Compositor |
| **Capture indicators** | crownbar shows "Context engine: observing / paused / processing"; distinct from screencast indicator (`org.crownos.ScreenCast1` [V]); compositor owns the truth of "VCGE has live capture sessions" | crownbar + C0 |
| **Pause / resume** | Compositor hotkey (works if VCGE is hung) + indicator menu + API `pause(duration)`; auto-pause on lock, idle, fullscreen game, screen-sharing to others (configurable) | C0, C14 |
| **User-configurable retention** | Retention classes R1 raw (text lines 30 d, input buckets 7 d), R2 episodic (1 y), R3 knowledge (until invalidated/decayed); per-app overrides; storage quota. Pixels have no retention class: they are never stored | C13 + C11 |
| **Inspection & deletion** | API + CLI + Inspector: browse timeline, see each memory's evidence ("why"), delete item / time range / app / entity / everything; "forget the last 15 minutes" one-click; deletion cascades ([03 §8.1](03-memory.md)) | C14, C9 |
| **Encryption at rest** | SQLCipher (AES-256) for `context.db` and `audit.db`; random 256-bit DEK wrapped by KEK from `systemd-creds --user` (TPM2-bound where available) or the user's login keyring; key held only in contextd, `mlock`ed, zeroized on exit | C13 |
| **Secure IPC** | crownos-ipc Unix socket in `$XDG_RUNTIME_DIR/crownos/` (0700); peer identity from `SO_PEERPIDFD` → `AppKey`; opt-in `SO_PASSPIDFD` sender check per message; scopes per caller. inferd serves any process of the user on its `0600` socket; its file system sandbox, not a peer check, keeps it away from the database [V]. inferd gets only region copies, in memfds sealed with `F_SEAL_WRITE\|F_SEAL_SHRINK\|F_SEAL_GROW`. It refuses a memfd without these seals [V] | C14 |
| **Explicit cloud consent** | Per category and endpoint; text only; first-use payload preview; monthly budget; kill switch | C16 |
| **Sensitive clipboard / input exclusion** | Secret hint (`x-kde-passwordManagerHint`) honoured [V]; clipboard content stored only for allowed source apps and text MIME ≤ 16 KB after secret detection; a clipboard secret flags the source window; input = aggregated counts only, zeroed in password fields; no keylogging | C0, C4, C5 |
| **Audit logs** | `audit.db` hash-chained: every API query (actor `AppKey`, scope, result count), every grant, denial and revocation, every deletion, policy change, cloud upload (hash, bytes, endpoint, purpose), pause/resume; viewable in Inspector | C14 |
| **Understandability** | Inspector "What VCGE collects" page generated from live policy; per-memory "Why do I know this?" (evidence chain); daily digest optional | C18 |

## 11.3 Caller identity and scopes

Any app may use `crownos_context_v1` and the Context API (decision d). The user controls access in crownos-settings → Privacy → App access. Both the compositor and contextd read the same `context_access.ron`.

**The honest promise.** An unsandboxed host app already has screencopy and input injection. For host apps, access control is **consent, not enforcement**. For sandboxed apps it is **enforcement**, because the sandbox engine vouches for the app id.

This is also true for the `engine` grant of `exe:/usr/bin/crown-contextd`. It holds only while root controls the file and its directory [V]. The compositor refuses `engine` to a process that started with `LD_PRELOAD` or `LD_LIBRARY_PATH`, and the contextd unit unsets both [V]. Another program of the same user can still read the screen in other ways.

### Identity: `AppKey`

| `AppKey` | Source | Strength | Grants allowed |
|---|---|---|---|
| `Sandboxed { engine, app_id }` | `wp_security_context_v1` metadata; Flatpak cgroup | Strong | Persistent |
| `Spawned { component }` | The compositor or a systemd unit hands over the connection (first-party daemons: crown-contextd, computer-use, crownos-secretsd) [A, not built: crown-contextd uses `exe:` until it exists] | Strong | Persistent |
| `HostExe { path, dev, ino }` | `SO_PEERPIDFD` → `/proc/<pid>/exe`, `fstat` compared with a root-owned path that the user cannot write | Medium | Persistent; re-stat on package upgrade |
| `Interpreted { interp, script }` | A known interpreter (python, node, bash, java) plus the script from argv | Weak | Session only, never `raw` |

Executables under `$HOME` or another user-writable path get session-only grants and never `raw`.

Resolution:
- **Compositor:** resolve the `AppKey` at accept and check it again at bind.
- **contextd:** read the peer with `SO_PEERPIDFD` instead of `pidfd_open` (no PID-reuse race). With opt-in `SO_PASSPIDFD`, drop each message whose sender is not the accept-time process. This defeats the fork-then-exec trick on the Context API [A, not tested].
- A plain exe path string, as used today, is weak. Fork-then-exec and mount namespaces can probably fake it [R, not tested].

### Scopes

| Scope | Grants | Default holders |
|---|---|---|
| `metadata` | Wayland: app_id, focus, session state, input counts. API: `status`, `current_context` without titles or text, `subscribe` to non-text topics | crownbar, shell, launcher |
| `text` | `metadata` plus titles, selection MIME types, and `search` / `timeline` / `episode` / `entity` / `context_pack` with redacted snippets | Approved agents (first-use consent) |
| `raw` | Raw text lines, exact clipboard and notification text, `export`. Needs fresh user auth (polkit `auth_self`, fprintd if enrolled) every `raw_reauth_minutes` (default 5) | Inspector only |
| `memory_write` | `remember`, `correct` | Inspector, approved agents |
| `memory_delete` | `forget` | Inspector, CLI (with auth) |
| `pause` | `pause` / `resume` of observation on the Context API, nothing else | crownbar (pause menu), crownos-settings |
| `policy_admin` | `set_policy`, change rules, retention and consent | crownos-settings, CLI (with auth) |
| `engine` | Pause all observation through `set_paused` on the Wayland protocol; take captures | `exe:/usr/bin/crown-contextd` only, while root controls the file [V] |
| `wayland.window_control`, `wayland.agent_cursor` | Today's trusted-agent Wayland globals. The compositor checks them against `context_access.ron` [V] | computer-use |

Old names in earlier drafts map as follows: `context.current` → `metadata`, `context.search` → `text`, `context.raw` → `raw`, `memory.write` → `memory_write`, `memory.delete` → `memory_delete`, `policy.admin` → `policy_admin`, `subscribe.context` → `metadata` (topics are filtered per scope). The MCP tool names in [08 Phase 9](08-implementation-plan.md#phase-9--integration-with-crownos-agents) do not change.

### `context_access.ron`

A new crownos-config section, shared by the compositor and contextd:

```ron
// ~/.config/crownos/context_access.ron
(
    ask_on_first_use: true,
    sandboxed_default: Deny,          // Deny | Ask
    host_default: Ask,                // Deny | Ask
    grants: {
        "sandbox:org.flatpak/org.example.Notes":   (scopes: [Metadata], expires: None),
        "exe:/usr/bin/crown-contextd":             (scopes: [Engine]),
        "exe:/usr/bin/suja":                       (scopes: [Metadata, Text, MemoryWrite]),
        "interp:/usr/bin/python3:/home/u/tool.py": (scopes: [Metadata], expires: Some("session")),
    },
    denied: ["exe:/opt/sketchy/bin/app"],
    raw_reauth_minutes: 5,
)
```

Migration from `agents.ron`:
- The `/usr/bin/crown-contextd` entry of `trusted_executables` becomes `exe:/usr/bin/crown-contextd` with `engine`. It keeps `engine` only while root controls the file [V].
- Every other `trusted_executables` entry becomes `exe:` with `metadata`, `text`, `pause`, `wayland.window_control` and `wayland.agent_cursor`. A migrated entry never gets `raw`, `engine` or `policy_admin`.
- `app_access` is not copied. It records which apps show their own UI to agents, not which apps may read context [V].

 `agents`, `visual_context` and `context_access` are added to the by-name config API and to the crownos-settings section list.

### First use, revocation and audit

- **First use.** When an app with no grant binds, the compositor parks the bind. When it calls the Context API, contextd parks the call. contextd runs the root-owned `/usr/lib/crownos-portal-dialog` itself (`crownos_config::consent_dialog`) and reads the answer from its exit status only [V]. No D-Bus name and no output stream is in this path, so another process of the user cannot answer "Allow" by taking the portal's bus name or by writing to the dialog's stdout. An API caller inside a Flatpak sandbox gets the same sandboxed key as in the compositor (`AppKey::of_sandboxed_process`) [V]. Debug builds can name a development dialog in `CROWN_CONSENT_DIALOG`. The compositor asks with the same helper. The dialog returns the choices "remember" and the scope. A question waits at most 120 s. After a deny that the user did not remember, the app is not asked again for 10 minutes [V]. The dialog ignores synthetic input from `crownos_input`, the virtual keyboard and agent cursors, so an agent cannot click "Allow".
- **Live revocation.** On a config change, the compositor and contextd recompute the scope of every live client. Wayland clients get the `revoked` event and an inert object ([07 §13.1](07-api.md#131-transports)). IPC peers are dropped with their subscriptions. Requests are checked one by one, so a revocation takes effect on the next message.
- **Audit.** Grants, denials, revocations, binds and the first use per session go to `crownpositor::audit::context` and to `audit.db` with the `AppKey` as actor.
- **Agent bridge.** Agents that act through `crown-context-mcp` get the **agent's** grant, not the bridge's [A].

## 11.4 Process hardening (systemd user units)

| Unit | Hardening |
|---|---|
| `crown-contextd` | `NoNewPrivileges`, `RestrictAddressFamilies=AF_UNIX`, `RestrictNamespaces=yes`, no file system or network namespace options (see below), `MemoryDenyWriteExecute=yes`, `LockPersonality`, `SystemCallFilter=@system-service`, `CPUWeight=20`, `IOWeight=20`, `MemoryHigh=256M`, `Nice=10`. `UnsetEnvironment=LD_PRELOAD LD_LIBRARY_PATH`. Frame protection: `LimitCORE=0`, `MemorySwapMax=0`, `MemoryZSwapMax=0` [V]. No `LimitMEMLOCK`: the user manager ignores it above its own limit (8 MiB), so `mlock` is best effort. contextd faults each frame in at allocation, so the pages count against this cgroup, where swap is off [V]. In the binary: `prctl(PR_SET_DUMPABLE, 0)` at startup. contextd is dumpable again only while it connects to Wayland, for at most ~3 s, so that the compositor can read `/proc/<pid>/exe`. Captures are held during that time. This is temporary until a compositor-spawned identity exists [V] |
| `crown-context-inferd` | `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome=read-only`, `PrivateTmp=yes`, `PrivateNetwork=yes`, `RestrictAddressFamilies=AF_UNIX`, `InaccessiblePaths=-%S/crownos/context` (contextd's state directory: its database and its key), `ReadWritePaths=` for its cache only, `SystemCallFilter=@system-service`, device access limited to GPU/NPU nodes (`DeviceAllow=` [A: the user manager may not apply device rules]), `MemoryHigh=6G`, `MemoryMax=8G`. Frame protection: `LimitCORE=0`, `MemorySwapMax=0`, `MemoryZSwapMax=0`, no `LimitMEMLOCK` [V]. inferd does not call `mlock` and does not turn off `PR_SET_DUMPABLE` [V]. inferd parses pixels with ONNX Runtime and llama.cpp, so this sandbox is the boundary that matters; `crown-contextd/tests/user_units.rs` checks that the database and the key stay closed to it [V] |

| `crown-context-relayd` | Hosts the HTTP backend. `IPAddressDeny=any` + `IPAddressAllow=` configured endpoints (localhost sidecars, LAN node, cloud) [A: needs BPF support in the user manager], no filesystem write, text only, started only when an endpoint is configured |

**Hardening regression in crown-contextd.service.** The contextd unit has no option that makes a user namespace (`ProtectSystem=`, `ProtectHome=`, `ReadWritePaths=`, `PrivateTmp=`, `PrivateNetwork=` and the like). This is a step back: before, the unit could not write outside its state and cache directories and had no network. In a user unit each of these options puts the process in a new user namespace. The kernel then refuses the ptrace-read of `/proc/<peer>/exe`, so contextd could not identify an API caller, and the scoped Context API needs that identity. The user also owns that namespace, so another process of the user could attach in spite of `PR_SET_DUMPABLE=0`. `RestrictAddressFamilies=AF_UNIX` takes the place of `PrivateNetwork=` [V]. `crown-contextd/tests/user_units.rs` shows that `PrivateTmp=yes` alone hides the caller [V]. Follow-up: a Landlock self-sandbox in contextd needs no namespace and would bring the file isolation back [A].

## 11.5 Data minimization defaults (decision a)

| Data | Default | Options |
|---|---|---|
| Window metadata, segments, summaries | On | — |
| Screen text (a11y/OCR) | On, 30 d | 0–365 d, per app |
| Pixels (frames) | **Never stored.** Analysed at capture time, then deleted | — |
| VLM caption of low-text frames | On where the tier allows it; off on battery | off |
| Clipboard content | Off (metadata only) | per app allowlist |
| Notification body | Off for messaging/email categories, on for others | per app |
| Typed text (from a11y editable fields) | **Off** | per app allowlist |
| Input activity | Aggregated buckets, 7 d | off |
| Cloud | **Off** | per category |

## 11.6 Analyse, then delete

Each capture is analysed at once. VCGE keeps the text and metadata and deletes the pixels. Nothing waits for later.

One content gate in contextd (`text_gate.rs`) decides for capture targets, for the store of capture output and for the a11y collector. It checks in this order: enabled, paused (engine and compositor), locked, access, policy, local sensitivity. Access is the scope that the compositor granted the engine. Text needs `text` or `engine`. Captures need `raw` or `engine`. A revoke or a disconnect clears it [V].

### Per capture

1. Compositor damage from `ext-image-copy-capture` → 32 px xxh3 tile hashes inside the damage → per-app volatile masks → union crop with a line margin.
2. Text sources in order: CrownUI tree → AT-SPI → OCR of changed crops. No CrownUI tree reader exists yet, so CrownUI apps are read through AT-SPI [V]. The line cache is keyed by a line-crop hash and stores text only, never pixels. The a11y collector keeps its last reading of each window. A reading counts only when its read started after the capture request [V]. The capture looks for one without a wait:
   - Text: the capture uses the a11y lines and runs no OCR. It does not report the lines again.
   - Password field: the frame is dropped before any pixel is read. There are no copies, no OCR, no caption and no frame record.
   - No text (the read was cut short, failed, or found no text, or the window has no a11y application): OCR reads the changed crops.
   - No reading yet: the capture asks the collector to read the window now. It makes its tiles and its sealed region copies, and zeroes the frame. Then it waits up to 300 ms for the reading. A pause, a lock or a sleep stops the wait at once. Text drops the OCR copy unread. A password field drops all copies and gives no frame record [V].
   The collector records "no text" at once for a window without an a11y application, so such a capture does not wait [V].
3. contextd copies the rows of each region that a model reads into a new memfd and seals it against all changes. Then the frame is zeroed and closed, before any model runs [V].
4. VLM JSON `{content_kind, caption, visible_entities[]}` (≤ 128 tokens) only when the text is under ~200 characters or image regions cover over ~40 % of the window. At least 50 % of the window must be damaged; a first capture counts as 100 %. Not on battery. The VLM reads a sealed copy of the whole frame [V].
5. Secret detectors run on the title, all extracted text and the caption. A hit drops the text and the caption, flags the window sensitive for 10 minutes and sets `secret_seen` in the frame record [V].
6. The output waits in RAM for the segment of its window, for at most 30 s. Then it is dropped [V].

### Frame memory

- One memfd per capture: `memfd_create(MFD_CLOEXEC|MFD_ALLOW_SEALING|MFD_NOEXEC_SEAL)`, `madvise(MADV_DONTDUMP|MADV_DONTFORK)` (`MADV_WIPEONFORK` is valid only for private anonymous mappings) [V].
- The pages are faulted in at allocation (`MADV_POPULATE_WRITE`). So they count against the contextd cgroup, and `MemorySwapMax=0` keeps them out of swap [V].
- `mlock` is best effort. The units cannot raise `RLIMIT_MEMLOCK` above the user manager's limit (8 MiB), so `LimitMEMLOCK` is in neither unit [V].
- After capture, seal the capture memfd with `F_SEAL_FUTURE_WRITE|F_SEAL_SHRINK|F_SEAL_GROW`. `F_SEAL_WRITE` fails with `EBUSY` while a writable shared mapping exists [R, man page], and contextd keeps one to zero the frame. The capture memfd never leaves contextd [V].
- Models read region copies: new memfds sealed with `F_SEAL_WRITE|F_SEAL_SHRINK|F_SEAL_GROW|F_SEAL_SEAL` and shared read-only. inferd refuses a memfd without `F_SEAL_WRITE|F_SEAL_SHRINK|F_SEAL_GROW`. It maps the memfd read-only, analyses and unmaps [V].
- Only the capture memfd is zeroed. contextd overwrites it with zeros before it unmaps and closes it. Region copies and inferd job memfds are closed, not zeroed [V].
- **Queue:** RAM only, at most 3 frames and 64 MB, latest wins per window, 60 s TTL. An expired frame is dropped and counted as `frame_dropped_unanalysed`. It is never written to disk.
- **Holds:** captures are held for each reason on its own: sleep, pause, lock, and identity exposure while contextd connects to Wayland. Waking from sleep does not end a pause. A hold stops contextd's wait for an inferd call at once, and contextd sends `cancel(job_id)` to inferd [V].
- **Cancel in inferd:** inferd cancels a job on `cancel(job_id)` and when the connection of its client closes (checked every 100 ms). A cancelled job that waits for its model does not run, and its memfds are unmapped and closed at once. A model call that already runs cannot be stopped from outside. Its mapping stays until the call returns, within the job deadline and the hang budget [V].
- **Hibernation:** `mlock` does not keep pages out of the hibernation image. contextd and inferd each take a logind `PrepareForSleep` delay inhibitor. On `PrepareForSleep(true)`, contextd holds captures, ends the running analysis and zeroes the frames. inferd refuses new jobs and cancels all jobs: the jobs that wait for a model end at once and unmap their memfds. inferd waits up to 4 s only for model calls that already run. Then each process releases its inhibitor [V].
- **Inference buffers:** inferd owns the input buffers (ORT IoBinding) and clears them after each run. The CPU arena is off for image models.
- `memfd_secret` is optional extra hardening. Its use as a `wl_shm` pool is not tested [A].

Residual risks: the compositor's own readback buffers are not zeroed. Region copies and inferd job memfds are not zeroed either: their pages go back to the kernel when the last descriptor closes. Runtime arenas and device buffers can hold activations derived from pixels.

### What analyse-then-delete removes

The user accepted these losses:
- Thumbnails and the encrypted media directory.
- Image embeddings and visual search ("the slide with the diagram").
- VLM description at segment close. The VLM now runs at capture time, on low-text frames only.
- Re-analysis of old images after a model change. Re-embedding and re-summarising of stored **text** still work.
- Battery deferral of OCR. On battery, OCR still runs at capture time on the NPU or CPU. Only the VLM is skipped.
- The `raw` scope and `export` no longer include any image.

### crownos-secrets-manager (interface)

crownos-secrets-manager is a separate component. It stores passwords and API keys and lets agents use them by name. VCGE has one interface with it: before the secrets daemon types the first key of a secret, the compositor flags the target window sensitive (and keeps the flag for 30 s after). Capture of that window fails, input counts are zeroed and selection events are suppressed. VCGE never sees the value.
