"""Export a trained checkpoint as the upstream project's AI Denoise model: `model.onnx` + `denoise-model.json`, then run
the app's own installation self-test on them.

Offline training tooling; not part of the app.

    python export.py --ckpt <run>/final.pt --out <folder> --rawdump <lc-denoise-rawdump> [--version 0.1.0]
"""

import argparse
import csv
import hashlib
import json
import os
import subprocess
import sys

import torch

import model as M

TILE, OVERLAP = 512, 64


def _upstream():
    """`[upstream]` from the repository's brand.toml: the project this model was made by."""
    import tomllib
    with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "brand.toml"), "rb") as f:
        return tomllib.load(f)["upstream"]


UP = _upstream()

NOTICE = (
    "{up}'s own denoise model, trained by the {up} contributors. The weights are licensed MIT OR "
    "Apache-2.0, like {up}: you may use them for any purpose, including commercial work, and pass them on with "
    "this notice. Training pictures: {n} low-ISO raw samples from raw.pixls.us, each dedicated to the public domain "
    "(CC0) by its contributor; thank you."
)

PROVENANCE = (
    "Trained by {up} contributors with {up}'s open training tools (tools/denoise-train) on {n} clean "
    "low-ISO raw photos from {cams} camera models published on raw.pixls.us under CC0, with synthetic sensor noise "
    "calibrated on real cameras (numbers only). No photos with other licences were used."
)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ckpt", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--rawdump", required=True)
    ap.add_argument("--data", help="the data folder, for the credits in the manifest")
    ap.add_argument("--version", default="0.1.0")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    s = torch.load(a.ckpt, map_location="cpu")
    net = M.BayerUNet(tuple(s.get("widths", (32, 64, 128, 256, 512))))
    net.load_state_dict(s["ema"])
    onnx_path = os.path.join(a.out, "model.onnx")
    M.export_onnx(net, onnx_path, TILE)
    data = open(onnx_path, "rb").read()

    n, cams = 0, 0
    if a.data and os.path.exists(os.path.join(a.data, "sources.csv")):
        rows = list(csv.DictReader(open(os.path.join(a.data, "sources.csv"), encoding="utf8")))
        assert all(r["licence"] == "CC0-1.0" for r in rows)
        n, cams = len(rows), len({(r["make"], r["model"]) for r in rows})

    manifest = {
        "id": UP["name"].lower() + "-bayer",
        "name": UP["name"] + " Denoise (Bayer)",
        "version": a.version,
        "licence": {
            "name": "MIT OR Apache-2.0",
            "commercial": "yes",
            "url": UP["repository"] + "/blob/main/LICENSE-MIT",
            "notice": NOTICE.format(n=n, up=UP["name"]),
        },
        "source": UP["repository"] + "/tree/main/tools/denoise-train",
        "sha256": hashlib.sha256(data).hexdigest(),
        "sizeBytes": len(data),
        "provenance": PROVENANCE.format(n=n, cams=cams, up=UP["name"]),
        "domain": "bayerToRgb",
        "tile": TILE,
        "overlap": OVERLAP,
        "gain": {"kind": "none"},
    }
    man_path = os.path.join(a.out, "denoise-model.json")
    json.dump(manifest, open(man_path, "w", encoding="utf8"), indent=2)
    print(json.dumps({k: manifest[k] for k in ("id", "version", "sha256", "sizeBytes")}))
    p = subprocess.run([a.rawdump, "selftest", onnx_path, man_path], capture_output=True, text=True)
    print(p.stdout.strip(), p.stderr.strip())
    return p.returncode


if __name__ == "__main__":
    sys.exit(main())
