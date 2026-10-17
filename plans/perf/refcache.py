"""Shared cache of reference-binary output for the three check scripts.

run_ref(binary, root, args, timeout) returns (exit code, stdout, stderr), read
from CACHE_DIR when a record exists for sha256(binary bytes) + root + args,
otherwise computed by running the binary and stored. Only the reference binary
goes through this; the new binary always runs.

The key also holds a fingerprint of the root (path, size and mtime of every
file under it, .git, target and node_modules left out), so editing the files a
call searches, such as this repo's src/, makes a fresh reference run. A root
outside the repo is read-only and keeps its entries. Set REFCACHE=0 to bypass
the cache, or delete CACHE_DIR to rebuild it.
"""
import hashlib
import json
import os
import subprocess
import tempfile

CACHE_DIR = os.environ.get(
    "REFCACHE_DIR", os.path.expanduser("~/.claude/jobs/30494ad5/tmp/ref-cache")
)
_digests = {}
_prints = {}
_SKIP = {".git", "target", "node_modules", ".claude"}


def _fingerprint(root):
    if root not in _prints:
        h = hashlib.sha256()
        for d, dirs, files in os.walk(root):
            dirs[:] = sorted(x for x in dirs if x not in _SKIP)
            for f in sorted(files):
                p = os.path.join(d, f)
                try:
                    st = os.stat(p)
                except OSError:
                    continue
                h.update(f"{p}\0{st.st_size}\0{st.st_mtime_ns}\n".encode())
        _prints[root] = h.hexdigest()
    return _prints[root]


def _binary_digest(binary):
    if binary not in _digests:
        with open(binary, "rb") as f:
            _digests[binary] = hashlib.sha256(f.read()).hexdigest()
    return _digests[binary]


def _path(binary, root, args):
    key = json.dumps([_binary_digest(binary), root, _fingerprint(root), list(args)])
    return os.path.join(CACHE_DIR, hashlib.sha256(key.encode()).hexdigest() + ".json")


def run_ref(binary, root, args, timeout=900, env=None):
    use = os.environ.get("REFCACHE", "1") != "0"
    path = _path(binary, root, args) if use else None
    if path and os.path.exists(path):
        try:
            with open(path) as f:
                d = json.load(f)
            return d["code"], bytes.fromhex(d["stdout"]), bytes.fromhex(d["stderr"])
        except (OSError, ValueError, KeyError):
            pass
    p = subprocess.run([binary] + list(args), cwd=root, capture_output=True, timeout=timeout, env=env)
    if path:
        os.makedirs(CACHE_DIR, exist_ok=True)
        fd, tmp = tempfile.mkstemp(dir=CACHE_DIR)
        with os.fdopen(fd, "w") as f:
            json.dump({"code": p.returncode, "stdout": p.stdout.hex(), "stderr": p.stderr.hex()}, f)
        os.replace(tmp, path)
    return p.returncode, p.stdout, p.stderr
