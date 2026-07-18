#!/usr/bin/env bash
# Rebuild assets/system.ttf from pinned upstream JetBrains Mono releases.
#
# The Nerd Fonts archive supplies Powerline outlines, while the unpatched
# JetBrains Mono archive defines the baseline Unicode coverage we retain. The
# result is the original face's coverage plus U+E0A0-U+E0B3, without embedding
# the thousands of unrelated Nerd Font icons in the kernel.

set -euo pipefail

NERD_FONTS_VERSION=3.4.0
JETBRAINS_MONO_VERSION=2.304
FONTTOOLS_VERSION=4.59.0

NERD_FONTS_URL="https://github.com/ryanoasis/nerd-fonts/releases/download/v${NERD_FONTS_VERSION}/JetBrainsMono.zip"
NERD_FONTS_SHA256=76f05ff3ace48a464a6ca57977998784ff7bdbb65a6d915d7e401cd3927c493c
JETBRAINS_MONO_URL="https://github.com/JetBrains/JetBrainsMono/releases/download/v${JETBRAINS_MONO_VERSION}/JetBrainsMono-${JETBRAINS_MONO_VERSION}.zip"
JETBRAINS_MONO_SHA256=6f6376c6ed2960ea8a963cd7387ec9d76e3f629125bc33d1fdcd7eb7012f7bbf

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUTPUT="${1:-$REPO_ROOT/assets/system.ttf}"
FONT_TMP="$(mktemp -d)"
trap 'rm -rf "$FONT_TMP"' EXIT

for command_name in curl shasum unzip uv; do
    if ! command -v "$command_name" >/dev/null 2>&1; then
        echo "Missing required command: $command_name" >&2
        exit 1
    fi
done

download_and_verify() {
    local url=$1 output=$2 expected=$3
    curl -fsSL --max-time 120 -o "$output" "$url"
    local actual
    actual=$(shasum -a 256 "$output" | awk '{print $1}')
    if [ "$actual" != "$expected" ]; then
        echo "SHA256 mismatch for $url" >&2
        echo "expected: $expected" >&2
        echo "actual:   $actual" >&2
        exit 1
    fi
}

download_and_verify "$NERD_FONTS_URL" "$FONT_TMP/nerd-fonts.zip" "$NERD_FONTS_SHA256"
download_and_verify "$JETBRAINS_MONO_URL" "$FONT_TMP/jetbrains-mono.zip" "$JETBRAINS_MONO_SHA256"

unzip -q "$FONT_TMP/nerd-fonts.zip" JetBrainsMonoNerdFontMono-Regular.ttf -d "$FONT_TMP/nerd"
unzip -q "$FONT_TMP/jetbrains-mono.zip" fonts/ttf/JetBrainsMono-Regular.ttf -d "$FONT_TMP/base"

# Record the base face's cmap and union in the Powerline range. Keeping this
# derivation in the script makes future refreshes independent of the already
# generated assets/system.ttf.
uv run --quiet --with "fonttools==$FONTTOOLS_VERSION" python - \
    "$FONT_TMP/base/fonts/ttf/JetBrainsMono-Regular.ttf" \
    "$FONT_TMP/unicodes.txt" <<'PY'
from fontTools.ttLib import TTFont
import sys

font = TTFont(sys.argv[1])
codepoints = set(font.getBestCmap()) | set(range(0xE0A0, 0xE0B4))
with open(sys.argv[2], "w", encoding="ascii") as output:
    output.write(",".join(f"U+{codepoint:04X}" for codepoint in sorted(codepoints)))
PY

uvx --quiet --from "fonttools==$FONTTOOLS_VERSION" pyftsubset \
    "$FONT_TMP/nerd/JetBrainsMonoNerdFontMono-Regular.ttf" \
    --output-file="$FONT_TMP/system.ttf" \
    --unicodes-file="$FONT_TMP/unicodes.txt" \
    --layout-features='*' \
    --glyph-names \
    --symbol-cmap \
    --legacy-cmap \
    --notdef-glyph \
    --notdef-outline \
    --recommended-glyphs \
    --name-IDs='*' \
    --name-legacy \
    --name-languages='*'

mkdir -p "$(dirname "$OUTPUT")"
cp "$FONT_TMP/system.ttf" "$OUTPUT.tmp.$$"
mv -f "$OUTPUT.tmp.$$" "$OUTPUT"
echo "Wrote $OUTPUT ($(wc -c < "$OUTPUT" | tr -d ' ') bytes)"
