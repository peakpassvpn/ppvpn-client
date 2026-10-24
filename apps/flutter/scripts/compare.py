#!/usr/bin/env python3
"""Measures the Flutter prototype against the native app on this runner's
platform, the same way for both (#387, acceptance 2 and 4):

- cold start: launch to the first visible window, 5 runs (the first after a
  fresh install, so with a cold file cache only as far as the runner's is);
- idle: resident memory of the app and of its whole process tree, and CPU,
  30 s after launch with the window open;
- window closed: the same 15 s after the window was asked to close (both
  apps hide to the tray then); not measured on macOS, where closing another
  app's window needs an accessibility grant the runner does not have;
- sizes: download (installer / zip) and unpacked;
- the prototype only: its startup phases (bench.dart marks), tray CPU at
  rest and with the tray rebuilt every second, and with --curve-minutes its
  memory over that long with a traffic sample pushed every second.

Both run signed out, with their own empty data directories. Linux also
gets the prototype's memory by mapping (smaps), to say what the RSS is.

Usage: compare.py --flutter BUNDLE --flutter-zip-kib N --native EXE
       --native-installer FILE --native-root DIR --out DIR
Writes DIR/compare.json and DIR/compare.md (also appended to the job
summary).
"""

import argparse
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

import psutil

SYSTEM = platform.system()
RUNS = 5
SETTLE = 30
CLOSED_SETTLE = 15


def dir_kib(path):
    total = 0
    for root, _, files in os.walk(path):
        for name in files:
            full = os.path.join(root, name)
            if not os.path.islink(full):
                total += os.path.getsize(full)
    return total // 1024


_MAC_PROBE = None


def _windows_handles(pids):
    """Windows: visible top-level windows of the given processes."""
    import ctypes
    from ctypes import wintypes
    user32 = ctypes.windll.user32
    found = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def each(hwnd, _):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value in pids and user32.IsWindowVisible(hwnd) and user32.GetWindow(hwnd, 4) == 0:
            found.append(hwnd)
        return True

    user32.EnumWindows(each, 0)
    return found


def visible_window(pid):
    """True once the process (or a child) has a visible top-level window."""
    pids = {pid} | {c.pid for c in psutil.Process(pid).children(recursive=True)}
    if SYSTEM == "Linux":
        for p in pids:
            out = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(p)],
                                 capture_output=True, text=True)
            if out.stdout.strip():
                return True
        return False
    if SYSTEM == "Darwin":
        global _MAC_PROBE
        if _MAC_PROBE is None:
            _MAC_PROBE = os.path.join(tempfile.mkdtemp(), "windows")
            here = os.path.dirname(os.path.abspath(__file__))
            subprocess.run(["swiftc", "-O", "-o", _MAC_PROBE, os.path.join(here, "windows.swift")], check=True)
        out = subprocess.run([_MAC_PROBE] + [str(p) for p in pids], capture_output=True, text=True)
        return out.stdout.strip() == "yes"
    return bool(_windows_handles(pids))


def close_window(pid):
    """Asks the app's windows to close, as the window's close button would."""
    if SYSTEM == "Linux":
        from Xlib import X, display, protocol  # python-xlib
        d = display.Display()
        delete = d.intern_atom("WM_DELETE_WINDOW")
        protocols = d.intern_atom("WM_PROTOCOLS")
        out = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(pid)],
                             capture_output=True, text=True)
        for wid in out.stdout.split():
            window = d.create_resource_object("window", int(wid))
            event = protocol.event.ClientMessage(window=window, client_type=protocols,
                                                 data=(32, [delete, X.CurrentTime, 0, 0, 0]))
            window.send_event(event)
        d.flush()
        return True
    if SYSTEM == "Windows":
        import ctypes
        WM_CLOSE = 0x0010
        pids = {pid} | {c.pid for c in psutil.Process(pid).children(recursive=True)}
        for hwnd in _windows_handles(pids):
            ctypes.windll.user32.PostMessageW(hwnd, WM_CLOSE, 0, 0)
        return True
    return False


def memory(pid):
    proc = psutil.Process(pid)
    main = proc.memory_info().rss
    tree = main
    for child in proc.children(recursive=True):
        try:
            tree += child.memory_info().rss
        except psutil.Error:
            pass
    return main // 1024, tree // 1024


