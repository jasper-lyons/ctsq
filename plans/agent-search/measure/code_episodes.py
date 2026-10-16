import json, re, collections, statistics, random
random.seed(21)

def group(t):
    if t.startswith('ctsq:'):
        b = t[5:].split('|')[0]
        if b in ('explain',): g = 'ctsq.explain'
        elif b == 'tree': g = 'ctsq.tree'
        elif b in ('def', 'callers', 'callees', 'callgraph', 'files', 'refs', 'path', 'example'): g = 'ctsq.' + b
        elif '+body' in b: g = 'ctsq.body'
        elif b.startswith(('literal', 'string')): g = 'ctsq.literal'
        elif b.startswith('id'): g = 'ctsq.id+C' if '+C' in b else 'ctsq.id'
        else: g = 'ctsq.other'
        return g + ('|grep' if '|grep' in t else '')
    if t.startswith('grep:'): return 'grep.' + ('name' if t[5:] in ('name', 'names', 'config_key') else 'text/pattern')
    return t.replace(':', '.')

evs = collections.defaultdict(list)
for l in open('code_events.jsonl'):
    e = json.loads(l); evs[e['file']].append(e)

eps = []
for f, lst in evs.items():
    lst.sort(key=lambda e: e['seq'])
    run = []
    for e in lst + [{'ctype': 'ACTION:end', 'tool': 'end'}]:
        if e['ctype'] == 'NONCODE':
            continue
        if e['ctype'].startswith('ACTION'):
            if run: eps.append({'file': f, 'sub': run[0]['sub'], 'calls': run, 'end': e['ctype'][7:]})
            run = []
        else:
            run.append(e)

def pct(a, b): return f"{100*a/b:.0f}%"
lens = [len(x['calls']) for x in eps]
chars = [sum(c.get('result_chars') or 0 for c in x['calls']) for x in eps]
q = lambda v, p: sorted(v)[int(len(v) * p)]
print(f"episodes {len(eps)} (main {sum(1 for x in eps if not x['sub'])}, sub {sum(1 for x in eps if x['sub'])})")
print(f"calls per episode: median {q(lens,.5)} p75 {q(lens,.75)} p90 {q(lens,.9)} max {max(lens)}  | single-call episodes {pct(sum(1 for v in lens if v==1), len(lens))}")
print(f"chars per episode: median {q(chars,.5)} p75 {q(chars,.75)} p90 {q(chars,.9)}")
big = [x for x in eps if len(x['calls']) >= 5]
print(f"episodes with 5+ calls: {len(big)} holding {pct(sum(len(x['calls']) for x in big), sum(lens))} of calls")

first = collections.Counter(group(x['calls'][0]['ctype']) for x in eps if len(x['calls']) >= 2)
print('\nfirst move (episodes of 2+ calls):', first.most_common(12))
last_before_edit = collections.Counter(group(x['calls'][-1]['ctype']) for x in eps if x['end'] in ('Edit', 'Write'))
print('last move before an Edit/Write:', last_before_edit.most_common(10))
print('what ends an episode:', collections.Counter(x['end'] for x in eps).most_common(10))

bi = collections.Counter(); tri = collections.Counter(); from_c = collections.Counter()
for x in eps:
    g = [group(c['ctype']) for c in x['calls']]
    for a, b in zip(g, g[1:]): bi[(a, b)] += 1; from_c[a] += 1
    for a, b, c in zip(g, g[1:], g[2:]): tri[(a, b, c)] += 1
print('\ntop transitions (share of moves out of the first type)')
for (a, b), n in bi.most_common(30): print(f"  {a:22} -> {b:22} {n:5} {pct(n, from_c[a]):>4}")
print('\ntop 3-call sequences')
for s, n in tri.most_common(15): print(f"  {' -> '.join(s):70} {n}")

# re-reads and ctsq-then-read of same file
reread = 0; reread_eps = 0; ctsq_then_read = 0; read_then_ctsq_same = 0; reads = 0
for x in eps:
    seen = collections.Counter(); hit = False
    for i, c in enumerate(x['calls']):
        if c['ctype'].startswith('read') and c['ctype'] != 'read:size':
            reads += 1
            for p in c['files']:
                if seen[p]: reread += 1; hit = True
                seen[p] += 1
            if any(p in prev['files'] for prev in x['calls'][:i] if prev['ctype'].startswith('ctsq') for p in c['files']):
                ctsq_then_read += 1
        elif c['ctype'].startswith('ctsq') and any(p in seen for p in c['files']):
            read_then_ctsq_same += 1
    reread_eps += hit
print(f"\nreads {reads}: re-read of a file already read in the episode {reread} ({pct(reread, reads)}), in {reread_eps} episodes")
print(f"reads of a file an earlier ctsq call in the episode had named: {ctsq_then_read} ({pct(ctsq_then_read, reads)})")
print(f"ctsq calls on a file already read in the episode: {read_then_ctsq_same}")

whole = sorted(c.get('result_chars') or 0 for x in eps for c in x['calls'] if c['ctype'] == 'read:whole')
print(f"whole-file read chars: median {q(whole,.5)} p75 {q(whole,.75)} p90 {q(whole,.9)}  share >20k chars {pct(sum(1 for v in whole if v>20000), len(whole))}, and they make up {pct(sum(v for v in whole if v>20000), sum(whole))} of whole-read chars")
allc = sum(chars)
by_g = collections.Counter()
for x in eps:
    for c in x['calls']: by_g[group(c['ctype'])] += c.get('result_chars') or 0
print('share of all code-search output chars by type:', [(g, pct(v, allc)) for g, v in by_g.most_common(8)])

ext = collections.Counter()
for x in eps:
    for c in x['calls']:
        for p in c['files']: ext[p.rsplit('.', 1)[-1]] += 1
print('\nfile types named in calls:', ext.most_common(14))

scope = collections.Counter()
for x in eps:
    for c in x['calls']:
        if c['ctype'].startswith(('ctsq', 'grep')):
            cmd = c['cmd'] or ''
            scope['one file' if len(c['files']) == 1 else 'several files' if len(c['files']) > 1 else 'directory/repo'] += 1
print('ctsq+grep scope:', scope.most_common())

# filters applied to ctsq output
filt = collections.Counter()
for x in eps:
    for c in x['calls']:
        if '|grep' in c['ctype']:
            m = re.search(r'\|\s*(?:grep|rg)\s+((?:-[\w]+\s+)*)(\'[^\']*\'|"[^"]*"|\S+)', c['cmd'] or '')
            if m:
                flags, pat = m.group(1).strip(), m.group(2).strip('\'"')
                kind = ('exclude ' if '-v' in flags else '') + ('count' if '-c' in flags else
                        'path/test filter' if re.search(r'test|spec|_test|\.go|\.ts|/', pat) else
                        'name' if re.match(r'^[\w|\\]+$', pat) else 'pattern')
                filt[kind] += 1
print('grep filters on ctsq output:', filt.most_common())

json.dump([{'file': x['file'], 'sub': x['sub'], 'end': x['end'], 'calls': x['calls']} for x in eps if len(x['calls']) >= 3],
          open('code_eps.json', 'w'))
