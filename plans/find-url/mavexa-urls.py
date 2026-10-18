#!/usr/bin/env python3
"""Paste a real URL for every Mavexa route and check find route's first row is that route."""
import re, subprocess, sys
from concurrent.futures import ThreadPoolExecutor

ROOT = "/Users/jasperlyons/workspace/projects/mavexa/mavexa"
CTSQ = "/Users/jasperlyons/.local/bin/ctsq"

def run(args):
    p = subprocess.run([CTSQ, "find", "route", *args], cwd=ROOT, capture_output=True, text=True)
    return p.stdout, p.stderr, p.returncode

out, _, _ = run(["--max-matches", "1000"])
rows = [l.split()[0] for l in out.splitlines() if re.match(r"^\S+:\d+\s", l)]

def full_key(loc):
    path, line = loc.rsplit(":", 1)
    text = open(f"{ROOT}/{path}").read().splitlines()[int(line) - 1]
    m = re.search(r"""(['"`])(.*?)\1""", text)
    key = m.group(2)
    if path.endswith("routesConfig.ts") and 60 < int(line) < 121:
        key = "/w/:workspaceID" + key
    return key

n = [0]
def sample(key):
    def val(_m):
        n[0] += 1
        return f"v{n[0]}"
    url = re.sub(r"\{\$\}", "", key)
    url = re.sub(r"\{\w+\.\.\.\}", "a/b", url)
    url = re.sub(r"\{\w+\}", val, url)
    url = re.sub(r":\w+", val, url)
    url = re.sub(r"\*\w*", "a/b", url)
    return url

def check(loc):
    key = full_key(loc)
    if key.startswith("*"):
        return None
    method = ""
    if re.match(r"^[A-Z]+ ", key):
        method, key = key.split(" ", 1)
        method += " "
    url = sample(key)
    pasted = f"{method}https://app.mavexa.com{url}?ref=x"
    so, se, rc = run([pasted])
    first = next((l.split()[0] for l in so.splitlines() if re.match(r"^\S+:\d+\s", l)), None)
    ok = first == loc
    return (ok, loc, key, pasted, first, so.splitlines()[0] if so else se.strip()[:120])

with ThreadPoolExecutor(8) as ex:
    results = [r for r in ex.map(check, rows) if r]
bad = [r for r in results if not r[0]]
print(f"{len(results)} routes pasted, {len(results) - len(bad)} first row correct, {len(bad)} wrong")
for r in bad:
    print("WRONG", r[1], r[2], "| pasted", r[3], "| first", r[4], "|", r[5])
