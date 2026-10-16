"""Extract code-search tool calls from Claude Code transcripts into events.jsonl.

Usage: python3 -I extract_search.py DAYS OUT_DIR
"""
import json, os, re, sys, time, glob

DAYS = int(sys.argv[1]); OUT = sys.argv[2]
ROOT = os.path.expanduser('~/.claude/projects')
cutoff = time.time() - DAYS * 86400

SRC_EXT = re.compile(r'\.(c|cc|cpp|h|hpp|py|js|jsx|ts|tsx|rs|go|rb|erb|java|cs|lua|svelte|vue|sql|sh)\b')
SHELL_KINDS = [
    ('ctsq', re.compile(r'(^|[\s;&|(])ctsq\s')),
    ('semble', re.compile(r'(^|[\s;&|(])semble\s')),
    ('mdq', re.compile(r'(^|[\s;&|(])mdq\s')),
    ('grep', re.compile(r'(^|[\s;&|(])(grep|rg|egrep|ag|git grep)\s')),
    ('find', re.compile(r'(^|[\s;&|(])(find|fd|ls|tree)\s')),
    ('read', re.compile(r'(^|[\s;&|(])(cat|head|tail|sed -n|less|bat|wc)\s')),
    ('git', re.compile(r'(^|[\s;&|(])git (log|show|diff|blame)\b')),
]

def text_of(content):
    if isinstance(content, str):
        return content
    out = []
    for c in content or []:
        if c.get('type') == 'text':
            out.append(c.get('text', ''))
    return '\n'.join(out)

def classify(name, inp):
    if name == 'Grep':
        return 'grep', f"Grep {inp.get('pattern')} path={inp.get('path','')} glob={inp.get('glob','')} mode={inp.get('output_mode','')}"
    if name == 'Glob':
        return 'find', f"Glob {inp.get('pattern')} path={inp.get('path','')}"
    if name == 'Read':
        p = inp.get('file_path', '')
        kind = 'read_src' if SRC_EXT.search(p) else ('read_md' if p.endswith('.md') else 'read_other')
        return kind, f"Read {p} off={inp.get('offset','')} lim={inp.get('limit','')}"
    if name == 'LSP':
        return 'lsp', 'LSP ' + json.dumps(inp)[:300]
    if name == 'Bash':
        cmd = inp.get('command', '')
        kinds = [k for k, rx in SHELL_KINDS if rx.search(cmd)]
        if kinds:
            return kinds[0], cmd[:600]
        return None, None
    if name in ('Agent', 'Task'):
        st = inp.get('subagent_type', '')
        return 'agent', f"[{st}] {inp.get('description','')} :: {inp.get('prompt','')[:1500]}"
    return None, None

files = [f for f in glob.glob(ROOT + '/**/*.jsonl', recursive=True) if os.path.getmtime(f) >= cutoff]
ev_out = open(os.path.join(OUT, 'events.jsonl'), 'w')
n_ev = 0
for f in files:
    proj = f[len(ROOT) + 1:].split('/')[0]
    is_sub = '/subagents/' in f
    pending = {}          # tool_use_id -> event
    last_prompt = ''
    last_text = ''
    seq = 0
    try:
        lines = open(f, errors='replace').readlines()
    except OSError:
        continue
    for line in lines:
        try:
            r = json.loads(line)
        except ValueError:
            continue
        t = r.get('type')
        msg = r.get('message') or {}
        if t == 'user':
            c = msg.get('content')
            if isinstance(c, str):
                last_prompt = c[:800]; continue
            for item in c or []:
                if item.get('type') == 'tool_result':
                    ev = pending.pop(item.get('tool_use_id'), None)
                    if ev:
                        res = text_of(item.get('content')) if not isinstance(item.get('content'), str) else item['content']
                        ev['result_chars'] = len(res)
                        ev['error'] = bool(item.get('is_error'))
                        ev['result_head'] = res[:300]
                        ev_out.write(json.dumps(ev) + '\n'); n_ev += 1
                elif item.get('type') == 'text' and not item.get('text', '').startswith('<'):
                    last_prompt = item['text'][:800]
        elif t == 'assistant':
            for item in msg.get('content') or []:
                it = item.get('type')
                if it == 'text':
                    last_text = item.get('text', '')[-500:]
                elif it == 'thinking' and item.get('thinking'):
                    last_text = '[thinking] ' + item['thinking'][-500:]
                elif it == 'tool_use':
                    seq += 1
                    kind, cmd = classify(item.get('name'), item.get('input') or {})
                    ev = {'file': f[len(ROOT) + 1:], 'proj': proj, 'sub': is_sub, 'seq': seq,
                          'ts': r.get('timestamp'), 'tool': item.get('name'), 'kind': kind,
                          'cmd': cmd, 'prompt': last_prompt[:400], 'why': last_text}
                    if kind is None:
                        # keep a stub so episodes can be split on edits and other actions
                        ev['cmd'] = item.get('name')
                    pending[item.get('id')] = ev
                    last_text = ''
    for ev in pending.values():
        ev_out.write(json.dumps(ev) + '\n'); n_ev += 1
print('files', len(files), 'events', n_ev)
