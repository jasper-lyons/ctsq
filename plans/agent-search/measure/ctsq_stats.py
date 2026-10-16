import json, re, collections, shlex, sys

SUBS = {'explain', 'tree', 'def', 'callers', 'callees', 'callgraph', 'files', 'example', 'refs', 'imports', 'outline', 'path'}
evs = collections.defaultdict(list)
for l in open('events.jsonl'):
    e = json.loads(l)
    evs[e['file']].append(e)

def ctsq_parts(cmd):
    m = re.search(r'ctsq\s+(.*)', cmd)
    rest = m.group(1) if m else ''
    first = rest.split()[0] if rest.split() else ''
    sub = first if first in SUBS else 'query'
    flags = sorted(set(re.findall(r'\s(--?[a-zA-Z][\w-]*)', ' ' + rest.split('|')[0])))
    return sub, flags, rest

sub_c = collections.Counter(); flag_c = collections.Counter(); size_by_sub = collections.defaultdict(list)
err_by_sub = collections.Counter(); empty_by_sub = collections.Counter()
next_c = collections.Counter(); next_same_file = collections.Counter()
pipe_c = collections.Counter(); per_cmd_count = collections.Counter(); retry = 0
selector_c = collections.Counter(); err_msgs = collections.Counter()
ctsq_sessions = set(); all_sessions = set()
for f, lst in evs.items():
    lst.sort(key=lambda e: e['seq'])
    all_sessions.add(f)
    for i, e in enumerate(lst):
        if e['kind'] != 'ctsq':
            continue
        ctsq_sessions.add(f)
        sub, flags, rest = ctsq_parts(e['cmd'])
        sub_c[sub] += 1
        for fl in flags: flag_c[fl] += 1
        for s in re.findall(r"[*&]?(\w+)#", rest): selector_c[s] += 1
        if '|' in rest:
            pipe_c[rest.split('|', 1)[1].split()[0] if rest.split('|', 1)[1].split() else ''] += 1
        n = rest.count('ctsq')
        rc = e.get('result_chars', 0)
        size_by_sub[sub].append(rc)
        head = (e.get('result_head') or '').lower()
        is_err = e.get('error') or head.startswith(('error', 'usage', 'ctsq:')) or 'unknown' in head[:80]
        if is_err:
            err_by_sub[sub] += 1
            err_msgs[re.sub(r'\d+', 'N', head[:90]).strip()] += 1
        elif rc < 5 or 'no match' in head[:60] or head.strip() in ('', '(no output)'):
            empty_by_sub[sub] += 1
        if i + 1 < len(lst):
            nx = lst[i + 1]
            k = nx['kind'] or nx['tool']
            if k == 'ctsq':
                k = 'ctsq'
                if is_err: retry += 1
            next_c[k] += 1
            if nx['kind'] in ('read_src', 'read') :
                paths = set(re.findall(r'[\w./-]+\.\w+', rest))
                if any(p in (nx['cmd'] or '') for p in paths):
                    next_same_file[nx['kind']] += 1

def pct(a, b): return f"{100*a/b:.0f}%" if b else '-'
tot = sum(sub_c.values())
print('sessions with ctsq', len(ctsq_sessions), 'of', len(all_sessions))
print('\nsubcommand  calls  share  errors  empty  median_chars  p90_chars')
for s, n in sub_c.most_common():
    sz = sorted(size_by_sub[s]); med = sz[len(sz)//2]; p90 = sz[int(len(sz)*.9)]
    print(f"{s:11} {n:6} {pct(n,tot):>5} {pct(err_by_sub[s],n):>6} {pct(empty_by_sub[s],n):>6} {med:>12} {p90:>10}")
print('\nflags', flag_c.most_common(15))
print('selectors', selector_c.most_common(15))
print('piped into', pipe_c.most_common(8))
print('\nnext call after ctsq', next_c.most_common(14))
print('next is a read of a file named in the ctsq call', dict(next_same_file))
print('ctsq retry after error', retry)
print('\ntop error heads'); [print(' ', n, m) for m, n in err_msgs.most_common(12)]
