#!/usr/bin/env bash
# Build every release artifact from a Mac and put them in dist/.
#
# Needs: rustup targets x86_64-apple-darwin and aarch64-unknown-linux-gnu,
# plus zig and cargo-zigbuild for the Linux builds (see README, Raspberry Pi).
set -euo pipefail
cd "$(dirname "$0")/.."

version=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
tag="v$version"

# Refuse to build artifacts for a version the tag does not agree with.
if head=$(git describe --tags --exact-match 2>/dev/null) && [ "$head" != "$tag" ]; then
  echo "HEAD is tagged $head but Cargo.toml says $version" >&2
  exit 1
fi

rm -rf dist
mkdir -p dist

# build <target[.glibc]> <cargo subcommand> [extra cargo args...]
build() {
  local target=$1 tool=$2
  shift 2
  local triple=${target%%.*}
  echo "==> $triple"
  cargo "$tool" --release --target "$target" "$@"
  tar -C "target/$triple/release" -czf "dist/tuican-$tag-$triple.tar.gz" tuican
}

build aarch64-apple-darwin build
build x86_64-apple-darwin build
# Linux: no udev, so the binaries need nothing installed beyond libc.
build x86_64-unknown-linux-gnu.2.31 zigbuild --no-default-features --features vendored-libusb
build aarch64-unknown-linux-gnu.2.31 zigbuild --no-default-features --features vendored-libusb

(cd dist && shasum -a 256 -- *.tar.gz > SHA256SUMS)
echo
ls -l dist
