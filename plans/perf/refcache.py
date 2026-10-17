"""Shared cache of reference-binary output for the three check scripts.

run_ref(binary, root, args, timeout) returns (exit code, stdout, stderr), read
from CACHE_DIR when a record exists for sha256(binary bytes) + root + args,
otherwise computed by running the binary and stored. Only the reference binary
goes through this; the new binary always runs.

The key does not include the files under root, so a cache entry goes stale if
the code a call searches changes (Mavexa, emc-gigs, fixtures/). Set
REFCACHE=0 to bypass the cache, or delete CACHE_DIR to rebuild it.
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


def _binary_digest(binary):
    if binary not in _digests:
        with open(binary, "rb") as f:
            _digests[binary] = hashlib.sha256(f.read()).hexdigest()
    return _digests[binary]


def _path(binary, root, args):
    key = json.dumps([_binary_digest(binary), root, list(args)])
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
