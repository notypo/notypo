#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

for tool in vhs ffmpeg gif2webp zsh; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "Missing $tool. Install with: brew install vhs ffmpeg webp zsh" >&2
        exit 1
    fi
done

cargo build --release
export PATH="$root/target/release:$PATH"

mkdir -p demo
vhs demo/notypo.tape

ffmpeg -y -v error -i demo/notypo-demo.mp4 \
    -vf "scale=1000:-2:flags=lanczos" \
    -c:v libsvtav1 -crf 32 -preset 7 -pix_fmt yuv420p -loop 0 -an \
    demo/notypo-demo.avif

ffmpeg -y -v error -i demo/notypo-demo.mp4 \
    -vf "fps=15,scale=900:-2:flags=lanczos,split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse" \
    -loop 0 demo/notypo-demo.gif

gif2webp -lossy -q 75 demo/notypo-demo.gif -o demo/notypo-demo.webp
rm -f demo/notypo-demo.mp4

printf 'Wrote demo/notypo-demo.avif, .webp, and .gif\n'
