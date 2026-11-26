#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

# Keep debug signing but omit debug symbols and enable enough optimization for
# an initial real-time audio test. NDK r27 needs explicit 16 KiB ELF alignment.
export CARGO_PROFILE_DEV_OPT_LEVEL=1 CARGO_PROFILE_DEV_DEBUG=0
alignment="-Clink-arg=-Wl,-z,max-page-size=16384"
if [[ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]]; then
  export CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS}"$'\x1f'"${alignment}"
else
  export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }${alignment}"
fi
cargo apk build -p leviathan-app --lib
