"""Run a fixed call set with a reference binary and the new one, diff the results.

Usage: python3 -I same-output.py REF_BIN NEW_BIN

The call set is every call in plans/agent-search/measure/bench.py (with and
without --tests) over this repo, Mavexa and emc-gigs, plus every worked
example in plans/agent-search/docs/SKILL.md and every EXAMPLES line in
`ctsq --help`, run in this repo. For each call stdout, stderr and the exit
code must match. Prints one block per difference and a summary line, and
exits 1 when anything differs.
"""

import os
import re
import shlex
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.dont_write_bytecode = True  # no __pycache__ beside bench.py
sys.path.insert(0, os.path.join(HERE, "..", "agent-search", "measure"))
import bench  # noqa: E402

REPO = bench.CTSQ


def example_calls():
    text = open(os.path.join(REPO, "plans/agent-search/docs/SKILL.md")).read()
    start = text.index("## Worked examples")
    end = text.index("\n## ", start + 5)
    out = []
    for block in re.findall(r"```\n(.*?)```", text[start:end], re.S):
        first = block.split("\n")[0]
        if first.startswith("ctsq "):
            out.append(shlex.split(first)[1:])
    return out


def help_calls(binary):
    h = subprocess.run([binary, "--help"], capture_output=True, text=True).stdout
    sec = h[h.index("EXAMPLES"):]
    sec = sec[: sec.index("\n\n")]
    return [shlex.split(l.strip())[1:] for l in sec.split("\n") if l.startswith("  ctsq ")]


def run(binary, args, cwd):
    env = dict(os.environ)
    env.pop("CTSQ_PARSE_STATS", None)
    p = subprocess.run([binary] + args, cwd=cwd, capture_output=True, timeout=300, env=env)
    return p.returncode, p.stdout, p.stderr


def main():
    ref, new = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
    jobs = []
    for r in bench.ROOTS:
        if not os.path.isdir(r["path"]):
            print(f"missing root {r['path']}")
            sys.exit(2)
        for _label, args, _cmp in bench.calls(r):
            jobs.append((r["path"], args))
            jobs.append((r["path"], args + ["--tests"]))
    for args in example_calls() + help_calls(ref) + [["--help"]]:
        jobs.append((REPO, args))
    diffs = 0
    for cwd, args in jobs:
        a, b = run(ref, args, cwd), run(new, args, cwd)
        if a != b:
            diffs += 1
            print(f"DIFF in {cwd}: ctsq {shlex.join(args)}")
            for name, x, y in (("exit", a[0], b[0]), ("stdout", a[1], b[1]), ("stderr", a[2], b[2])):
                if x != y:
                    xs = x if isinstance(x, int) else x.decode(errors="replace")[:600]
                    ys = y if isinstance(y, int) else y.decode(errors="replace")[:600]
                    print(f"  {name} ref: {xs!r}\n  {name} new: {ys!r}")
    print(f"same-output: {len(jobs)} calls, {diffs} differences")
    sys.exit(1 if diffs else 0)


if __name__ == "__main__":
    main()
