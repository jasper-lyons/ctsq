"""Bucket /usr/bin/sample call-graph output into cost buckets.

analyse(paths) -> dict(samples, buckets=[(name, pct, [top fns])], parse_incl_pct, own=[...])
Only samples under ctsq::main are counted. Each sample goes to the bucket of its
deepest matching frame; "top fns" are the nearest ctsq:: frames above the leaf.
"""
import re, collections

LINE = re.compile(r'^([ +!:|]*)(\d+) (.*)$')
HASH = re.compile(r'::h[0-9a-f]{16}')


def clean(name):
    name = re.sub(r'\s+\(in [^)]*\).*$', '', name)
    name = name.replace('_$LT$', '<').replace('$GT$', '>').replace('$u20$', ' ')
    return HASH.sub('', name)


def parse(path):
    nodes = []
    started = False
    for ln in open(path, errors="replace"):
        if ln.startswith("Call graph:"):
            started = True
            continue
        if not started:
            continue
        if ln.startswith("Total number in stack"):
            break
        m = LINE.match(ln.rstrip("\n"))
        if m:
            nodes.append((len(m.group(1)), int(m.group(2)), m.group(3)))
    stack, parent = [], []
    for i, (col, n, name) in enumerate(nodes):
        while stack and stack[-1][0] >= col:
            stack.pop()
        parent.append(stack[-1][1] if stack else -1)
        stack.append((col, i))
    childsum = collections.Counter()
    for i, p in enumerate(parent):
        if p >= 0:
            childsum[p] += nodes[i][1]
    for i, (col, n, name) in enumerate(nodes):
        selfn = n - childsum[i]
        if selfn <= 0:
            continue
        fr, j = [], i
        while j >= 0:
            fr.append(clean(nodes[j][2]))
            j = parent[j]
        yield selfn, fr  # fr[0] = leaf


RULES = [
    ("regex", re.compile(r'regex')),
    ("fs_read", re.compile(r'^(read|open|__open|fstat|stat|lstat|getdirentries|__getdirentries|close|__read_nocancel)\b|std::fs|std::io::Read|read_to_string|ignore::|walkdir|openat|fstatat|opendir|readdir|__fcntl|mmap|munmap')),
    ("output", re.compile(r'^(write|__write_nocancel)\b|stdio|LineWriter|BufWriter|Stdout|println|std::io::Write|fmt::write|core::fmt')),
    ("resolve", re.compile(r'ctsq::(resolve|calltree|explain|imports|cooccur)|graph|CallGraph|DefIndex')),
]
ALLOC = re.compile(r'malloc|free$|memmove|memcpy|realloc|_xzm|nano_|_platform_mem|__rust_(de)?alloc|__rust_realloc')


def bucket(fr):
    has_parse = any('ts_parser_parse' in f or 'Parser::parse' in f for f in fr)
    has_qnew = any('ts_query_new' in f or 'Query::new' in f for f in fr)
    has_qc = any('ts_query_cursor' in f or 'QueryCursor' in f for f in fr)
    for f in fr:
        if f.startswith(('ts_', '_ts_', 'tree_sitter')) and not re.search(r'ts_(malloc|free|realloc|calloc)', f):
            if has_parse:
                return "ts_parse"
            if has_qnew:
                return "query_compile"
            if has_qc:
                return "query_match"
            return "ts_other"
        if re.search(r'^_?ts_(malloc|free|realloc|calloc)', f) or ALLOC.search(f):
            return "alloc"
        for b, rx in RULES:
            if rx.search(f):
                return b
    return "other"


def ctsq_fn(fr):
    for f in fr:
        if f.startswith("ctsq::") and not f.startswith("ctsq::main"):
            return f
    for f in fr:
        if f.startswith("ctsq::"):
            return f
    return None


def analyse(paths):
    tot = 0
    bk = collections.Counter()
    topfn = collections.defaultdict(collections.Counter)
    own = collections.Counter()
    parse_incl = 0
    for p in paths:
        for n, fr in parse(p):
            if not any(f.startswith("ctsq::main") for f in fr):
                continue
            tot += n
            b = bucket(fr)
            bk[b] += n
            cf = ctsq_fn(fr)
            if cf:
                topfn[b][cf] += n
                own[cf] += n
            if any('ts_parser_parse' in f or 'Parser::parse' in f for f in fr):
                parse_incl += n
    if not tot:
        return {"samples": 0, "buckets": [], "parse_incl_pct": 0, "own": []}
    short = lambda f: f.replace('ctsq::', '')
    return {
        "samples": tot,
        "buckets": [(b, round(100 * n / tot, 1), [short(f) for f, _ in topfn[b].most_common(1)])
                    for b, n in bk.most_common()],
        "parse_incl_pct": round(100 * parse_incl / tot, 1),
        "own": [f"{short(f)} {round(100*c/tot,1)}%" for f, c in own.most_common(4)],
    }
