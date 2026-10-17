"""Differential check for the scoped call graph (plans/perf/scoped-graph.md).

Runs callers, callees and explain at depth 1, 3 and 6 for names from Mavexa
backend (Go), emc-gigs app (Ruby, the fallback resolver) and a ctsq checkout
(Rust), with the reference binary and the new one, and compares stdout,
stderr and the exit code. Prints one DIFF line per call that differs and a
total. Usage: differential.py REF NEW [JOBS]
"""
import concurrent.futures
import subprocess
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import refcache  # noqa: E402

REF, NEW = sys.argv[1], sys.argv[2]
JOBS = int(sys.argv[3]) if len(sys.argv) > 3 else 8

M = ("/Users/jasperlyons/workspace/projects/mavexa/mavexa", "backend")
E = ("/Users/jasperlyons/workspace/projects/eastbournemusic/emc-gigs", "app")
C = ("/Users/jasperlyons/.claude/jobs/30494ad5/tmp/pc-ref", "src")

NAMES = {
    M: [
        # handlers and helpers
        "writeError", "register", "login", "decodeBody",
        # interface methods, dispatched through interfaces
        "Parse", "Name", "Chat", "ChatWithUsage", "Send", "Embed", "Model", "Merge", "GetEvent", "Capture", "Operators",
        # common method names
        "Close", "String", "Get", "Error",
        # recursive
        "walk", "collect", "visit",
        # defined in tests
        "TestBrandedPDFEvidence", "fakeSalesforce", "serve",
    ],
    E: [
        "authenticate", "show", "update", "create", "index", "destroy", "new", "edit",
        "display_resource", "set_gig", "contact_params", "render_invite_error",
        "ticket_added?", "sync_lineup!", "signup_open?", "signup_full?",
        "summary_hash", "strip_body", "state", "when_at", "social_post", "default_sorting_attribute",
    ],
    C: [
        "run_rows", "report", "build", "new", "parse", "get", "run", "name", "text", "holds",
        "compile", "def_rows", "keep", "default", "from_query", "source",
        # recursive
        "resolve_callee", "walk_scope", "collect_use", "type_name", "walk_children",
        # defined in tests
        "tmp", "jobs_for", "go_pair", "fixtures",
    ],
}


def calls():
    for (root, where), names in NAMES.items():
        for name in names:
            for depth in (1, 3, 6):
                yield root, ["callers", name, where, "--max-depth", str(depth)]
                yield root, ["callees", name, where, "--max-depth", str(depth)]
                yield root, ["explain", name, where, "--depth", str(depth)]


def run(binary, root, args):
    p = subprocess.run([binary] + args, cwd=root, capture_output=True, timeout=600)
    return p.returncode, p.stdout, p.stderr


def check(call):
    root, args = call
    a = refcache.run_ref(REF, root, args, timeout=600)
    b = run(NEW, root, args)
    return call, a == b, a, b


def main():
    todo = list(calls())
    diffs = 0
    with concurrent.futures.ThreadPoolExecutor(JOBS) as pool:
        for (root, args), same, a, b in pool.map(check, todo):
            if not same:
                diffs += 1
                what = [k for k, x, y in zip(("code", "stdout", "stderr"), a, b) if x != y]
                print(f"DIFF {root} {' '.join(args)}: {', '.join(what)}", flush=True)
    print(f"differential: {len(todo)} calls, {diffs} differences")
    sys.exit(1 if diffs else 0)


main()
