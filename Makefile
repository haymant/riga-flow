# Canonical local build/test entrypoints. Cargo itself keeps its portable CPU defaults;
# `make` auto-enables CUDA only when this host exposes a CUDA-capable NVIDIA runtime.
CARGO ?= $(shell if command -v cargo >/dev/null 2>&1; then command -v cargo; elif [ -x "$$HOME/.cargo/bin/cargo" ]; then printf '%s' "$$HOME/.cargo/bin/cargo"; else printf cargo; fi)
GPU ?= auto

ifeq ($(GPU),auto)
CUDA_AVAILABLE := $(shell ./scripts/cuda_available.sh >/dev/null 2>&1 && printf yes || printf no)
ifeq ($(CUDA_AVAILABLE),yes)
GPU_FEATURES := --features riga-engine/gpu
GPU_MODE := CUDA (auto-detected)
else
GPU_FEATURES :=
GPU_MODE := CPU (CUDA unavailable; GPU feature off)
endif
else ifeq ($(GPU),on)
GPU_FEATURES := --features riga-engine/gpu
GPU_MODE := CUDA (explicitly requested; no fallback)
else ifeq ($(GPU),off)
GPU_FEATURES :=
GPU_MODE := CPU (explicitly selected)
else
$(error GPU must be one of auto, on, or off; got '$(GPU)')
endif

.PHONY: all info build test lint fmt check fixture gpu-preflight gpu-test

all: build

info:
	@printf 'Selected backend: %s\n' '$(GPU_MODE)'

# GPU=on is an explicit hardware request and must not silently compile on a CPU-only host.
gpu-preflight:
	@if [ "$(GPU)" = "on" ]; then \
		./scripts/cuda_available.sh || { echo 'GPU=on requires a visible NVIDIA device, driver, and CUDA 13 NVRTC runtime'; exit 1; }; \
	fi

build: gpu-preflight
	@echo 'Building workspace — $(GPU_MODE)'
	$(CARGO) build --workspace $(GPU_FEATURES)

test: gpu-preflight
	@echo 'Testing workspace — $(GPU_MODE)'
	$(CARGO) test --workspace $(GPU_FEATURES)

lint: gpu-preflight
	$(CARGO) clippy --workspace --all-targets $(GPU_FEATURES) -- -D warnings

fmt:
	$(CARGO) fmt --all -- --check

check: fmt lint test
	./scripts/dep_hygiene.sh
	./scripts/test_backend_selection.sh

fixture: gpu-preflight
	$(CARGO) run --release $(GPU_FEATURES) -q -p riga-cli -- price-put docs/fixture/termsheet1.json

# Hardware-dependent CUDA gate; no auto-selection or CPU fallback.
gpu-test:
	@./scripts/cuda_available.sh || { echo 'gpu-test requires a visible NVIDIA device, driver, and CUDA 13 NVRTC runtime'; exit 1; }
	$(CARGO) test -p riga-engine --features gpu