def cpu_percent(pid, seconds):
    proc = psutil.Process(pid)
    procs = [proc] + proc.children(recursive=True)
    for p in procs:
        p.cpu_percent(None)
    time.sleep(seconds)
    total = 0.0
    for p in procs:
        try:
            total += p.cpu_percent(None)
        except psutil.Error:
            pass
    return round(total, 1)


def stop(proc):
    try:
        parent = psutil.Process(proc.pid)
        for p in parent.children(recursive=True) + [parent]:
            try:
                p.kill()
            except psutil.Error:
                pass
    except psutil.Error:
        pass
    try:
        proc.wait(10)
    except subprocess.TimeoutExpired:
        pass


def launch(argv, home, extra=None):
    env = dict(os.environ)
    env.update(extra or {})
    # Each app's own empty data: HOME (macOS, Linux) and the Windows profile
    # folders it writes to.
    env.update(HOME=home, XDG_DATA_HOME=os.path.join(home, "share"),
               XDG_STATE_HOME=os.path.join(home, "state"),
               XDG_CONFIG_HOME=os.path.join(home, "config"))
    if SYSTEM == "Windows":
        env.update(LOCALAPPDATA=os.path.join(home, "local"), APPDATA=os.path.join(home, "roaming"))
        for key in ("local", "roaming"):
            os.makedirs(os.path.join(home, key), exist_ok=True)
    return subprocess.Popen(argv, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def native_env(argv):
    """The native Linux app as its desktop entry starts it."""
    return ["env", "MALLOC_ARENA_MAX=2"] + argv if SYSTEM == "Linux" else argv


def read_phases(path):
    """`phase ms` lines from the prototype's PPVPN_FLUTTER_TIMING file."""
    phases = {}
    try:
        with open(path, encoding="utf-8") as f:
            for line in f:
                name, _, ms = line.strip().partition(" ")
                if ms.lstrip("-").isdigit():
                    phases[name] = int(ms)
    except OSError:
        pass
    return phases


def measure(name, argv):
    home = tempfile.mkdtemp(prefix=f"cmp-{name}-")
    starts = []
    phases = []
    for _ in range(RUNS):
        timing = os.path.join(home, "timing.txt")
        if os.path.exists(timing):
            os.remove(timing)
        extra = {"PPVPN_FLUTTER_TIMING": timing, "PPVPN_FLUTTER_T0_MS": str(int(time.time() * 1000))}
        began = time.monotonic()
        proc = launch(argv, home, extra if name == "flutter" else None)
        shown = None
        while time.monotonic() - began < 60 and proc.poll() is None:
            if visible_window(proc.pid):
                shown = time.monotonic() - began
                break
            time.sleep(0.05)
        if name == "flutter":
            time.sleep(1)  # the first frame's mark can land after the window shows
            phases.append(read_phases(timing))
        stop(proc)
        starts.append(None if shown is None else round(shown * 1000))
        time.sleep(2)

    proc = launch(argv, home)
    time.sleep(SETTLE)
    result = {"cold_start_ms": starts}
    if phases:
        names = []
        for run in phases:
            names += [n for n in run if n not in names]
        result["phases_ms"] = {n: median([run.get(n) for run in phases[1:]]) for n in names}
    if proc.poll() is not None:
        result["error"] = f"exited with {proc.returncode} within {SETTLE}s"
        return result
    result["idle_rss_kib"], result["idle_tree_kib"] = memory(proc.pid)
    result["idle_cpu_percent"] = cpu_percent(proc.pid, 10)
    if close_window(proc.pid):
        time.sleep(CLOSED_SETTLE)
        if proc.poll() is None:
            result["closed_rss_kib"], result["closed_tree_kib"] = memory(proc.pid)
            result["closed_cpu_percent"] = cpu_percent(proc.pid, 10)
        else:
            result["closed_error"] = "the app exited when its window closed"
    if SYSTEM == "Linux" and name == "flutter":
        result["smaps_top_kib"] = smaps_top(proc.pid)
    stop(proc)
    if name == "flutter":
        result.update(tray_cpu(argv, home))
    shutil.rmtree(home, ignore_errors=True)
    return result


def tray_cpu(argv, home):
    """CPU of the prototype over 30 s at rest, then over 30 s with the tray
    rebuilt every second (PPVPN_FLUTTER_TRAY_BENCH)."""
    out = {}
    for key, extra in (("tray_rest_cpu_percent", None), ("tray_1hz_cpu_percent", {"PPVPN_FLUTTER_TRAY_BENCH": "1"})):
        proc = launch(argv, home, extra)
        time.sleep(15)
        if proc.poll() is None:
            out[key] = cpu_percent(proc.pid, 30)
        stop(proc)
    return out


def committed_kib(pid):
    """Memory the process holds: RSS, except on Windows, where the RSS is the
    working set, which the system trims at will (it fell from 148 to 17 MiB
    in a curve with the window open); there it is the private bytes."""
    info = psutil.Process(pid).memory_info()
    return (info.private if SYSTEM == "Windows" else info.rss) // 1024


def curve(argv, minutes, out_dir):
    """Memory (committed_kib) every 10 s for `minutes` with a traffic sample
    pushed every second (PPVPN_FLUTTER_SYNTHETIC_TRAFFIC) and the overview
    showing it."""
    home = tempfile.mkdtemp(prefix="cmp-curve-")
    proc = launch(argv, home, {"PPVPN_FLUTTER_SYNTHETIC_TRAFFIC": "1"})
    began = time.monotonic()
    samples = []
    while time.monotonic() - began < minutes * 60 and proc.poll() is None:
        time.sleep(10)
        try:
            samples.append((round(time.monotonic() - began), committed_kib(proc.pid)))
        except psutil.Error:
            break
    exited = proc.poll()
    stop(proc)
    shutil.rmtree(home, ignore_errors=True)
    with open(os.path.join(out_dir, "curve.csv"), "w", encoding="utf-8") as f:
        f.write("seconds,kib\n")
        f.writelines(f"{t},{kib}\n" for t, kib in samples)
    result = {"minutes": minutes, "samples": len(samples)}
    if exited is not None:
        result["error"] = f"exited with {exited} after {round(time.monotonic() - began)} s"
    # After the first 2 minutes (startup settling): first, last, peak, and
    # the least-squares slope in MiB per hour.
    steady = [(t, kib) for t, kib in samples if t >= 120]
    if len(steady) >= 2:
        n = len(steady)
        mean_t = sum(t for t, _ in steady) / n
        mean_k = sum(k for _, k in steady) / n
        var = sum((t - mean_t) ** 2 for t, _ in steady)
        slope = sum((t - mean_t) * (k - mean_k) for t, k in steady) / var if var else 0.0
        result.update(first_kib=steady[0][1], last_kib=steady[-1][1], peak_kib=max(k for _, k in steady),
                      slope_mib_per_hour=round(slope * 3600 / 1024, 2))
    return result


def smaps_top(pid, count=12):
    """Resident KiB by mapping (shared libraries by file, anonymous memory as
    [anon] / [heap]), largest first."""
    sizes = {}
    current = None
    with open(f"/proc/{pid}/smaps") as smaps:
        for line in smaps:
            parts = line.split()
            if not parts:
                continue
            if "-" in parts[0] and not parts[0].endswith(":"):
                current = parts[5] if len(parts) > 5 else "[anon]"
                current = os.path.basename(current) if current.startswith("/") else current
            elif parts[0] == "Rss:" and current is not None:
                sizes[current] = sizes.get(current, 0) + int(parts[1])
    return sorted(sizes.items(), key=lambda kv: -kv[1])[:count]


def median(values):
    values = [v for v in values if v is not None]
    return round(statistics.median(values)) if values else None


def fmt_mib(kib):
    return "n/a" if kib is None else f"{kib / 1024:.0f} MiB"


def main():
    # The report has non-ASCII text; Windows runners default stdout to cp1252.
    sys.stdout.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser()
    parser.add_argument("--flutter", required=True)
    parser.add_argument("--flutter-zip-kib", type=int, required=True)
    parser.add_argument("--native", required=True)
    parser.add_argument("--native-installer", required=True)
    parser.add_argument("--native-root", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--curve-minutes", type=int, default=0)
    args = parser.parse_args()

    flutter_bundle = args.flutter
    if SYSTEM == "Darwin":
        flutter_exe = os.path.join(flutter_bundle, "Contents", "MacOS", "PPVPN")
    elif SYSTEM == "Windows":
        flutter_exe = os.path.join(flutter_bundle, "ppvpn.exe")
    else:
        flutter_exe = os.path.join(flutter_bundle, "ppvpn")

    if SYSTEM == "Darwin":
        visible_window(os.getpid())  # builds the window probe before timing anything

    report = {
        "platform": f"{SYSTEM} {platform.release()} {platform.machine()}",
        "flutter": {"download_kib": args.flutter_zip_kib, "unpacked_kib": dir_kib(flutter_bundle)},
        "native": {"download_kib": os.path.getsize(args.native_installer) // 1024,
                   "unpacked_kib": dir_kib(args.native_root)},
    }
    report["flutter"].update(measure("flutter", [flutter_exe]))
    report["native"].update(measure("native", native_env([args.native])))

    os.makedirs(args.out, exist_ok=True)
    if args.curve_minutes:
        report["flutter"]["curve"] = curve([flutter_exe], args.curve_minutes, args.out)
    with open(os.path.join(args.out, "compare.json"), "w", encoding="utf-8") as f:
        json.dump(report, f, indent=2)

    f, n = report["flutter"], report["native"]
    rows = [
        ("Download", fmt_mib(f["download_kib"]) + " (zip, app only)", fmt_mib(n["download_kib"]) + " (installer)"),
        ("Unpacked", fmt_mib(f["unpacked_kib"]) + " (app only)", fmt_mib(n["unpacked_kib"]) + " (app, service, core)"),
        ("Cold start, first run", f"{f['cold_start_ms'][0]} ms", f"{n['cold_start_ms'][0]} ms"),
        ("Cold start, median of runs 2–5", f"{median(f['cold_start_ms'][1:])} ms", f"{median(n['cold_start_ms'][1:])} ms"),
        ("Idle RSS, window open (app / all processes)",
         f"{fmt_mib(f.get('idle_rss_kib'))} / {fmt_mib(f.get('idle_tree_kib'))}",
         f"{fmt_mib(n.get('idle_rss_kib'))} / {fmt_mib(n.get('idle_tree_kib'))}"),
        ("RSS, window closed (app / all processes)",
         f"{fmt_mib(f.get('closed_rss_kib'))} / {fmt_mib(f.get('closed_tree_kib'))}",
         f"{fmt_mib(n.get('closed_rss_kib'))} / {fmt_mib(n.get('closed_tree_kib'))}"),
        ("Idle CPU over 10 s, window open", f"{f.get('idle_cpu_percent', 'n/a')} %", f"{n.get('idle_cpu_percent', 'n/a')} %"),
        ("CPU over 10 s, window closed", f"{f.get('closed_cpu_percent', 'n/a')} %", f"{n.get('closed_cpu_percent', 'n/a')} %"),
    ]
    lines = [f"### Flutter prototype vs native 0.6.3, {report['platform']}", "",
             "| | Flutter prototype | Native 0.6.3 |", "| --- | --- | --- |"]
    lines += [f"| {a} | {b} | {c} |" for a, b, c in rows]
    lines += [f"| Tray CPU over 30 s, at rest / rebuilt every second | {f.get('tray_rest_cpu_percent', 'n/a')} % / "
              f"{f.get('tray_1hz_cpu_percent', 'n/a')} % | n/a |"]
    if f.get("phases_ms"):
        lines += ["", "Prototype startup, ms since launch (median of runs 2–5):", "",
                  "| Phase | ms |", "| --- | --- |"]
        lines += [f"| `{k}` | {v} |" for k, v in f["phases_ms"].items()]
    c = f.get("curve")
    if c:
        measure_name = "private bytes" if SYSTEM == "Windows" else "RSS"
        lines += ["", f"Prototype {measure_name} with a traffic sample every second, {c['minutes']} min "
                  f"({c['samples']} samples, `curve.csv`): after the first 2 min "
                  f"{fmt_mib(c.get('first_kib'))} → {fmt_mib(c.get('last_kib'))}, peak {fmt_mib(c.get('peak_kib'))}, "
                  f"slope {c.get('slope_mib_per_hour', 'n/a')} MiB/h."]
        if "error" in c:
            lines.append(f"Curve: {c['error']}")
    for key in ("error", "closed_error"):
        for side, data in (("Flutter", f), ("Native", n)):
            if key in data:
                lines.append(f"\n{side}: {data[key]}")
    if "smaps_top_kib" in f:
        lines += ["", "Prototype RSS by mapping (window closed), KiB:", "",
                  "| Mapping | RSS |", "| --- | --- |"]
        lines += [f"| `{name}` | {kib} |" for name, kib in f["smaps_top_kib"]]
    text = "\n".join(lines) + "\n"
    with open(os.path.join(args.out, "compare.md"), "w", encoding="utf-8") as out:
        out.write(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as out:
            out.write(text)
    print(text)


if __name__ == "__main__":
    sys.exit(main())
