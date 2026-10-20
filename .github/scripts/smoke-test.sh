#!/usr/bin/env bash
# Starts an image and waits for /api/health to answer. Usage: smoke-test.sh <image>
set -euo pipefail

image="$1"
name="kessel-smoke-$$"

docker run -d --name "$name" -p 18099:8099 "$image" > /dev/null
trap 'docker rm -f "$name" > /dev/null 2>&1 || true' EXIT

for _ in $(seq 1 30); do
  if body="$(curl -fsS http://127.0.0.1:18099/api/health 2> /dev/null)"; then
    echo "health: $body"
    grep -q '"name":"kessel"' <<< "$body"
    exit 0
  fi
  sleep 2
done

echo "::error::$image did not answer /api/health within 60s"
docker logs "$name" || true
exit 1
