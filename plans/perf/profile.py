"""Profile each call in a calls file; print a markdown table.

usage: profile.py BIN CALLS_FILE OUT_DIR [name-filter]
calls file: one per line, ROOT<TAB>ARGS (ARGS shell-split, run with cwd=ROOT); '#' lines skipped.
BIN should be a release build with debug symbols (see profile.sh).
"""
import ctypes, os, sys, subprocess, time, signal, shlex, re, statistics, math

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bucket as B

libc = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
STATS = re.compile(r'parsed (\d+) files, (\d+) cache hits, (\d+) text reads')
NFILES = re.compile(r'\((\d+) files?\)')


def run_plain(binp, root, argv, env=None):
    t = time.time()
    p = subprocess.run([binp] + argv, cwd=root, stdout=subprocess.DEVNULL,
                       stderr=subprocess.PIPE, env=env)
    return time.time() - t, p.stderr.decode(errors="replace")


def sample_once(binp, root, argv, out):
    os.chdir(root)
    attr = ctypes.create_string_buffer(64)
    libc.posix_spawnattr_init(attr)
    libc.posix_spawnattr_setflags(attr, ctypes.c_short(0x0080))  # POSIX_SPAWN_START_SUSPENDED
    pid = ctypes.c_int()
    av = (ctypes.c_char_p * (len(argv) + 2))(binp.encode(), *[a.encode() for a in argv], None)
    fa = ctypes.create_string_buffer(64)
    libc.posix_spawn_file_actions_init(fa)
    libc.posix_spawn_file_actions_addopen(fa, 1, b"/dev/null", os.O_WRONLY, 0)
    libc.posix_spawn_file_actions_addopen(fa, 2, b"/dev/null", os.O_WRONLY, 0)
    env = [f"{k}={v}".encode() for k, v in os.environ.items()]
    envp = (ctypes.c_char_p * (len(env) + 1))(*env, None)
    r = libc.posix_spawn(ctypes.byref(pid), binp.encode(), fa, attr, av, envp)
    assert r == 0, r
    sp = subprocess.Popen(["/usr/bin/sample", str(pid.value), "60", "1", "-file", out],
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.4)
    os.kill(pid.value, signal.SIGCONT)
    os.waitpid(pid.value, 0)
    sp.wait()


def scope_of(root, argv):
    for a in reversed(argv):
        if not a.startswith("-") and os.path.exists(os.path.join(root, a.split(":")[0])):
            return a.split(":")[0]
    return "."


def main():
    binp, calls, out = sys.argv[1:4]
    filt = sys.argv[4] if len(sys.argv) > 4 else None
    os.makedirs(out + "/samples", exist_ok=True)
    rows = []
    for n, ln in enumerate(open(calls)):
        ln = ln.rstrip("\n")
        if not ln.strip() or ln.startswith("#"):
            continue
        root, args = ln.split("\t", 1)
        if filt and filt not in args:
            continue
        argv = shlex.split(args)
        walls = [run_plain(binp, root, argv)[0] for _ in range(3)]
        wall = statistics.median(walls)
        env = dict(os.environ, CTSQ_PARSE_STATS="1")
        _, err = run_plain(binp, root, argv, env)
        st = STATS.findall(err)
        parses = sum(int(s[0]) for s in st)
        hits = sum(int(s[1]) for s in st)
        reads = sum(int(s[2]) for s in st)
        sc = scope_of(root, argv)
        _, merr = run_plain(binp, root, ["-q", "id#zzqqx", sc])
        nf = NFILES.findall(merr)
        files_nt = int(nf[-1]) if nf else -1          # files in scope, tests hidden
        _, merr = run_plain(binp, root, ["-q", "id#zzqqx", sc, "--tests"])
        nf = NFILES.findall(merr)
        files = int(nf[-1]) if nf else -1             # files in scope, tests included
        reps = max(1, min(10, math.ceil(1.0 / max(wall, 0.01))))
        paths = []
        for i in range(reps):
            p = f"{out}/samples/c{n}-{i}.txt"
            sample_once(binp, root, argv, p)
            paths.append(p)
        a = B.analyse(paths)
        rows.append(dict(n=n, root=os.path.basename(root.rstrip("/")), args=args, wall=wall,
                         samples=a["samples"], parses=parses, hits=hits, reads=reads,
                         files=files, files_nt=files_nt, a=a))
    print("| # | root | call | wall s | samples | parses/hits/reads | files in scope | parse incl % | top buckets (top ctsq fn) |")
    print("|--|--|--|--|--|--|--|--|--|")
    for r in rows:
        tb = "; ".join(f"{b} {p}% ({fn[0] if fn else '-'})" for b, p, fn in r["a"]["buckets"][:3] if p >= 3)
        flag = " **>2s**" if r["wall"] > 2 else ""
        rep = " **P>F**" if r["files"] >= 0 and r["parses"] > r["files"] else ""
        print(f"| {r['n']} | {r['root']} | `{r['args']}` | {r['wall']:.2f}{flag} | {r['a']['samples']} | "
              f"{r['parses']}/{r['hits']}/{r['reads']}{rep} | {r['files']} ({r['files_nt']} non-test) | {r['a']['parse_incl_pct']} | {tb} |")


main()
