"""Differential check for `ctsq path` (the path-flow package).

Runs call routes (`-->`, `->`, `<--`) and data-flow routes (`==>`, `=>`,
`<==`, `<=`) with the reference binary and the new one, and compares stdout,
stderr and the exit code. The inputs are routes on Mavexa backend, and every
query string the tests/path*.rs suites hold, run against each directory under
fixtures/flow plus fixtures/path. Prints one DIFF line per call that differs
and a total. Usage: path-differential.py REF NEW REPO [JOBS]
"""
import concurrent.futures
import glob
import os
import re
import subprocess
import sys

REF, NEW, REPO = sys.argv[1], sys.argv[2], sys.argv[3]
JOBS = int(sys.argv[4]) if len(sys.argv) > 4 else 8

MAVEXA = "/Users/jasperlyons/workspace/projects/mavexa/mavexa"
MAVEXA_ROUTES = [
    ["function#login --> function#writeError"],
    ["function#register --> function#writeError"],
    ["function#register --> function#decodeBody"],
    ["function#login --> function#writeJSON"],
    ["function#writeError <-- function#login"],
    ["function#register -> function#writeError"],
    ["function#login --> function#writeError:not(function#decodeBody)"],
    ["function#login --> function#writeError", "--body"],
    ["function#register --> function#writeError", "--all"],
    ["field#signupRequest.Email ==>"],
    ["field#loginRequest.Email ==>"],
    ["field#signupRequest.Email ==> function#writeError"],
    ["function#writeError param#msg ==>"],
    ["function#writeError param#msg =>"],
    ["function#decodeBody param#v ==>"],
    ["function#writeJSON param#v ==> function#/Encode/"],
    ["function#login param#r ==> function#decodeBody"],
    ["function#writeError param#msg <=="],
    ["function#login variable#req <=="],
    ["field#signupRequest.Email ==>", "--max", "3"],
]


def test_queries():
    """Every route string in tests/path*.rs, placeholders left out."""
    out = []
    for f in sorted(glob.glob(os.path.join(REPO, "tests", "path*.rs"))):
        for q in re.findall(r'"((?:[^"\\]|\\.)*)"', open(f).read()):
            if "#" not in q or "\n" in q or "{" in q or "\\" in q or len(q) > 120:
                continue
            if not re.search(r"-->|->|<--|<-|==>|=>|<==|<=", q):
                continue
            if q not in out:
                out.append(q)
    return out


def calls():
    for args in MAVEXA_ROUTES:
        yield MAVEXA, ["path", args[0], "backend"] + args[1:]
    dirs = sorted(d for d in glob.glob(os.path.join(REPO, "fixtures", "flow", "*")) if os.path.isdir(d))
    dirs.append(os.path.join(REPO, "fixtures", "path"))
    for q in test_queries():
        for d in dirs:
            yield d, ["path", q, "."]


def run(binary, root, args):
    p = subprocess.run([binary] + args, cwd=root, capture_output=True, timeout=900)
    return p.returncode, p.stdout, p.stderr


def check(item):
    root, args = item
    a = run(REF, root, args)
    b = run(NEW, root, args)
    return root, args, a == b, a, b


def main():
    items = list(calls())
    diffs = 0
    routes = 0
    with concurrent.futures.ThreadPoolExecutor(JOBS) as ex:
        for root, args, same, a, b in ex.map(check, items):
            if a[0] == 0:
                routes += 1
            if not same:
                diffs += 1
                where = os.path.relpath(root, REPO) if root.startswith(REPO) else root
                print("DIFF", where, " ".join(args), "exit", a[0], b[0])
    print(f"{len(items)} calls ({routes} found a route), {diffs} differences")
    sys.exit(1 if diffs else 0)


main()
