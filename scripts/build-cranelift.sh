#!/usr/bin/env bash
set -euo pipefail

# Build with the Cranelift codegen backend. Requires a nightly toolchain with the
# rustc-codegen-cranelift component installed; the default stable LLVM backend is
# unchanged for normal builds.

target="${1:-$(rustc +nightly -vV | sed -n 's/^host: //p')}"

case "$target" in
  x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu | \
  x86_64-pc-windows-msvc | aarch64-pc-windows-msvc | \
  x86_64-apple-darwin | aarch64-apple-darwin)
    ;;
  *)
    echo "cranelift backend is not supported for target $target; use the default LLVM backend" >&2
    exit 1
    ;;
esac

if ! rustup component list --toolchain nightly | grep -q '^rustc-codegen-cranelift'; then
  rustup component add rustc-codegen-cranelift --toolchain nightly
fi

export RUSTFLAGS="${RUSTFLAGS:-} -Zcodegen-backend=cranelift"

cargo +nightly build --locked "$@"
