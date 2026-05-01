#!/usr/bin/env bash
# Build host + every available cross target and stage all binaries under dist/
# with stable, platform-tagged filenames:
#
#   dist/aweme-db-decrypt-macos-arm64
#   dist/aweme-db-decrypt-windows-x86_64.exe
#   dist/aweme-db-decrypt-linux-x86_64
#   ...
#
# Targets whose toolchain isn't installed are skipped with a one-line note.

set -euo pipefail
cd "$(dirname "$0")"

NAME=aweme-db-decrypt
DIST=dist
mkdir -p "$DIST"

triple_to_tag() {
    case "$1" in
        aarch64-apple-darwin)       echo "macos-arm64" ;;
        x86_64-apple-darwin)        echo "macos-x86_64" ;;
        x86_64-unknown-linux-gnu)   echo "linux-x86_64" ;;
        aarch64-unknown-linux-gnu)  echo "linux-arm64" ;;
        x86_64-unknown-linux-musl)  echo "linux-x86_64" ;;
        aarch64-unknown-linux-musl) echo "linux-arm64" ;;
        x86_64-pc-windows-gnu)      echo "windows-x86_64" ;;
        x86_64-pc-windows-msvc)     echo "windows-x86_64" ;;
        aarch64-pc-windows-msvc)    echo "windows-arm64" ;;
        *)                          echo "$1" ;;
    esac
}

is_windows() { [[ "$1" == *-windows-* ]]; }

build_one() {
    local triple=$1
    local tag suffix out_dir
    tag=$(triple_to_tag "$triple")
    suffix=""
    is_windows "$triple" && suffix=".exe"

    if [[ "$triple" == "$HOST" ]]; then
        echo "==> [host]  $triple → $tag"
        cargo build --release
        out_dir="target/release"
    else
        echo "==> [cross] $triple → $tag"
        case "$triple" in
            x86_64-pc-windows-gnu)
                if ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
                    echo "    skipped: x86_64-w64-mingw32-gcc not on PATH (brew install mingw-w64)"
                    return 0
                fi
                CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
                AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar \
                cargo build --release --target "$triple"
                ;;
            x86_64-unknown-linux-musl)
                if ! command -v x86_64-linux-musl-gcc >/dev/null 2>&1; then
                    echo "    skipped: x86_64-linux-musl-gcc not on PATH (brew install FiloSottile/musl-cross/musl-cross)"
                    return 0
                fi
                CC_x86_64_unknown_linux_musl=x86_64-linux-musl-gcc \
                AR_x86_64_unknown_linux_musl=x86_64-linux-musl-ar \
                cargo build --release --target "$triple"
                ;;
            aarch64-unknown-linux-musl)
                if ! command -v aarch64-linux-musl-gcc >/dev/null 2>&1; then
                    echo "    skipped: aarch64-linux-musl-gcc not on PATH (brew install FiloSottile/musl-cross/musl-cross --with-aarch64)"
                    return 0
                fi
                CC_aarch64_unknown_linux_musl=aarch64-linux-musl-gcc \
                AR_aarch64_unknown_linux_musl=aarch64-linux-musl-ar \
                cargo build --release --target "$triple"
                ;;
            *)
                cargo build --release --target "$triple"
                ;;
        esac
        out_dir="target/${triple}/release"
    fi

    local src="${out_dir}/${NAME}${suffix}"
    local dst="${DIST}/${NAME}-${tag}${suffix}"
    cp "$src" "$dst"
    chmod +x "$dst" 2>/dev/null || true
}

HOST=$(rustc -vV | sed -n 's|^host: ||p')

INSTALLED=""
if command -v rustup >/dev/null 2>&1; then
    INSTALLED=$(rustup target list --installed 2>/dev/null || true)
fi

# Host always.
build_one "$HOST"

# Every other installed target, best-effort.
while IFS= read -r t; do
    [[ -z "$t" || "$t" == "$HOST" ]] && continue
    build_one "$t" || echo "    failed: $t (continuing)"
done <<< "$INSTALLED"

echo
echo "==> $DIST/ contents:"
shopt -s nullglob
for f in "$DIST"/${NAME}-*; do
    base=${f##*/}
    sz=$(stat -f '%z' "$f" 2>/dev/null || stat -c '%s' "$f")
    sha=$(shasum -a 256 "$f" | awk '{print $1}')
    printf '  %-44s %10s bytes  %s\n' "$base" "$sz" "$sha"
done
