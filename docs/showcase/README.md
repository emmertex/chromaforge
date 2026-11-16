# README showcase

The README screenshots in `docs/images/classic/` are rendered headless (no window) from the procedural demo
library by [`classic.jsonl`](classic.jsonl), a list of control-protocol requests:

```sh
# (<PREFIX>_FACE_MODELS at an empty folder keeps the face scan, and its activity row, out of the shots)
<PREFIX>_FACE_MODELS=$(mktemp -d) cargo run --release -p lcc-cli -- snapshot --demo --script docs/showcase/classic.jsonl --size 1600x1000
# the four scope modes, cropped from the Develop panel at 2x into one 2x2 image
cd docs/images/classic && for mh in rgb:123 parade:163 waveform:163 vectorscope:209; do m=${mh%:*}; h=${mh#*:}
  magick scope-$m.png -crop 268x${h}+1332+93 +repage -background black -gravity center -extent 268x209 -resize 200% c-$m.png; done
magick \( c-rgb.png c-parade.png +append \) \( c-waveform.png c-vectorscope.png +append \) -append scopes.png
rm c-*.png scope-*.png && cd -
# compress for the repo (ImageMagick): 1280 px wide JPEGs, well under 300 KB each
for f in docs/images/classic/*.png; do magick "$f" -resize "1280x>" -strip -sampling-factor 4:2:0 -quality 82 "${f%.png}.jpg" && rm "$f"; done
```

`demo.jsonl` and `pd-edits.jsonl` are the upstream project's scripts for its own (non-Classic) layout; they drive a
running app over the control channel with `run.py`, which sends each JSON line (`{"method":…,"params":…}` or
`{"sleep": seconds}`) to `127.0.0.1:7980`. Their output is not kept in this repository.
