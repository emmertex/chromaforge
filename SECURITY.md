# Security Policy

The app reads camera raw files, image formats, metadata and sidecars, imports other photo catalogs, downloads and runs machine-learning models, and exposes CLI, JSON control, and MCP automation. Treat files, paths, metadata, catalogs, model files, automation requests, and command parameters as potentially malicious.

## Supported versions

Only the latest release and `main` receive security fixes. Older releases and tags are not supported; please update before reporting.

## Reporting a vulnerability

**Do not open a public issue for a security vulnerability.**

Use GitHub's **private vulnerability reporting** for this repository (*Security → Report a vulnerability*) — it reaches the maintainers directly and privately. If that is unavailable, contact the repository maintainers through the repository owner/maintainer channels already listed in the project, provide only a minimal non-exploitable summary in public, and ask for an appropriate private transfer method before sharing sensitive details.

We aim to acknowledge a report within a week. This is a volunteer-maintained project, so complex fixes can take longer; we will keep you informed, practise coordinated disclosure (the fix lands first, the advisory follows), and credit you in the fix unless you prefer to stay anonymous.

Please give the repository maintainers a reasonable opportunity to investigate and coordinate a fix before publishing exploitable details, proof-of-concept code, or weaponized files.

## What to include

A useful report includes:

- affected version of the app, commit, or release artifact;
- operating system, architecture, build profile (desktop or web), and relevant feature flags;
- the affected component or path;
- security impact and required attacker access;
- exact reproduction steps;
- expected behavior and actual behavior;
- crash logs, backtraces, sanitizer output, or resource measurements when applicable;
- whether the problem reproduces from a clean checkout;
- any known workaround or containment.

Remove credentials, personal paths, private photos, and unrelated system information from logs.

## Malicious test files

Do not attach a potentially harmful or non-redistributable file to a public issue. Initially provide its format, size, cryptographic hash, observed effect, provenance category (for example, synthetic or fuzz-generated), and the command needed to reproduce the problem. Coordinate a private transfer method with maintainers before sending the sample.

Test files accepted into the repository must be minimized, free of personal or proprietary data, legally redistributable, and documented with the expected failure behavior. A fixed parser defect should receive a permanent regression test whenever safe and practical.

## Scope

Security reports may cover:

- camera raw, DNG, TIFF, HEIF, JPEG and other image decoding, and EXIF/XMP metadata and sidecar parsing;
- decompression bombs, oversized dimensions, integer overflow, memory exhaustion, recursion, or hangs;
- the app's catalog, and importing other catalogs and libraries (such as Lightroom catalogs);
- import/export and filesystem path handling, including traversal, symlinks, and unsafe overwrite or move behavior;
- downloading, verifying, unpacking and running machine-learning models (denoise, faces, masks);
- engine command validation and serialization/deserialization;
- the desktop control channel, the MCP server, and the MCP-to-app connection;
- the WebAssembly build;
- GPU resource validation or driver-facing input handling;
- dependencies, CI, packaging, signing, checksums, and release artifact integrity.

General bugs without a security impact should use the project's normal issue and contribution process.

## Current limitations

The control channel (`--control PORT`) listens on loopback only and accepts a connection only after it sends the token the app writes to its settings folder ([control-protocol.md](docs/control-protocol.md) → Authentication); still, only enable it when you need it.
