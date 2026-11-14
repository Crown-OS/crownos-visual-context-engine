# 05 — Privacy and Security Architecture

Tags: **[V]** verified · **[R]** reported · **[A]** design decision / assumption to validate.

## 11.1 Threat model

| Adversary / failure | Example | Primary controls |
|---|---|---|
| Malware running as the user | Reads DB file; injects into a consumer UI (TotalRecall-style [R]) | SQLCipher + key only in contextd memory; key wrapped by TPM/login secret; raw scopes need fresh auth; pidfd caller verification; rate limits; audit |
| Sandboxed (Flatpak) app | Tries to bind capture / context globals or connect to API | Compositor denies privileged globals to security-context clients [V]; API rejects callers whose cgroup is a sandbox scope unless granted |
| Over-collection | Passwords, private windows, banking, DMs stored | Compositor veto, fail-closed policy gate, secret detectors, per-app permissions, no keylogging |
| Model hallucination becoming "memory" | LLM invents a fact | Provenance + confidence; facts derived only from observed evidence; user correction API; low-confidence expiry |
| Cloud leakage | Sensitive text uploaded | Off by default; consent per category; redaction + relay re-verification; LAN-first; audit |
| Device theft / offline disk | Disk image | Full-disk encryption expected + SQLCipher + encrypted media; TPM-sealed key bound to user login |
| VCGE bug / crash | Hang starves desktop | No inference in compositor; compositor never waits on VCGE; cgroup CPU/IO/memory limits |
| Shoulder surfing of memory UI | Inspector open | Inspector behind fresh auth; auto-lock after 5 min |

Out of scope: root/kernel compromise, compositor compromise (same trust as the display server), hardware attacks on TPM.

## 11.2 Control map (requirement → mechanism)

| Requirement | Mechanism | Where |
|---|---|---|
| **Local-first** | All models local by default; inferd `PrivateNetwork=yes`; relayd not installed/started without consent | systemd units, C15/C16 |
| **Per-app capture permissions** | `visual_context.ron` rules per app_key: `capture: none\|metadata\|text\|pixels`, `clipboard_content`, `notification_body`, `typed_text_from_a11y` | Read by compositor (veto) **and** contextd (gate) |
| **Sensitive app exclusion (defaults)** | Password managers (KeePassXC, Bitwarden, 1Password, GNOME Secrets), polkit agent, lock screen, crownos-settings privacy pages, authenticator apps, system key dialogs; user-extensible | Compositor veto (pixels) + policy gate (metadata reduced to "excluded app, duration") |
| **Password / auth field detection** | text-input-v3 `ContentPurpose::Password\|Pin` [V]; AT-SPI `PasswordText` role [V]; OCR/a11y line patterns (`[sudo] password for`, `Password:`, `Enter passphrase`, OTP prompts) → `flag_sensitive(window, 30 s)` | Compositor + C2 + C6 |
| **Private browsing / incognito** | Firefox title suffix (localized table) [V]; optional native-messaging extension for Chromium/Firefox incognito flags; user patterns; unknown browser state on a window that previously matched → keep excluded (fail closed) | Compositor rules + C2 |
| **Capture indicators** | crownbar shows "Context engine: observing / paused / processing"; distinct from screencast indicator (`org.crownos.ScreenCast1` [V]); compositor owns the truth of "VCGE has live capture sessions" | crownbar + C0 |
| **Pause / resume** | Compositor hotkey (works if VCGE is hung) + indicator menu + API `pause(duration)`; auto-pause on lock, idle, fullscreen game, screen-sharing to others (configurable) | C0, C14 |
| **User-configurable retention** | Retention classes R1 raw (text lines/frames/input buckets: default 30 d / thumbnails 7 d / input 7 d), R2 episodic (1 y), R3 knowledge (until invalidated/decayed); per-app overrides; storage quota | C13 + C11 |
| **Inspection & deletion** | API + CLI + Inspector: browse timeline, see each memory's evidence ("why"), delete item / time range / app / entity / everything; "forget the last 15 minutes" one-click; deletion cascades ([03 §8.1](03-memory.md)) | C14, C9 |
| **Encryption at rest** | SQLCipher (AES-256) for `context.db` and `audit.db`; media files XChaCha20-Poly1305; random 256-bit DEK wrapped by KEK from `systemd-creds --user` (TPM2-bound where available) or the user's login keyring; key held only in contextd, `mlock`ed, zeroized on exit | C13 |
| **Secure IPC** | crownos-ipc Unix socket in `$XDG_RUNTIME_DIR/crownos/` (0700), `SO_PEERCRED` [V] + pidfd → exe path + cgroup app id; scopes per caller; memfd payloads sealed (`F_SEAL_WRITE`) before passing to inferd | C14 |
| **Explicit cloud consent** | Per category and destination; first-use payload preview; monthly budget; kill switch | C16 |
| **Sensitive clipboard / input exclusion** | Secret hint (`x-kde-passwordManagerHint`) honoured [V]; clipboard content stored only for allowed source apps and text MIME ≤ 16 KB after secret detection; input = aggregated counts only, zeroed in password fields; no keylogging | C0, C4, C5 |
| **Audit logs** | `audit.db` hash-chained: every API query (caller, scope, result count), every deletion, policy change, cloud upload (hash, bytes, destination, purpose), pause/resume; viewable in Inspector | C14 |
| **Understandability** | Inspector "What VCGE collects" page generated from live policy; per-memory "Why do I know this?" (evidence chain); daily digest optional | C18 |

