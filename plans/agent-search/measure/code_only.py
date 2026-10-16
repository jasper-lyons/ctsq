"""Keep only code-search calls, give each a fine-grained type, write code_events.jsonl."""
import json, re, collections

CODE_EXT = r'(c|cc|cpp|h|hpp|py|js|jsx|mjs|ts|tsx|rs|go|rb|erb|rake|java|cs|lua|svelte|vue|sql|sh|json|ya?ml|toml|css|scss|html|graphql|proto|tf|mod|gemspec)'
CODE_FILE = re.compile(r'[\w./-]+\.' + CODE_EXT + r'\b')
NOT_CODE_PATH = re.compile(r'\.log\b|/tmp/|/private/tmp|\.output\b|/tasks/|\.md\b|\.txt\b|\.jsonl\b|/\.claude/projects/')
CMD_UPSTREAM = re.compile(r'go test|vitest|npm|pnpm|yarn|pytest|rspec|bun |cargo|make|playwright|\bgit\b|curl|ssh|docker|dokku|tsc|eslint|ps |env\b|brief |wacli|gws |aws |psql|sqlite|python|ruby |node ')
IDENT = re.compile(r'^[A-Za-z_$][\w$]*$')

def term_shape(t):
    t = t.strip('/"\'')
    alts = [a.strip('^$\\b()') for a in re.split(r'\\?\|', t) if a]
    if alts and all(IDENT.match(a) for a in alts):
        if all(re.match(r'^[A-Z][A-Z0-9_]{2,}$', a) for a in alts): return 'config_key'
        return 'name' if len(alts) == 1 else 'names'
    if re.search(r'[A-Za-z]{2,}\s+[A-Za-z]{2,}', t) and not re.search(r'[.*\\(]', t): return 'text'
    return 'pattern'

def classify(e):
    """Return fine type or None if not code search."""
    k, cmd, tool = e['kind'], e['cmd'] or '', e['tool']
    if k == 'ctsq':
        m = re.search(r'ctsq\s+(\S+)', cmd); first = m.group(1) if m else ''
        if first in ('explain', 'tree', 'def', 'callers', 'callees', 'callgraph', 'files', 'example', 'refs', 'path'):
            t = 'ctsq:' + first
        else:
            sel = re.search(r"([*&]?)(\w+)#", cmd)
            s = (sel.group(1) + sel.group(2)) if sel else 'raw'
            body = ' --body' in cmd
            t = f"ctsq:{s}{'+body' if body else ''}{'+C' if re.search(r' -C ?\d', cmd) else ''}"
        if re.search(r'\|\s*(grep|rg)\b', cmd): t += '|grep'
        return t
    if k in ('semble', 'lsp'): return k
    if tool == 'Read':
        p = cmd.split()[1] if len(cmd.split()) > 1 else ''
        if NOT_CODE_PATH.search(p) or not CODE_FILE.search(p): return None
        return 'read:window' if re.search(r'off=\d|lim=\d', cmd) else 'read:whole'
    if tool == 'Grep':
        m = re.match(r'Grep (.*?) path=(\S*)', cmd)
        if not m or NOT_CODE_PATH.search(m.group(2)): return None
        return 'grep:' + term_shape(m.group(1))
    if tool == 'Glob':
        return None if NOT_CODE_PATH.search(cmd) else 'locate'
    if k == 'grep':
        up = re.search(r'^(.*?)\|\s*(grep|rg)\b', cmd)
        if up and (CMD_UPSTREAM.search(up.group(1)) or not CODE_FILE.search(up.group(1))): return None
        if NOT_CODE_PATH.search(cmd): return None
        m = re.search(r'\b(?:grep|rg)\s+(?:-[\w-]+(?:=\S+)?\s+)*(?:-e\s+)?(\'[^\']*\'|"[^"]*"|\S+)', cmd)
        return 'grep:' + (term_shape(m.group(1)) if m else 'pattern')
    if k == 'read':
        if CMD_UPSTREAM.search(cmd.split('|')[0]) or NOT_CODE_PATH.search(cmd) or not CODE_FILE.search(cmd): return None
        if re.search(r'^\s*wc\b', cmd): return 'read:size'
        return 'read:window' if re.search(r'sed -n|head|tail', cmd) else 'read:whole'
    if k == 'find':
        if NOT_CODE_PATH.search(cmd) or CMD_UPSTREAM.search(cmd): return None
        return 'locate'
    return None

def files_in(e):
    return set(m.group(0) for m in CODE_FILE.finditer(e['cmd'] or ''))

out = open('code_events.jsonl', 'w')
c = collections.Counter(); dropped = collections.Counter(); n = 0
for l in open('events.jsonl'):
    e = json.loads(l)
    if e['kind'] is None:
        e['ctype'] = 'ACTION:' + (e['tool'] or '?')     # keep non-search tools as episode boundaries
    elif e['kind'] in ('agent',):
        e['ctype'] = 'ACTION:Agent'
    else:
        t = classify(e)
        if t is None:
            dropped[e['kind']] += 1
            e['ctype'] = 'NONCODE'
        else:
            e['ctype'] = t; c[t] += 1; n += 1
    e['files'] = sorted(files_in(e))
    out.write(json.dumps(e) + '\n')
print('code-search calls', n)
print('dropped as non-code by kind', dropped.most_common())
for t, k in c.most_common(40): print(f'  {t:28} {k}')
