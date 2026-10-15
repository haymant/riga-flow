#!/usr/bin/env bash
set -euo pipefail

# cudarc JIT-compiles the kernels through NVRTC; nvcc is not required. Auto mode
# selects CUDA only when a device, driver library, and the CUDA 13 NVRTC runtime
# required by the pinned cudarc feature are all visible.
command -v nvidia-smi >/dev/null 2>&1 || exit 1
nvidia-smi -L 2>/dev/null | grep -q '^GPU ' || exit 1

driver_found=false
nvrtc13_found=false
if command -v ldconfig >/dev/null 2>&1; then
    ldconfig -p 2>/dev/null | grep -Eq 'libcuda\.so\.1[[:space:]]' && driver_found=true
    ldconfig -p 2>/dev/null | grep -Eq 'libnvrtc\.so\.13([.[:space:]]|$)' && nvrtc13_found=true
fi

for root in "${CUDA_HOME:-}" "${CUDA_PATH:-}" /usr/local/cuda /opt/cuda /usr/lib /usr/lib64 /lib /lib64; do
    [[ -n "$root" ]] || continue
    for libdir in "$root/lib64" "$root/lib" "$root/targets/x86_64-linux/lib" "$root/x86_64-linux-gnu"; do
        if compgen -G "$libdir/libcuda.so.1" >/dev/null; then
            driver_found=true
        fi
        if compgen -G "$libdir/libnvrtc.so.13*" >/dev/null; then
            nvrtc13_found=true
        fi
    done
done

[[ "$driver_found" == true && "$nvrtc13_found" == true ]]
