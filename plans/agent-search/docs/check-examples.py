import os, subprocess, sys, re, shlex

ctsq = os.environ["CTSQ"]
fail = 0

def run(cmd):
    argv = shlex.split(cmd)
    assert argv[0] == "ctsq", cmd
    p = subprocess.run([ctsq] + argv[1:], capture_output=True, text=True)
    return p.returncode, p.stdout + p.stderr

# 1. SKILL.md: fenced blocks inside the "Worked examples" section
text = open(sys.argv[1]).read()
start = text.index("## Worked examples")
end = text.index("\n## ", start + 5)
section = text[start:end]
n = 0
for block in re.findall(r"```\n(.*?)```", section, re.S):
    lines = block.split("\n")
    if not lines[0].startswith("ctsq "):
        continue
    n += 1
    rc, out = run(lines[0])
    have = {l.strip() for l in out.split("\n")}
    for shown in lines[1:]:
        s = shown.strip()
        if not s or s == "...":
            continue
        if s not in have:
            print(f"FAIL skill: {lines[0]}\n  missing line: {s}")
            fail = 1
    if rc != 0:
        print(f"FAIL skill: {lines[0]} exited {rc}")
        fail = 1
print(f"skill: {n} examples checked")

# 2. help: every "  ctsq ..." line in the EXAMPLES section must exit 0
h = subprocess.run([ctsq, "--help"], capture_output=True, text=True).stdout
sec = h[h.index("EXAMPLES"):]
sec = sec[: sec.index("\n\n")]
m = 0
for line in sec.split("\n"):
    if line.startswith("  ctsq "):
        m += 1
        rc, _ = run(line.strip())
        if rc != 0:
            print(f"FAIL help: {line.strip()} exited {rc}")
            fail = 1
print(f"help: {m} examples checked")
sys.exit(fail)
