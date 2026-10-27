#!/usr/bin/env bash
# Builds rust/ into dist/wasm/ and checks the gzipped module fits the Workers Free plan.
#
#   PLATFORM_DIR=~/src/platform scripts/build.sh   # build against a local dashpay/platform checkout
#   MAX_GZIP_BYTES=...                             # size gate, default 2.8 MiB
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/dist/wasm"
max_gzip_bytes="${MAX_GZIP_BYTES:-2936012}"

cargo_args=(build --release --manifest-path "$root/rust/Cargo.toml" --target wasm32-unknown-unknown)
if [[ -n "${PLATFORM_DIR:-}" ]]; then
  # Patching rewrites Cargo.lock to the local paths; put the committed one back afterwards.
  cp "$root/rust/Cargo.lock" "$root/rust/Cargo.lock.committed"
  trap 'mv "$root/rust/Cargo.lock.committed" "$root/rust/Cargo.lock"' EXIT
  for crate in dash-sdk:rs-sdk rs-sdk-trusted-context-provider:rs-sdk-trusted-context-provider simple-signer:simple-signer; do
    cargo_args+=(--config "patch.'https://github.com/dashpay/platform'.${crate%%:*}.path='$PLATFORM_DIR/packages/${crate#*:}'")
  done
fi

# secp256k1-sys compiles C for wasm32; Apple's clang cannot target it.
if [[ "$(uname)" == Darwin && -z "${CC_wasm32_unknown_unknown:-}" ]]; then
  llvm="$(brew --prefix llvm)/bin"
  export CC_wasm32_unknown_unknown="$llvm/clang" AR_wasm32_unknown_unknown="$llvm/llvm-ar"
fi

cargo "${cargo_args[@]}"

target_dir="${CARGO_TARGET_DIR:-$root/rust/target}"
wasm-bindgen --target web --out-dir "$out" --out-name dash_platform \
  "$target_dir/wasm32-unknown-unknown/release/dash_platform_workers.wasm"
wasm-opt --strip-producers -Oz --enable-bulk-memory --enable-nontrapping-float-to-int \
  "$out/dash_platform_bg.wasm" -o "$out/dash_platform_bg.wasm"

gzip_bytes="$(gzip -9c "$out/dash_platform_bg.wasm" | wc -c | tr -d ' ')"
echo "dash_platform_bg.wasm: $(wc -c <"$out/dash_platform_bg.wasm" | tr -d ' ') bytes, $gzip_bytes gzipped (limit $max_gzip_bytes)"
if (( gzip_bytes > max_gzip_bytes )); then
  echo "error: the module no longer leaves room in the Free plan's 3 MiB Worker limit" >&2
  exit 1
fi
