#!/usr/bin/env bash
# Starts an HTTP server for the test PAC scripts on port 8080 and waits until it responds.
set -u
python=$(command -v python3 || command -v python)
"$python" -m http.server 8080 --bind 127.0.0.1 --directory "$GITHUB_WORKSPACE/tests" > "$RUNNER_TEMP/http_server.log" 2>&1 &
for _ in $(seq 1 30); do
  if curl -sf -o /dev/null http://127.0.0.1:8080/pac.js; then
    exit 0
  fi
  sleep 1
done
echo "HTTP server did not start"
cat "$RUNNER_TEMP/http_server.log"
exit 1
