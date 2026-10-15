#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
make_bin="$(command -v make)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin"

cat >"$tmp/bin/nvidia-smi" <<'MOCK'
#!/usr/bin/env bash
if [[ "${MOCK_GPU:-no}" == yes && "${1:-}" == -L ]]; then
    echo 'GPU 0: simulated CUDA device'
fi
MOCK
cat >"$tmp/bin/ldconfig" <<'MOCK'
#!/usr/bin/env bash
if [[ "${1:-}" == -p ]]; then
    printf '%s\n' \
        'libcuda.so.1 (libc6,x86-64) => /mock/libcuda.so.1' \
        'libnvrtc.so.13 (libc6,x86-64) => /mock/libnvrtc.so.13'
fi
MOCK
chmod +x "$tmp/bin/nvidia-smi" "$tmp/bin/ldconfig"

cpu="$(cd "$root" && env -u GPU MOCK_GPU=no PATH="$tmp/bin:$PATH" "$make_bin" -s info)"
[[ "$cpu" == *'CPU (CUDA unavailable'* ]]

forced_cpu="$(cd "$root" && env MOCK_GPU=yes PATH="$tmp/bin:$PATH" "$make_bin" -s GPU=off info)"
[[ "$forced_cpu" == *'CPU (explicitly selected)'* ]]

auto_cuda="$(cd "$root" && env -u GPU MOCK_GPU=yes PATH="$tmp/bin:$PATH" "$make_bin" -s info)"
[[ "$auto_cuda" == *'CUDA (auto-detected)'* ]]

if (cd "$root" && env MOCK_GPU=no PATH="$tmp/bin:$PATH" "$make_bin" -s GPU=on build) >"$tmp/gpu-on.log" 2>&1; then
    echo 'GPU=on unexpectedly passed without a visible device' >&2
    exit 1
fi
grep -q 'GPU=on requires a visible NVIDIA device' "$tmp/gpu-on.log"

if (cd "$root" && "$make_bin" -s GPU=invalid info) >"$tmp/invalid.log" 2>&1; then
    echo 'invalid GPU mode unexpectedly passed' >&2
    exit 1
fi
grep -q 'GPU must be one of auto, on, or off' "$tmp/invalid.log"

echo 'backend selection tests passed'
