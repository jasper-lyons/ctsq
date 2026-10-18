"""Decision 05 identity: run `find` calls with the reference binary and a new
one, and report every call whose stdout, stderr or exit code differs.

Usage: python3 identity.py NEW_BINARY [REFERENCE_BINARY]
"""

import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
REF = "/Users/jasperlyons/.claude/jobs/30494ad5/tmp/fu-ref/target/release/ctsq"
M = "/Users/jasperlyons/workspace/projects/mavexa/mavexa"
E = "/Users/jasperlyons/workspace/projects/eastbournemusic/emc-gigs"
FQ = os.path.join(REPO, "fixtures/queries")

# (directory, args). find takes its root from the current directory.
CALLS = [
    # find error: Mavexa backend, messages as written and as a log shows them
    (M, ["find", "error"]),
    (M, ["find", "error", "name is required"]),
    (M, ["find", "error", "domain must be 1 to 253 characters"]),
    (M, ["find", "error", "fetch https://acme.com/brand: context deadline exceeded"]),
    (M, ["find", "error", "no llm client configured"]),
    (M, ["find", "error", "summarise: rate limited"]),
    (M, ["find", "error", "criteria: unknown field \"colour\""]),
    (M, ["find", "error", "mailer: empty recipient"]),
    (M, ["find", "error", "render: build request: invalid URL"]),
    (M, ["find", "error", "create brand guidelines: UNIQUE constraint failed: brand.id"]),
    (M, ["find", "error", "delete brand guidelines proposal: sql: no rows in result set"]),
    (M, ["find", "error", "decode: unexpected EOF (styleguide.json)"]),
    (M, ["find", "error", "read programs without an event: database is locked"]),
    (M, ["find", "error", "email already registered"]),
    (M, ["find", "error", "websearch: empty query"]),
    (M, ["find", "error", "GetSalesforceOpportunitySnapshot: timeout"]),
    (M, ["find", "error", "level=ERROR msg=\"request failed\" err=\"mailer: empty recipient\" id=42"]),
    (M, ["find", "error", "/^summarise/"]),
    (M, ["find", "error", "x"]),
    (M, ["find", "error", "nothing anywhere matches this message qq"]),
    # find error: Mavexa frontend
    (M, ["find", "error", "create failed"]),
    (M, ["find", "error", "Public report fetch failed: 500 Internal Server Error"]),
    (M, ["find", "error", "Save chat failed: 403 Forbidden"]),
    # find error: emc-gigs
    (E, ["find", "error"]),
    (E, ["find", "error", "unknown attrs: phone, fax"]),
    (E, ["find", "error", "cannot import a refused result"]),
    (E, ["find", "error", "Expected 12 gigs to keep their owner"]),
    (E, ["find", "error", "\"The Dome\" missing from mapping!"]),
    (E, ["find", "error", "acts collide after normalisation"]),
    (E, ["find", "error", "sjis not supported."]),
    (E, ["find", "error", "--tests", "unknown"]),
    # find error: fixtures
    (os.path.join(FQ, "errors"), ["find", "error"]),
    (os.path.join(FQ, "errors"), ["find", "error", "failed to load pipeline abc-1: open x: no such file"]),
    (os.path.join(FQ, "errors"), ["find", "error", "render 3 blocks ({1 2})"]),
    (os.path.join(FQ, "errors"), ["find", "error", "bad field 7"]),
    (os.path.join(FQ, "errors"), ["find", "error", "Slot 4 is taken"]),
    (os.path.join(FQ, "errors"), ["find", "error", "nothing to parse"]),
    (os.path.join(FQ, "errors"), ["find", "error", "pipeline is empty (id=3)"]),
    (os.path.join(FQ, "errors"), ["find", "error", "something unrelated happened"]),
    # find route / text / label / component with keys that exist today
    (M, ["find"]),
    (M, ["find", "route"]),
    (M, ["find", "route", "workspaces"]),
    (M, ["find", "route", "/api/workspaces/{workspaceID}"]),
    (M, ["find", "route", "GET /api/me"]),
    (M, ["find", "route", "/login"]),
    (M, ["find", "route", "/^\\/api\\/auth/"]),
    (M, ["find", "text", "This workspace has no brand guidelines"]),
    (M, ["find", "text", "Loading"]),
    (M, ["find", "label", "Alt text"]),
    (M, ["find", "label", "name@company.com"]),
    (M, ["find", "component", "BrandProposalReview"]),
    (M, ["find", "component", "CreateButton"]),
    (E, ["find"]),
    (E, ["find", "route"]),
    (E, ["find", "route", "/gigs"]),
    (E, ["find", "route", "past"]),
    (E, ["find", "route", "login"]),
    (os.path.join(FQ, "solid"), ["find", "route", "/about"]),
    (os.path.join(FQ, "react-router"), ["find", "route", "team"]),
    (os.path.join(FQ, "go-http"), ["find", "route", "/api/auth"]),
    (os.path.join(FQ, "go-chi"), ["find", "route", "users"]),
    (os.path.join(FQ, "rails"), ["find", "route", "contact"]),
    (os.path.join(FQ, "next-app"), ["find", "route", "blog"]),
    (os.path.join(FQ, "jsx"), ["find", "text"]),
    (os.path.join(FQ, "jsx"), ["find", "label"]),
    (os.path.join(FQ, "jsx"), ["find", "component"]),
]


def run(binary, cwd, args):
    p = subprocess.run([binary] + args, cwd=cwd, capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


def main():
    new = sys.argv[1]
    ref = sys.argv[2] if len(sys.argv) > 2 else REF
    new = os.path.abspath(new)
    diffs = 0
    for cwd, args in CALLS:
        a = run(ref, cwd, args)
        b = run(new, cwd, args)
        if a != b:
            diffs += 1
            print("DIFF in %s: ctsq %s" % (cwd, " ".join(repr(x) for x in args)))
            for label, x, y in (("code", a[0], b[0]), ("stdout", a[1], b[1]), ("stderr", a[2], b[2])):
                if x != y:
                    print("  %s ref: %r" % (label, x[:600] if isinstance(x, str) else x))
                    print("  %s new: %r" % (label, y[:600] if isinstance(y, str) else y))
    errors = sum(1 for _, a in CALLS if a[1:2] == ["error"])
    print("%d calls (%d find error), %d differences" % (len(CALLS), errors, diffs))
    return 1 if diffs else 0


if __name__ == "__main__":
    sys.exit(main())
