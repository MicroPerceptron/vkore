#!/bin/sh
set -eu

demo=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH='' cd -- "$demo/../.." && pwd)
revision=04a57e72df83c747a5863213282b453224060f1a
checkout="$demo/target/kitesurf-$revision"
toolchain=nightly-2026-07-13

# Keep the pinned upstream tree and its generated dependencies out of vKore's workspace.
if [ ! -d "$checkout/.git" ]; then
    mkdir -p "$checkout"
    git -C "$checkout" init -q
fi
if ! git -C "$checkout" cat-file -e "$revision^{commit}" 2>/dev/null; then
    git -C "$checkout" fetch --depth 1 https://github.com/cloudflare/kitesurf.git "$revision"
fi
if [ "$(git -C "$checkout" rev-parse --verify HEAD 2>/dev/null || true)" != "$revision" ]; then
    git -C "$checkout" checkout --detach "$revision"
fi
if ! git -C "$checkout" apply --reverse --check "$demo/wasi-p2.patch" 2>/dev/null; then
    git -C "$checkout" apply "$demo/wasi-p2.patch"
fi

package="$checkout/packages/vkore-kitesurf-browser"
mkdir -p "$package/src" "$package/wit"
cp "$demo/Cargo.toml" "$package/Cargo.toml"
cp "$demo"/src/* "$package/src/"
cp "$root/crates/vkored/wit/browser/browser.wit" "$package/wit/"
cd "$checkout"
RUSTUP_TOOLCHAIN="$toolchain" node --input-type=module -e '
    import { getPatchedVendorCrates } from "./scripts/get-patched-vendor-crates.mjs";
    getPatchedVendorCrates();
'
cargo "+$toolchain" build --locked --profile vkore-component --target wasm32-wasip2 -p vkore-kitesurf-browser

mkdir -p "$root/artifacts/kitesurf-browser-notices"
cp target/wasm32-wasip2/vkore-component/vkore_kitesurf_browser.wasm "$root/artifacts/kitesurf-browser.wasm"
cp LICENSE NOTICE "$root/artifacts/kitesurf-browser-notices/"
cp packages/engine/public/fonts/OFL.txt "$root/artifacts/kitesurf-browser-notices/Noto-Sans-OFL.txt"
printf '%s\n' "$root/artifacts/kitesurf-browser.wasm"