## 11.3 Caller identity and scopes

| Scope | Grants | Default holders |
|---|---|---|
| `context.current` | Current context (app, doc_key, project, activity label, recent entities) — **no raw text** | CrownOS shell, launcher, crownbar, approved agents |
| `context.search` | Search over summaries, episodes, entities, facts; snippets redacted | Approved agents (grant via consent dialog) |
| `context.raw` | Raw text lines, thumbnails, exact clipboard/notification text | Inspector only; **fresh user auth** (polkit `auth_self`, fprintd if enrolled) per 5-min session |
| `memory.write` | Create/correct memories (user-asserted) | Inspector, approved agents |
| `memory.delete` | Delete/forget | Inspector, CLI (with auth) |
| `policy.admin` | Change rules, retention, consent | crownos-settings, CLI (with auth) |
| `subscribe.context` | Context change stream | shell components, approved agents |

Grants stored in `visual_context.ron` (`grants: { app_key: [scopes] }`), shown and revocable in settings. Agents acting via `crown-context-mcp` inherit the **agent's** grant, not the bridge's [A].

## 11.4 Process hardening (systemd user units)

| Unit | Hardening |
|---|---|
| `crown-contextd` | `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome=read-only` + `ReadWritePaths=%S/crownos/context %C/crownos/context`, `PrivateNetwork=yes`, `RestrictAddressFamilies=AF_UNIX`, `MemoryDenyWriteExecute=yes`, `LockPersonality`, `SystemCallFilter=@system-service`, `CPUWeight=20`, `IOWeight=20`, `MemoryHigh=256M`, `Nice=10` |
| `crown-context-inferd` | As above, plus no access to `%S/crownos/context` (no DB/media), device access limited to GPU/NPU nodes (`DeviceAllow=/dev/dri/* /dev/accel/* /dev/nvidia*`), `MemoryHigh` per tier, `CPUSchedulingPolicy=idle` for batch jobs (interactive jobs use a separate high-priority thread), in-process **Landlock** ruleset for model dir + memfds [A: verify GPU driver compatibility with `MemoryDenyWriteExecute`] |
| `crown-context-relayd` | `IPAddressDeny=any` + `IPAddressAllow=` configured destinations, no filesystem write, started only when consent exists |

## 11.5 Data minimization defaults (reviewer decision #1)

| Data | Default | Options |
|---|---|---|
| Window metadata, segments, summaries | On | — |
| Screen text (a11y/OCR) | On, 30 d | 0–365 d, per app |
| Thumbnails | **On, 7 d, 1280 px** | off / 1–90 d / full-res |
| Clipboard content | Off (metadata only) | per app allowlist |
| Notification body | Off for messaging/email categories, on for others | per app |
| Typed text (from a11y editable fields) | **Off** | per app allowlist |
| Input activity | Aggregated buckets, 7 d | off |
| Image embeddings | Off on T1/T3, on T2/T4/T5 | — |
| Cloud | **Off** | per category |
