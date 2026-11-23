#!/usr/bin/env python3
"""Measure what AT-SPI costs the apps, in four states.

Chromium and Electron show no tree below their frames with IsEnabled alone, so
each is also measured as "<app>-basic", started with
--force-renderer-accessibility=basic.

States, each with a fresh private session and a fresh app:
  off        IsEnabled false
  enabled    IsEnabled true, no listener
  listening  IsEnabled true, the collector registers its events and reads nothing
  reading    IsEnabled true, the collector reads the app's window

Every process runs in a private XDG_RUNTIME_DIR under /tmp, with its own
dbus-daemon, a11y bus and registry, and GSETTINGS_BACKEND=memory. The live
session under /run/user is never used. The apps need a nested compositor that
runs in a private runtime dir too: pass its absolute socket with --display.

For each app and state the script records the PSS and the CPU time of the app's
processes, of the session bus and registry, and of the collector. A group is the
processes the script started, the members of their own sessions, and their
descendants, so the measurement needs no cgroup and no call to the user's
systemd. The a11y bus launcher leaves its parent's session and is not counted.
VS Code starts from its Electron main process, which the `code` launcher would
detach into a session of its own.

  eval/a11y-cost/run.py --display /tmp/vcge-nested/wayland-1 \\
      --driver target/release/a11y-cost --seconds 600
"""

import argparse
import itertools
import json
import os
import re
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

STATES = ["off", "enabled", "listening", "reading"]
CLOCK_TICKS = os.sysconf("SC_CLK_TCK")
SAMPLE_EVERY = 5.0
HERE = Path(__file__).resolve().parent
# The Electron main process itself: the `code` launcher detaches it into a session of its own.
VSCODE_MAIN = "/usr/share/code/code"

WEB_WORKLOAD = """<!doctype html>
<html><head><meta charset="utf-8"><title>Cost workload</title></head>
<body><main><article><header><h1>Release notes for the update daemon</h1></header>
<section id="sections"></section>
<section><h2>Draft</h2><textarea id="draft" rows="8" cols="80"></textarea></section>
</article></main>
<script>
const words = "the update daemon stages each release in a new snapshot and rEFInd boots the last good one when the health check fails".split(" ");
const sections = document.getElementById("sections");
function nested(depth, text) {
  let outer = document.createElement("div"), inner = outer;
  for (let level = 0; level < depth; level++) { const child = document.createElement("div"); inner.appendChild(child); inner = child; }
  const paragraph = document.createElement("p"); paragraph.textContent = text; inner.appendChild(paragraph);
  return outer;
}
function sentence(seed) { return Array.from({length: 24}, (_, i) => words[(seed * 7 + i) % words.length]).join(" ") + "."; }
for (let index = 0; index < 40; index++) {
  const heading = document.createElement("h3"); heading.textContent = "Section " + index;
  sections.appendChild(heading); sections.appendChild(nested(index % 8, sentence(index)));
}
const list = document.createElement("ul");
for (let index = 0; index < 30; index++) { const item = document.createElement("li"); item.textContent = sentence(index + 50); list.appendChild(item); }
sections.appendChild(list);
const draft = document.getElementById("draft"); let typed = 0;
setInterval(() => { draft.value += words[typed++ % words.length] + " "; }, 150);
let appended = 100;
setInterval(() => { sections.appendChild(nested(appended % 8, sentence(appended++))); }, 5000);
setInterval(() => { window.scrollBy(0, 120); if (window.scrollY + innerHeight >= document.body.scrollHeight) window.scrollTo(0, 0); }, 2000);
</script></body></html>
"""

TEXT_WORKLOAD_LINE = "the update daemon stages each release in a new snapshot and boots the last good one\n"


