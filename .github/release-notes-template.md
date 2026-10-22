## Download

Pick your computer from the **Assets** list below.

| Your computer | Download this file | Status |
|---|---|---|
| Windows 10 or 11 (64-bit) | `plugable-chat_<version>_x64-setup.exe` | Tested: signed by LEANCODE, INC.; install, update, repair and uninstall checked |
| Windows, managed by IT | `plugable-chat_<version>_x64_en-US.msi` | Tested, same checks as the setup file |
| Linux (any distribution) | `plugable-chat_<version>_amd64.AppImage` | Built and signed in CI; not yet run-tested |
| Linux (Debian/Ubuntu or Fedora/RHEL) | `.deb` or `.rpm` | Built and signed in CI; not yet run-tested |
| macOS (Apple Silicon, M1 or later) | `plugable-chat_<version>_aarch64.dmg` | Built in CI and signed (ad hoc). **rc15's Mac file did not start; fixed in rc16**, and CI now launches the app and checks that its AI engine loads. A full chat on a Mac has been checked by hand only on one machine. Until Apple notarization is set up, macOS asks you to confirm the first open (right-click, *Open*) |

**Windows needs an NVIDIA RTX graphics card of the Ampere generation or newer** (RTX 30-series or later). Older cards such as
the Tesla T4 or RTX 20-series are not supported. Plan for about 6 GB of free disk space and an internet connection the first
time you start the app.

**Check your download.** On Windows, right-click the file, choose *Properties*, then the *Digital Signatures* tab. The signer
should read *LEANCODE, INC.* with a timestamp. Checksum files are in the Assets list (`SHA256SUMS-windows.txt`, `SHA256SUMS-macos.txt`, `SHA256SUMS`).

**Which build is this?** The title bar and *Settings* (bottom of the window) show the release and the exact commit, for example `v0.1.0-rc12 (a1b2c3d)`. The installer file name carries the same release. Please include it when you report a problem; *Copy details for support* adds it for you.

**Windows "Installed apps" shows `0.1.0` for every release candidate.** Windows only accepts numbers in an installer's version, so `-rcN` cannot appear there. The title bar and *Settings* carry the full release and commit; use those to tell builds apart.

**Please do not set `PLUGABLE_FOUNDRY_BACKEND`.** It was a test switch for an older build. If it is set and the `foundry` command-line tool is not installed, the app now uses its built-in engine instead.

**How to use the app:** see the [user guide](https://github.com/PlugableTechnologies/plugable-chat/blob/main/docs/user-guide.md).

## What changed

