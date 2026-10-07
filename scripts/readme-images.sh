#!/bin/sh
# Renders the README screenshots with the real app, offscreen, and writes them to docs/images/.
set -e
cd "$(dirname "$0")/.."
cargo test --release -p ferrender readme_ -- --ignored
mkdir -p docs/images
for png in target/demo/*.png; do
    name=$(basename "$png" .png)
    if command -v sips >/dev/null; then
        sips -Z 2000 -s format jpeg -s formatOptions 82 "$png" --out "docs/images/$name.jpg" >/dev/null
    else
        magick "$png" -resize 2000x -quality 82 "docs/images/$name.jpg"
    fi
done
ls -lh docs/images