def app_commands(scratch):
    """The apps to measure, as argv lists, with their workload kind."""
    web = f"file://{scratch / 'workload.html'}"
    text = str(scratch / "workload.txt")
    profile = scratch / "profile"
    candidates = {
        "gnome-text-editor": (["gnome-text-editor", "--standalone", text], "text", {}),
        "kate": (["kate", "--new", text], "text", {"QT_QPA_PLATFORM": "wayland"}),
        "chromium": ([
            "chromium", "--ozone-platform=wayland", f"--user-data-dir={profile}",
            "--no-first-run", "--no-default-browser-check", "--password-store=basic",
            "--disable-sync", "--disable-background-networking", web,
        ], "web", {}),
        "code": ([
            VSCODE_MAIN if Path(VSCODE_MAIN).exists() else "code", "--ozone-platform=wayland", f"--user-data-dir={profile}",
            f"--extensions-dir={scratch / 'extensions'}", "--disable-extensions",
            "--skip-welcome", "--skip-release-notes", "--disable-workspace-trust",
            "--disable-telemetry", "--new-window", text,
        ], "text", {}),
        "firefox": (["firefox", "--profile", str(profile), "--no-remote", web], "web",
                    {"MOZ_ENABLE_WAYLAND": "1"}),
    }
    for name in ("chromium", "code"):
        argv, workload, extra = candidates[name]
        candidates[f"{name}-basic"] = (
            [argv[0], "--force-renderer-accessibility=basic", *argv[1:]], workload, extra)
    return {name: spec for name, spec in candidates.items() if shutil.which(spec[0][0])}


GROUP_ROOTS = {}
GROUP_SESSIONS = {}


def adopt(group, process, own_session=False):
    """Counts `process` and its descendants in `group`, and, when it started its own session,
    every process of that session. Nothing else is matched, so a reused pid or a process of
    the user's own session can never land in a group. A root counts only while it runs, so its
    pid cannot be reused into the group."""
    GROUP_ROOTS.setdefault(group, []).append(process)
    if own_session:
        GROUP_SESSIONS.setdefault(group, set()).add(process.pid)
    return process


def marked(group):
    """The pids of a group: its root processes, the members of their own sessions, and every
    descendant of those."""
    roots = {process.pid for process in GROUP_ROOTS.get(group, []) if process.poll() is None}
    sessions = GROUP_SESSIONS.get(group, set())
    parents, members = {}, set()
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        pid = int(entry)
        try:
            fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        except (OSError, IndexError):
            continue
        parents[pid] = int(fields[1])
        if pid in roots or int(fields[3]) in sessions:
            members.add(pid)
    grown = True
    while grown:
        grown = False
        for pid, parent in parents.items():
            if pid not in members and parent in members:
                members.add(pid)
                grown = True
    members.discard(os.getpid())
    return sorted(members)


def cpu_ticks(pids):
    total = 0
    for pid in pids:
        try:
            fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
            total += int(fields[11]) + int(fields[12])
        except (OSError, IndexError, ValueError):
            continue
    return total


def pss_kib(pids):
    total = 0
    for pid in pids:
        try:
            for line in Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines():
                if line.startswith("Pss:"):
                    total += int(line.split()[1])
                    break
        except (OSError, ValueError):
            continue
    return total


