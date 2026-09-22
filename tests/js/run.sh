#!/usr/bin/env bash
# Drives src/web/mention.js in headless Chrome and prints what it did, as
# JSON. `cargo test` cannot run a script; this is the measurement instead
# (plan-stage-4.md, task 3). Needs google-chrome or chromium on PATH.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
port=${PORT:-8766}
profile=$(mktemp -d)
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$root" >/dev/null 2>&1 &
server=$!
trap 'kill $server; rm -rf "$profile"' EXIT
for _ in $(seq 20); do curl -s -o /dev/null "http://127.0.0.1:$port/" && break; sleep 0.2; done
chrome=$(command -v google-chrome || command -v chromium)
# Compared against expected.txt: the first line is the normal run, the second
# the run in which /mentionable fails (nothing may open, nothing is changed).
# A change to what the script does changes that file, in the same commit.
actual=$(for query in "" "?fail"; do
  "$chrome" --headless=new --disable-gpu --user-data-dir="$profile" \
    --virtual-time-budget=5000 --dump-dom \
    "http://127.0.0.1:$port/tests/js/mention.html$query" 2>/dev/null \
    | sed -n 's:.*<pre id="out">\(.*\)</pre>.*:\1:p'
done)
# PRINT=1 writes what the script did instead of comparing — to renew
# expected.txt after a deliberate change, and to read it before committing.
if [ "${PRINT:-}" = 1 ]; then
  echo "$actual"
elif [ "$actual" = "$(cat "$root/tests/js/expected.txt")" ]; then
  echo "mention.js: as expected (2 runs)"
else
  echo "mention.js: NOT as expected" >&2
  diff <(echo "$actual") "$root/tests/js/expected.txt" >&2 || true
  exit 1
fi
