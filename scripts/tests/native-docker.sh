#!/usr/bin/env bash
# Native acceptance for `devtrim clean docker` against a real, disposable
# Docker engine: `docker:27-dind` in a Linux VM run by Apple's `container`
# tool, its socket published to the Mac. The developer's own Docker daemon,
# contexts and images are never touched: HOME and DOCKER_CONFIG point at a
# temporary folder, and the only endpoint devtrim sees is the disposable one.
#
# Proves what recording stubs cannot: the real daemon removes every unused
# image and all build cache, and a volume with data survives.
#
# usage: scripts/tests/native-docker.sh <devtrim-binary>
# Needs: Apple `container` with its default kernel installed, the Docker CLI
# with buildx, and network access to pull docker:27-dind and busybox.
set -euo pipefail

[[ $# -eq 1 && -x "$1" ]] || { echo 'usage: scripts/tests/native-docker.sh <devtrim-binary>' >&2; exit 2; }
binary="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
docker_cli="$(command -v docker)"
command -v container >/dev/null
buildx=
for candidate in "$(dirname "$(readlink -f "$docker_cli")")/../lib/docker/cli-plugins/docker-buildx" \
  /opt/homebrew/lib/docker/cli-plugins/docker-buildx /usr/local/lib/docker/cli-plugins/docker-buildx; do
  [[ -x "$candidate" ]] && { buildx="$candidate"; break; }
done
[[ -n "$buildx" ]] || { echo "native-docker: the Docker CLI's buildx plugin was not found" >&2; exit 1; }
home="$(mktemp -d /tmp/devtrim-native-docker-home.XXXXXX)"
socket_dir="$home/socket"
socket="$socket_dir/docker.sock"
name="devtrim-native-docker-$$-${home##*.}"
docker_config="$home/.docker"

cleanup() {
  container stop "$name" >/dev/null 2>&1 || true
  container rm "$name" >/dev/null 2>&1 || true
}
finish() {
  cleanup
  rm -r -f -- "$home"
}
trap finish EXIT

fail() { echo "native-docker: FAIL: $*" >&2; exit 1; }
docker() { env -u DOCKER_HOST -u DOCKER_CONTEXT -u DOCKER_TLS_VERIFY -u DOCKER_CERT_PATH HOME="$home" DOCKER_CONFIG="$docker_config" "$docker_cli" "$@"; }
engine() { docker --host "unix://$socket" "$@"; }

mkdir -p "$socket_dir"
container system start --disable-kernel-install
container run --detach --name "$name" --cap-add ALL --env DOCKER_TLS_CERTDIR= \
  --publish-socket "$socket:/var/run/docker.sock" docker:27-dind \
  --bridge=none --iptables=false --ip-forward=false >/dev/null

mkdir -p "$docker_config/cli-plugins"
ln -s "$buildx" "$docker_config/cli-plugins/docker-buildx"
for _ in $(seq 60); do engine version >/dev/null 2>&1 && break; sleep 1; done
engine version >/dev/null || fail "the disposable engine never answered on $socket"

# Fixture: an unused image, build cache from BuildKit, and a volume with data.
# The VM's /proc/sys is read-only, so the engine runs without a bridge
# network; nothing here needs one.
engine pull --quiet busybox:1.36 >/dev/null
printf 'FROM busybox:1.36\nRUN echo layer > /layer\n' |
  engine buildx build --quiet --load --network none --tag devtrim-native:cache - >/dev/null
engine volume create devtrim-native-data >/dev/null
engine run --rm --network none --volume devtrim-native-data:/data busybox:1.36 sh -c 'echo keep > /data/file'
[[ -n "$(engine image ls --quiet)" ]] || fail "fixture has no image"
cache="$(engine system df --format '{{.Type}} {{.TotalCount}}' | awk '$1=="Build"{print $3}')"
[[ "$cache" =~ ^[0-9]+$ && "$cache" -gt 0 ]] ||
  fail "fixture has no build cache"

docker context create devtrim-native --docker "host=unix://$socket" >/dev/null
docker context use devtrim-native >/dev/null
[[ "$(docker context inspect --format '{{.Endpoints.docker.Host}}' devtrim-native)" == "unix://$socket" ]] ||
  fail "the selected Docker context does not name the disposable socket"

# devtrim sees the real Docker CLI and nothing else from the host's PATH.
mkdir -p "$home/bin"
ln -s "$docker_cli" "$home/bin/docker"
printf '#!/bin/sh\nexit 1\n' > "$home/bin/pgrep"
chmod +x "$home/bin/pgrep"
devtrim() { env -u XDG_CONFIG_HOME -u XDG_STATE_HOME -u DOCKER_HOST -u DOCKER_CONTEXT -u DOCKER_TLS_VERIFY -u DOCKER_CERT_PATH HOME="$home" DOCKER_CONFIG="$docker_config" PATH="$home/bin:/usr/bin:/bin" "$binary" "$@"; }

preview="$(devtrim clean docker --json)" || fail "preview failed: $preview"
grep -q '"image","prune"' <<<"${preview//[[:space:]]/}" || fail "preview offers no image prune: $preview"
apply="$(devtrim clean docker --apply --yolo --json)" || fail "apply failed: $apply"

[[ -z "$(engine image ls --quiet)" ]] || fail "unused images survived: $(engine image ls)"
cache="$(engine system df --format '{{.Type}} {{.TotalCount}}' | awk '$1=="Build"{print $3}')"
[[ "$cache" == "0" ]] || fail "build cache survived ($cache records)"
engine volume inspect devtrim-native-data >/dev/null || fail "the volume was removed"
engine pull --quiet busybox:1.36 >/dev/null
[[ "$(engine run --rm --network none --volume devtrim-native-data:/data busybox:1.36 cat /data/file)" == keep ]] ||
  fail "the volume's data changed"
echo "native-docker: the real daemon pruned every unused image and all build cache; the volume and its data survived"
