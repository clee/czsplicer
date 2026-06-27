#!/usr/bin/env bash
# Download the top N Adium message styles from adiumxtras.com and extract
# their CSS/template layout patterns for analysis.
#
# Usage: ./scripts/scan_adium_themes.sh [N]   (default N=40)
set -euo pipefail

N="${1:-40}"
OUT="${2:-/tmp/adium_themes}"
mkdir -p "$OUT"
echo "Downloading top $N message styles to $OUT ..."

# (xtra_id, name) pairs scraped from adiumxtras.com ranked message-style list.
# IDs are stable; names are for human reference. Order ≈ by ranking score.
THEMES=(
    "2160:Renkoo"
    "198:Pushpin"
    "7780:Mnmlsm"
    "3629:ModernBubbling"
    "7997:Taz"
    "8063:Lighty"
    "6058:StickerStyle"
    "8259:iMessages"
    "1385:Candybars"
    "6766:Ravenant"
    "2141:Ethereal"
    "4745:Succinct"
    "4987:AdiumMatte"
    "345:aNon"
    "1907:h4x0r"
    "7014:Terminal"
    "4430:iPhone"
    "7960:Fluffy"
    "2463:GoneDark"
    "1598:yaz"
    "8317:Mockie"
    "6134:SimPixelPro"
    "5855:Smoke"
    "1962:boxer"
    "1527:Buuf"
    "2835:Aluminum"
    "7067:Flight"
    "5766:Disco"
    "6562:minimal"
    "1183:smooth"
    "2101:Renkoo_Naked"
    "4466:Prosope"
    "5602:macosiChat"
    "4522:macOSiChat"
    "6938:PrettySimple"
    "3665:Bash"
    "8095:Pretty"
    "7394:ElegantSimple"
    "8241: Nachrichten"
    "7741:refined"
)

count=0
for entry in "${THEMES[@]}"; do
    id="${entry%%:*}"
    name="${entry##*:}"
    count=$((count+1))
    [ "$count" -gt "$N" ] && break
    dst="$OUT/$name.AdiumMessageStyle"
    if [ -d "$dst" ]; then
        echo "  [$count/$N] $name (cached)"
        continue
    fi
    # adiumxtras serves .adiumessagestyle as a zip; download then unzip in place.
    url="https://www.adiumxtras.com/download/$id"
    zip="/tmp/adium_$id.zip"
    if curl -fsSL "$url" -o "$zip" 2>/dev/null; then
        # The zip may extract to either a .AdiumMessageStyle dir or its parent.
        (cd "$OUT" && unzip -q -o "$zip") || {
            echo "  [$count/$N] $name: unzip FAILED"
            continue
        }
        # Normalize: find the extracted .AdiumMessageStyle and rename to $name.
        found=$(find "$OUT" -maxdepth 2 -name "*.AdiumMessageStyle" -type d | head -1)
        if [ -n "$found" ] && [ "$(basename "$found")" != "$name.AdiumMessageStyle" ]; then
            mv "$found" "$dst"
        fi
        echo "  [$count/$N] $name: OK"
    else
        echo "  [$count/$N] $name: download FAILED ($url)"
    fi
    rm -f "$zip"
    # Be polite.
    sleep 0.4
done

echo ""
echo "Extracted bundles:"
ls -1 "$OUT" | head -50
