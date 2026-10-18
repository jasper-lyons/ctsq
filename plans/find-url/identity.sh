#!/bin/bash
# Decision 05 identity: `find error`, route, text, label and component calls on
# Mavexa, emc-gigs and the fixtures, reference binary against a new one.
# Usage: bash plans/find-url/identity.sh <new-binary> [reference-binary]
# Prints each difference and a count line, exits 1 on any difference.
exec python3 -I "$(dirname "$0")/identity.py" "$@"
