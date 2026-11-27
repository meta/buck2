#!/bin/sh
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Installs a prebuilt buck2 for this machine (Linux or macOS):
#
#   curl -fsSL https://raw.githubusercontent.com/facebook/buck2/main/install.sh | sh
#
# Environment:
#   BUCK2_VERSION      release tag to install (default: latest)
#   BUCK2_INSTALL_DIR  directory to install buck2 into (default: $HOME/.local/bin)
#   BUCK2_RELEASES_URL base URL of the release downloads
#                      (default: https://github.com/facebook/buck2/releases/download)

set -eu

version="${BUCK2_VERSION:-latest}"
install_dir="${BUCK2_INSTALL_DIR:-$HOME/.local/bin}"
releases_url="${BUCK2_RELEASES_URL:-https://github.com/facebook/buck2/releases/download}"
docs="https://buck2.build/docs/getting_started/install/"

fail() {
    echo "buck2 install: $*" >&2
    exit 1
}

case "$(uname -s)" in
    Linux)
        if ldd --version 2>&1 | grep -q musl; then
            os=unknown-linux-musl
        else
            os=unknown-linux-gnu
        fi
        ;;
    Darwin) os=apple-darwin ;;
    *) fail "no prebuilt buck2 for $(uname -s); see $docs" ;;
esac

case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    riscv64) arch=riscv64gc ;;
    *) fail "no prebuilt buck2 for $(uname -m); see $docs" ;;
esac

if [ "$arch" = riscv64gc ] && [ "$os" != unknown-linux-gnu ]; then
    fail "no prebuilt buck2 for riscv64 $os; see $docs"
fi

url="$releases_url/$version/buck2-$arch-$os.zst"

download=
binary=
rc=
for sig in 1 2 3 13 15; do eval "trap 'exit $((sig + 128))' $sig"; done
trap 'rc=$?; set +e; rm -f "$download" "$binary"; exit $rc' EXIT

mkdir -p "$install_dir"
download=$(mktemp "$install_dir/.buck2.zst.XXXXXX")
binary=$(mktemp "$install_dir/.buck2.XXXXXX")

echo "Downloading $url"
if command -v curl > /dev/null 2>&1; then
    case "$url" in
        https://*) proto='=https' ;;
        *) proto='=https,http' ;;
    esac
    curl --proto "$proto" -fL --retry 3 -o "$download" "$url" || fail "could not download $url"
elif command -v wget > /dev/null 2>&1; then
    wget -q -O "$download" "$url" || fail "could not download $url"
else
    fail "needs curl or wget"
fi

# The release binaries are zstd-compressed. Python has zstd from 3.14.
if command -v zstd > /dev/null 2>&1; then
    zstd -q -d -f "$download" -o "$binary"
elif command -v python3 > /dev/null 2>&1 && python3 -c 'import compression.zstd' > /dev/null 2>&1; then
    python3 -c 'import compression.zstd, sys; open(sys.argv[2], "wb").write(compression.zstd.decompress(open(sys.argv[1], "rb").read()))' "$download" "$binary"
else
    fail "needs zstd to unpack $url: install it (for example 'apt install zstd' or 'brew install zstd') and run this again"
fi

chmod +x "$binary"
mv -f "$binary" "$install_dir/buck2"
binary=

echo "Installed $("$install_dir/buck2" --version) to $install_dir/buck2"
case ":$PATH:" in
    *":$install_dir:"*) ;;
    *) echo "Add $install_dir to your PATH, for example: export PATH=\"$install_dir:\$PATH\"" ;;
esac