class PrivateSession:
    """A dbus-daemon, a11y bus and registry that live only in a runtime dir under /tmp."""

    def __init__(self, run_id, accessible):
        self.runtime = Path(tempfile.mkdtemp(prefix="vcge-cost-", dir="/tmp"))
        self.group = f"infra-{run_id}"
        self.env = clean_env(self.runtime, self.group)
        self.daemon = adopt(self.group, subprocess.Popen(
            ["dbus-daemon", "--session", "--nofork", "--print-address=1"],
            env=self.env, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True))
        self.address = self.daemon.stdout.readline().strip()
        self.registry = None
        try:
            self.start_accessibility(accessible)
        except BaseException:
            self.close()
            raise

    def start_accessibility(self, accessible):
        if "/run/user/" in self.address:
            raise SystemExit(f"refusing a live session bus: {self.address}")
        busctl(self.address, "set-property", "org.a11y.Bus", "/org/a11y/bus",
               "org.a11y.Status", "IsEnabled", "b", "true" if accessible else "false")
        reply = busctl(self.address, "call", "org.a11y.Bus", "/org/a11y/bus",
                       "org.a11y.Bus", "GetAddress")
        self.a11y = re.match(r's "(.*)"', reply).group(1)
        guard_private(self.a11y, self.runtime)
        registry_env = dict(self.env, AT_SPI_BUS_ADDRESS=self.a11y,
                            DBUS_SESSION_BUS_ADDRESS=self.address)
        self.registry = adopt(self.group, subprocess.Popen(
            ["/usr/lib/at-spi2-registryd"], env=registry_env,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        for _ in range(50):
            if "org.a11y.atspi.Registry" in busctl(self.a11y, "list", "--acquired"):
                break
            time.sleep(0.1)

    def close(self):
        for pid in marked(self.group):
            if pid not in (self.daemon.pid, getattr(self.registry, "pid", None)):
                try:
                    os.kill(pid, signal.SIGTERM)
                except OSError:
                    pass
        for process in (self.registry, self.daemon):
            if process is None:
                continue
            process.terminate()
            try:
                process.wait(5)
            except subprocess.TimeoutExpired:
                process.kill()
        unmount_and_remove(self.runtime)


def clean_env(runtime, group):
    env = {name: value for name, value in os.environ.items()
           if name not in ("DISPLAY", "WORKSPACE", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS",
                           "AT_SPI_BUS_ADDRESS")}
    for name, sub in (("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"),
                      ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state")):
        env[name] = str(runtime / sub)
    (runtime / "home").mkdir(exist_ok=True)
    env.update(XDG_RUNTIME_DIR=str(runtime), HOME=str(runtime / "home"),
               GSETTINGS_BACKEND="memory")
    return env


def guard_private(address, runtime):
    if "/run/user/" in address or str(runtime) not in address:
        raise SystemExit(f"refusing a bus outside the private runtime dir: {address}")


def busctl(address, *args):
    return subprocess.run(["busctl", f"--address={address}", *args], check=True,
                          capture_output=True, text=True).stdout


def unmount_and_remove(directory):
    for mount in Path("/proc/self/mounts").read_text().splitlines():
        point = mount.split()[1]
        if point.startswith(str(directory)):
            subprocess.run(["fusermount3", "-u", point], capture_output=True)
    shutil.rmtree(directory, ignore_errors=True)


def connection_pid(address, name):
    reply = subprocess.run(["busctl", f"--address={address}", "call", "org.freedesktop.DBus",
                            "/org/freedesktop/DBus", "org.freedesktop.DBus",
                            "GetConnectionUnixProcessID", "s", name],
                           capture_output=True, text=True)
    return int(reply.stdout.split()[1]) if reply.returncode == 0 else None


def application(session, group, timeout=30.0):
    """The pid and unique name of the app's connection that the registry lists."""
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        pids = set(marked(group))
        owner = busctl(session.a11y, "call", "org.freedesktop.DBus", "/org/freedesktop/DBus",
                       "org.freedesktop.DBus", "GetNameOwner", "s", "org.a11y.atspi.Registry")
        registry = re.match(r's "(.*)"', owner).group(1)
        children = busctl(session.a11y, "call", registry, "/org/a11y/atspi/accessible/root",
                          "org.a11y.atspi.Accessible", "GetChildren")
        for name in re.findall(r'"(:[0-9.]+)"', children):
            pid = connection_pid(session.a11y, name)
            if pid in pids:
                return pid, name
        time.sleep(0.5)
    return None, None


def stop_group(group):
    for pid in marked(group):
        try:
            os.kill(pid, signal.SIGKILL)
        except OSError:
            pass


def measure(app, spec, state, args, run_id, scratch):
    argv, workload, extra = spec
    session = PrivateSession(run_id, accessible=state != "off")
    app_group, driver_group = f"app-{run_id}", f"driver-{run_id}"
    driver_log = scratch / f"{app}-{state}-driver.log"
    try:
        return run_workload(app, state, argv, workload, extra, args, session, app_group,
                            driver_group, driver_log, scratch)
    finally:
        for driver in GROUP_ROOTS.get(driver_group, []):
            if driver.poll() is None:
                driver.kill()
                driver.wait()
        stop_group(app_group)
        session.close()


def run_workload(app, state, argv, workload, extra, args, session, app_group, driver_group,
                 driver_log, scratch):
    app_env = dict(session.env, DBUS_SESSION_BUS_ADDRESS=session.address,
                   WAYLAND_DISPLAY=args.display, **extra)
    text = scratch / "workload.txt"
    text.write_text(TEXT_WORKLOAD_LINE * 40)
    adopt(app_group, subprocess.Popen(argv, env=app_env, stdout=subprocess.DEVNULL,
                                      stderr=subprocess.DEVNULL, start_new_session=True),
          own_session=True)
    driver = None
    if state in ("listening", "reading"):
        driver_env = dict(session.env, RUST_LOG="context_collect=debug")
        driver_argv = [args.driver, "--bus", session.a11y]
        if state == "reading":
            pid, _ = application(session, app_group)
            driver_argv += ["read", "--pid", str(pid or 0)]
        else:
            driver_argv += ["listen"]
        driver = adopt(driver_group, subprocess.Popen(
            driver_argv, env=driver_env, stdout=subprocess.PIPE,
            stderr=open(driver_log, "w"), text=True))
    time.sleep(args.warmup)
    groups = {"app": app_group, "session services": session.group, "collector": driver_group}
    start = {name: cpu_ticks(marked(group)) for name, group in groups.items()}
    samples = {name: [] for name in groups}
    began = time.monotonic()
    for tick in itertools.count():
        elapsed = time.monotonic() - began
        if elapsed >= args.seconds:
            break
        if workload == "text" and tick % 2 == 0:
            with text.open("a") as file:
                file.write(TEXT_WORKLOAD_LINE)
        for name, group in groups.items():
            samples[name].append(pss_kib(marked(group)))
        time.sleep(min(SAMPLE_EVERY, args.seconds - elapsed))
    seconds = time.monotonic() - began
    end = {name: cpu_ticks(marked(group)) for name, group in groups.items()}
    reads = 0
    if driver:
        driver.terminate()
        output, _ = driver.communicate(timeout=10)
        reads = output.count("text lines=")
    log = driver_log.read_text() if driver_log.exists() else ""
    structures = re.findall(r"nodes=(\d+) complete=(true|false)", log)
    result = {
        "app": app, "state": state, "seconds": round(seconds, 1), "reads_sent": reads,
        "structures": len(structures),
        "structures_complete": sum(1 for _, complete in structures if complete == "true"),
        "largest_structure": max((int(nodes) for nodes, _ in structures), default=0),
    }
    for name in groups:
        result[f"{name} cpu %"] = round(100 * (end[name] - start[name]) / CLOCK_TICKS / seconds, 2)
        result[f"{name} pss MiB"] = (round(statistics.mean(samples[name]) / 1024, 1)
                                     if samples[name] else 0)
    return result


def summary(results, apps_missing):
    lines = ["| app | state | app CPU % | app PSS MiB | Δ CPU vs off | Δ PSS vs off | session services CPU % | collector CPU % | collector PSS MiB | reads | complete structures |",
             "|---|---|---|---|---|---|---|---|---|---|---|"]
    for result in results:
        base = next(r for r in results if r["app"] == result["app"] and r["state"] == "off")
        cpu_delta = result["app cpu %"] - base["app cpu %"]
        pss_delta = 100 * (result["app pss MiB"] / base["app pss MiB"] - 1) if base["app pss MiB"] else 0
        lines.append(
            f"| {result['app']} | {result['state']} | {result['app cpu %']} | {result['app pss MiB']} "
            f"| {cpu_delta:+.2f} pt | {pss_delta:+.1f} % | {result['session services cpu %']} "
            f"| {result['collector cpu %']} | {result['collector pss MiB']} | {result['reads_sent']} "
            f"| {result['structures_complete']}/{result['structures']} |")
    if apps_missing:
        lines.append(f"\nNot installed: {', '.join(apps_missing)}.")
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--display", required=True,
                        help="absolute Wayland socket of a nested compositor under /tmp")
    parser.add_argument("--driver", required=True, help="the a11y-cost binary")
    parser.add_argument("--seconds", type=float, default=600)
    parser.add_argument("--warmup", type=float, default=20)
    parser.add_argument("--apps", nargs="*")
    parser.add_argument("--out", default=str(HERE.parent / "results"))
    args = parser.parse_args()
    if not args.display.startswith("/tmp/"):
        raise SystemExit("--display must be a nested compositor's socket under /tmp")
    args.driver = str(Path(args.driver).resolve())
    scratch = Path(tempfile.mkdtemp(prefix="vcge-cost-scratch-", dir="/tmp"))
    (scratch / "workload.html").write_text(WEB_WORKLOAD)
    apps = app_commands(scratch)
    wanted = args.apps or ["gnome-text-editor", "kate", "chromium", "chromium-basic", "code",
                           "code-basic", "firefox"]
    missing = [app for app in wanted if app not in apps]
    results = []
    run_ids = itertools.count()
    try:
        for app in (app for app in wanted if app in apps):
            for state in STATES:
                shutil.rmtree(scratch / "profile", ignore_errors=True)
                result = measure(app, apps[app], state, args, next(run_ids), scratch)
                print(json.dumps(result), flush=True)
                results.append(result)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    stamp = time.strftime("%Y-%m-%d-%H%M")
    (out / f"a11y-cost-{stamp}.json").write_text(json.dumps(
        {"seconds_per_state": args.seconds, "missing": missing, "results": results}, indent=2) + "\n")
    (out / f"a11y-cost-{stamp}.md").write_text(summary(results, missing))
    print(summary(results, missing))


if __name__ == "__main__":
    sys.exit(main())
