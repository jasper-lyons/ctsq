"""Time a fixed set of ctsq calls with the base binary and the new one.

Usage: python3 -I bench.py BASE_BIN NEW_BIN [RUNS]

Each call runs once to warm the file cache, then RUNS times (default 5).
The table prints the median wall time of each binary in seconds and the
ratio new / base, then the peak resident set size of one run of each binary
in MB, read from `/usr/bin/time -l` (macOS), and its ratio. `new --tests` shows the cost of hiding tests, since the
new binary hides tests by default and the base binary has no notion of them.
A call marked `-` in the base column exists only in the new binary.
"""

import os
import re
import statistics
import subprocess
import sys
import time

HOME = os.path.expanduser("~")
CTSQ = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
MAVEXA = os.path.join(HOME, "workspace/projects/mavexa/mavexa")
EMC = os.path.join(HOME, "workspace/projects/eastbournemusic/emc-gigs")

# Per root: the names each call uses, all of which exist in that root.
ROOTS = [
    {
        "label": "ctsq",
        "path": CTSQ,
        "dir": "src",
        "common": "name",
        "func": "run_rows",
        "nested": "compiled_by_lang",
        "files": ("Lang", "tree_sitter"),
        "error": "unknown kind 'X' for go, valid kinds: a, b",
        "text": "no definition to",
        "scopeof": "src/resolve/mod.rs:600",
    },
    {
        "label": "mavexa",
        "path": MAVEXA,
        "dir": ".",
        "common": "ctx",
        "func": "GetEvent",
        "nested": "activated",
        "files": ("ctx", "Store"),
        "error": "begin tx: connection refused",
        "text": "Welcome back",
        "scopeof": "backend/internal/store/brand.go:190",
    },
    {
        "label": "emc-gigs",
        "path": EMC,
        "dir": ".",
        "common": "gig",
        "func": "lineup",
        "nested": "secondIndex",
        "files": ("gig", "venue"),
        "error": "Gig not found",
        "text": "Sign up",
        "scopeof": "app/models/gig.rb:110",
    },
]


def calls(r):
    d, f, n = r["dir"], r["func"], r["nested"]
    a, b = r["files"]
    # (label, args, comparable with base)
    return [
        ("tree", ["tree", d], True),
        ("-q id#common", ["-q", "id#" + r["common"], d], True),
        ("-q *function --body", ["-q", "*function#" + f, "--body", d], True),
        ("def", ["def", f, d], True),
        ("def nested", ["def", n, d], True),
        ("refs", ["refs", f, d], True),
        ("explain", ["explain", f, d], True),
        ("callers", ["callers", f, d], True),
        ("files", ["files", "-q", "id#" + a, "-q", "id#" + b, d], True),
        ("find", ["find"], False),
        ("find route", ["find", "route"], False),
        ("find error", ["find", "error", r["error"]], False),
        ("find text", ["find", "text", r["text"]], False),
        ("scopeof", ["scopeof", r["scopeof"]], False),
        ("def --scope", ["def", f, "--scope", d], False),
        ("miss", ["-q", "id#zzqqx", d], False),
    ]


def peak_mb(binary, args, cwd):
    """Peak resident set size of one run in MB, from /usr/bin/time -l."""
    p = subprocess.run(
        ["/usr/bin/time", "-l", binary] + args,
        cwd=cwd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=120,
    )
    m = re.search(rb"(\d+)\s+maximum resident set size", p.stderr)
    return int(m.group(1)) / (1024 * 1024) if m else float("nan")


def run(binary, args, cwd, runs):
    cmd = [binary] + args
    subprocess.run(cmd, cwd=cwd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=120)
    times = []
    code = 0
    for _ in range(runs):
        t = time.perf_counter()
        p = subprocess.run(cmd, cwd=cwd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=120)
        times.append(time.perf_counter() - t)
        code = p.returncode
    return statistics.median(times), code


def main():
    base, new = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
    runs = int(sys.argv[3]) if len(sys.argv) > 3 else 5
    hdr = (
        f"{'command':<22} {'root':<9} {'base':>7} {'new':>7} {'ratio':>6} {'new+t':>7} {'ratio+t':>7} "
        f"{'baseMB':>7} {'newMB':>7} {'rssx':>5} exit"
    )
    print(hdr)
    print("-" * len(hdr))
    for r in ROOTS:
        for label, args, comparable in calls(r):
            nm, nc = run(new, args, r["path"], runs)
            nr = peak_mb(new, args, r["path"])
            if comparable:
                bm, bc = run(base, args, r["path"], runs)
                tm, _ = run(new, args + ["--tests"], r["path"], runs)
                br = peak_mb(base, args, r["path"])
                print(
                    f"{label:<22} {r['label']:<9} {bm:7.3f} {nm:7.3f} {nm / bm:6.2f} "
                    f"{tm:7.3f} {tm / bm:7.2f} {br:7.1f} {nr:7.1f} {nr / br:5.2f} {bc}/{nc}"
                )
            else:
                print(
                    f"{label:<22} {r['label']:<9} {'-':>7} {nm:7.3f} {'-':>6} {'-':>7} {'-':>7} "
                    f"{'-':>7} {nr:7.1f} {'-':>5} -/{nc}"
                )
            sys.stdout.flush()


if __name__ == "__main__":
    main()
