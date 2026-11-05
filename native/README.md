Unicorn 2.1.x is loaded dynamically. No Python runtime is used.

Windows x64: run `scripts/setup-windows.ps1`, or put `unicorn.dll` beside the executable.
The setup script extracts the official Unicorn 2.1.4 PyPI wheel's native DLL, checking the archive SHA-256 first.
Linux: install Unicorn 2.1, or set `DSMC_UNICORN_LIBRARY` to the absolute library path.

Upstream: https://github.com/unicorn-engine/unicorn (GPL-2.0; API headers/bindings LGPL-2.1).
If distributing the DLL, include the corresponding license and source-offer obligations. The upstream source for the pinned binary is https://github.com/unicorn-engine/unicorn/tree/2.1.4.
The proprietary RED firmware and package assets are never downloaded or included.
