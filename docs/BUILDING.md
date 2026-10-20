# Building SONORA

SONORA has three parts that build separately:

| Part | Where | Toolchain | Notes |
| --- | --- | --- | --- |
| Analysis engine, library core, CLI, dev bridge | `crates/` (root Cargo workspace) | Rust | Standard library only, builds offline. Links the operating system's SQLite. |
| Interface | `src/` | Node.js, Vite, React, TypeScript | |
| Desktop shell | `src-tauri/` (its own Cargo workspace) | Rust, Tauri 2 | Wraps the core, bundles SQLite and, in release builds, FFmpeg. |

Release bundles are built on the operating system they are for: macOS bundles on
a Mac, Windows installers on Windows. Cross-compiling is not supported; CI uses
one runner per platform (see [CI](#continuous-integration)).

## Prerequisites

All platforms:

- **Rust**, stable, **1.90 or newer** (Tauri 2.12 requires it). Install with
  [rustup](https://rustup.rs) and keep it current with `rustup update stable`.
- **Node.js 22.18 or newer** with npm (the unit tests rely on Node's built-in
  TypeScript support).

macOS (to develop, and to build macOS bundles):

- **Xcode Command Line Tools**: `xcode-select --install`.
- Optional for development: `brew install ffmpeg` (see [FFmpeg](#ffmpeg)).
- The app itself runs on macOS 13 or newer: the interface is built for the
  WebKit (Safari 16) that macOS 13 ships with.

Windows (to develop, and to build Windows installers):

- **Microsoft C++ Build Tools** (or Visual Studio 2022 or newer) with the
  **Desktop development with C++** workload, which includes the MSVC compiler
  and the Windows SDK. Use the default `x86_64-pc-windows-msvc` Rust toolchain.
- **Microsoft Edge WebView2 Runtime**. It is part of Windows 11 and of current
  Windows 10 installations; otherwise install the Evergreen runtime from
  Microsoft. The installers SONORA produces download it automatically when it
  is missing.
- Optional for development: `winget install Gyan.FFmpeg`.

Linux is not a release target, but the core, CLI, dev bridge and browser
interface work there. The core needs the SQLite library (`libsqlite3-dev` on
Debian/Ubuntu); running the desktop shell needs the
[Tauri Linux prerequisites](https://v2.tauri.app/start/prerequisites/)
(WebKitGTK 4.1).

## First steps

```sh
npm install          # interface and Tauri CLI dependencies
npm run test:rust    # cargo test --workspace (engine, core, JSON)
npm run test:unit    # interface unit tests
```

## Development

### Desktop app

```sh
npm run app:dev
```

This fetches FFmpeg into `src-tauri/binaries/` if it is not there yet, starts
Vite on <http://localhost:1420> and runs a debug build of the desktop shell
against it. The first build compiles Tauri and takes a few minutes; later
builds are incremental.

The library database and logs live in the app data folder:

| OS | Folder |
| --- | --- |
| macOS | `~/Library/Application Support/app.sonora.analyzer/` |
| Windows | `%LOCALAPPDATA%\app.sonora.analyzer\` (local, so it never roams with a profile) |
| Linux | `~/.local/share/app.sonora.analyzer/` |

It contains `sonora.db` and `logs/sonora.log` (JSON lines, rotated at 5 MB).

### Browser

```sh
npm run dev:web
```

Builds and starts the dev bridge (`crates/sonora-devserver`: the same core,
served over HTTP on `127.0.0.1:4317`) together with Vite on
<http://localhost:1420>, which proxies `/api` to it. Output from both is
prefixed; Ctrl+C stops both. The bridge keeps its library in
`.sonora-dev-data/`; pass another folder with `npm run dev:web -- --data <folder>`.

Native dialogs and drag-and-drop of files from Finder or Explorer need the
desktop app. Reveal in Finder/Explorer works in the browser too: the bridge
opens the file manager on the machine it runs on.

### Tests

```sh
npm run test:rust               # Rust tests; FFmpeg-based tests skip when FFmpeg is missing
npm run test:unit               # interface unit tests (node --test)
npm run typecheck
npx playwright install chromium # once
npm run build                   # the end-to-end tests run against dist/
npm run test:e2e                # Playwright, against the dev bridge and the real core
```

`npm run test:e2e` starts the bridge through `e2e/serve.mjs`, which builds it
in release mode, generates an audio corpus in `e2e/.data/music` when that
folder has no `manifest.json` yet, and starts each run from an empty library
in `e2e/.data/library`. The corpus has 39 files (36 tracks and 3 broken
files); 26 of them need FFmpeg, so without it only 13 are written and some
tests fail. After installing FFmpeg, delete `e2e/.data/music` to generate the
full corpus.

The bridge for the end-to-end tests keeps slskd's API key in a file in its
library folder instead of the system's keychain (`SONORA_SECRET_STORE=file`)
and looks at the Soulseek download folder every second
(`SONORA_SOURCE_POLL_SECONDS=1`; `0` looks only when asked, as the Rust
tests do). The Soulseek test talks to a stand-in for slskd
(`e2e/fakeSlskd.ts`); no test reaches Soulseek or a real slskd.

`npm run fixtures` writes the same corpus to `fixtures/library` with
45-second tracks, which is handy as a demo library: import that folder in
the app.

## FFmpeg

SONORA decodes WAV, AIFF and FLAC itself and runs FFmpeg as a separate program
for everything else. It uses the first FFmpeg it finds, in this order:

1. `SONORA_FFMPEG`, the full path to an `ffmpeg` program. Debug builds set it
   to `src-tauri/binaries/ffmpeg-<target-triple>` when that file exists and
   the variable is not already set.
2. The FFmpeg next to the SONORA executable. **Release builds** bundle a
   static one there: `scripts/fetch-ffmpeg.mjs` downloads it into
   `src-tauri/binaries/`, and the `src-tauri/tauri.bundle-ffmpeg.json` overlay
   adds it and its licence notice to the bundle. `npm run app:build` and CI
   use both.
3. `PATH`, then the usual Homebrew, MacPorts, winget, Scoop and Chocolatey
   locations (apps started from Finder do not inherit the shell's `PATH`).

```sh
node scripts/fetch-ffmpeg.mjs                  # this machine's target
node scripts/fetch-ffmpeg.mjs --target x86_64-apple-darwin
node scripts/fetch-ffmpeg.mjs --target universal-apple-darwin
node scripts/fetch-ffmpeg.mjs --from-system    # copy the installed FFmpeg (local builds only)
node scripts/fetch-ffmpeg.mjs --help
```

The script prints the FFmpeg version and SHA-256 of what it stored. Behind an
HTTP proxy, run it with `NODE_USE_ENV_PROXY=1`. Read
[`src-tauri/binaries/README.md`](../src-tauri/binaries/README.md) before
distributing builds: the default static builds are GPL-licensed, and it
explains your obligations and the LGPL alternative.

## Release builds

### For this machine

```sh
npm run app:build
```

This fetches FFmpeg for the host target and runs
`tauri build --config src-tauri/tauri.bundle-ffmpeg.json`, which builds the
interface (`npm run build`, including the type-check), compiles the shell in
release mode and bundles it.

### For a specific target

```sh
rustup target add <triple>
node scripts/fetch-ffmpeg.mjs --target <triple>
npx tauri build --target <triple> --config src-tauri/tauri.bundle-ffmpeg.json
```

| Platform | Target triple | Build on |
| --- | --- | --- |
| macOS, Apple silicon | `aarch64-apple-darwin` | any Mac |
| macOS, Intel | `x86_64-apple-darwin` | any Mac |
| macOS, universal | `universal-apple-darwin` | any Mac (needs both targets above) |
| Windows x64 | `x86_64-pc-windows-msvc` | Windows |
| Windows on Arm | `aarch64-pc-windows-msvc` | Windows (FFmpeg runs as x64 under emulation) |

Universal macOS build:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
node scripts/fetch-ffmpeg.mjs --target universal-apple-darwin
npx tauri build --target universal-apple-darwin --config src-tauri/tauri.bundle-ffmpeg.json
```

A plain `npx tauri build` bundles no FFmpeg. The result is smaller, but
formats other than WAV, AIFF and FLAC then need an FFmpeg installed on the
machine.

### Output

Bundles land in `src-tauri/target/release/bundle/`, or
`src-tauri/target/<triple>/release/bundle/` with `--target`:

| Platform | Files |
| --- | --- |
| macOS | `macos/SONORA.app`, `dmg/SONORA_<version>_<arch>.dmg` (`aarch64`, `x64` or `universal`) |
| Windows | `msi/SONORA_<version>_x64_en-US.msi`, `nsis/SONORA_<version>_x64-setup.exe` |

The NSIS installer installs for the current user without administrator rights;
the MSI is for managed deployment. Both install WebView2 when it is missing.

## Signing and notarization

Signing is optional. Unsigned builds work, but macOS Gatekeeper and Windows
SmartScreen warn before the first launch (see
[Troubleshooting](#troubleshooting)).

### macOS: Developer ID and notarization

You need an Apple Developer Program membership and a **Developer ID
Application** certificate. Tauri signs the app, the FFmpeg sidecar and the DMG
with the hardened runtime (enabled in `tauri.conf.json`), then submits the app
to Apple's notary service and staples the ticket. It reads these environment
variables:

| Variable | Purpose |
| --- | --- |
| `APPLE_SIGNING_IDENTITY` | Certificate name, e.g. `Developer ID Application: Your Name (TEAMID)`. Enough on its own when the certificate is in your login keychain. |
| `APPLE_CERTIFICATE` | Base64 of the exported `.p12` (`openssl base64 -A -in cert.p12`), imported into a temporary keychain. For CI. |
| `APPLE_CERTIFICATE_PASSWORD` | Password of that `.p12`. |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | Notarize with an Apple ID and an app-specific password (created at account.apple.com). |
| `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_PATH` | Or notarize with an App Store Connect API key (key ID, issuer ID, path to the `.p8` file). |

Locally:

```sh
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export APPLE_ID="you@example.com" APPLE_PASSWORD="app-specific-password" APPLE_TEAM_ID="TEAMID"
npm run app:build
```

Verify the result with `codesign --verify --deep --strict --verbose=2 SONORA.app`
and `spctl --assess --type execute --verbose SONORA.app`.

### Windows: Authenticode

Tauri signs the executable, the FFmpeg sidecar (unless it is already signed)
and both installers with `signtool`. Configure one of:

- a certificate installed in the Windows certificate store, referenced by
  thumbprint: `bundle.windows.certificateThumbprint`, with `digestAlgorithm`
  `sha256` and a `timestampUrl` such as `http://timestamp.digicert.com`;
- `bundle.windows.signCommand`, a command that signs one file (`%1`), for
  certificates kept in a hardware token or a cloud signing service such as
  Azure Trusted Signing.

Pass these in an extra config file or inline JSON, e.g.
`npx tauri build --config src-tauri/tauri.bundle-ffmpeg.json --config '{"bundle":{"windows":{"certificateThumbprint":"…","digestAlgorithm":"sha256","timestampUrl":"http://timestamp.digicert.com"}}}'`.

### Signing in CI

The `desktop` job signs only when these repository secrets exist; without
them it produces unsigned Windows installers and ad-hoc-signed macOS bundles.
Without a macOS certificate it also skips notarization, which Apple refuses
for ad-hoc signatures, even if notarization secrets are set.

| Secret | Used for |
| --- | --- |
| `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD` | Developer ID signing |
| `APPLE_SIGNING_IDENTITY` | Optional; inferred from the certificate when unset |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | Notarization with an Apple ID |
| `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_PRIVATE_KEY` | Or notarization with an API key; `APPLE_API_PRIVATE_KEY` holds the contents of the `.p8` file |
| `WINDOWS_CERTIFICATE`, `WINDOWS_CERTIFICATE_PASSWORD` | Base64 of a `.pfx` and its password, imported into the runner's certificate store |

## Continuous integration

`.github/workflows/ci.yml` runs on pushes to `main`, version tags (`v*`), pull
requests and on demand:

| Job | What it does |
| --- | --- |
| `rust` | `cargo test --workspace --locked` on Ubuntu, macOS and Windows (FFmpeg from apt and Homebrew; Windows deliberately without FFmpeg), plus `cargo clippy -D warnings` on Ubuntu; on macOS and Windows a round trip through the real keychain or Credential Manager, and on Windows the Recycle Bin safety test |
| `frontend` | Type-check, unit tests and production build of the interface |
| `e2e` | Builds the interface and runs the Playwright tests against the dev bridge (which generates its audio corpus); uploads the report on failure |
| `desktop` | After `rust` and `frontend` pass: bundles for macOS Apple silicon (`macos-15`), macOS Intel (`macos-15-intel`) and Windows x64, each with its FFmpeg; uploads the `.dmg`, the `.app` (as `.tar.gz`), the `.msi` and the NSIS `.exe` as workflow artifacts |

No `package-lock.json` or `src-tauri/Cargo.lock` is committed yet, so CI
resolves dependencies fresh (`npm install`) within the pinned minor versions.
Commit both lockfiles and switch the jobs to `npm ci` with setup-node's
`cache: npm` for reproducible builds.

## Troubleshooting

**macOS: "SONORA can't be opened" / "Apple could not verify…"** — the build is
not signed with a Developer ID and notarized. For your own builds, open
**System Settings → Privacy & Security** and choose **Open Anyway** after the
first attempt, or remove the quarantine flag:
`xattr -dr com.apple.quarantine /Applications/SONORA.app`. Distributed builds
should be signed and notarized.

**Windows: "Windows protected your PC"** — SmartScreen does not recognise an
unsigned (or newly signed) installer. Choose **More info → Run anyway**, or sign
the installers.

**Formats other than WAV, AIFF and FLAC fail with "FFmpeg is required…"** —
SONORA found no FFmpeg. Release builds made with `npm run app:build` or CI
include one. Otherwise install FFmpeg (`brew install ffmpeg`,
`winget install Gyan.FFmpeg`) and restart SONORA; when starting SONORA from a
terminal you can also point `SONORA_FFMPEG` at an `ffmpeg` program.

**Build error about `binaries/ffmpeg-<triple>` or `binaries/ffmpeg-LICENSE.txt`**
— the FFmpeg overlay is in use but nothing was fetched for that target. Run
`node scripts/fetch-ffmpeg.mjs --target <triple>` with the same triple as
`tauri build`, or build without `--config src-tauri/tauri.bundle-ffmpeg.json`.

**"Found version mismatched Tauri packages"** — the `@tauri-apps/*` npm
packages and the Tauri Rust crates must share major and minor versions. Both
are pinned to a minor version with `~` (in `package.json` and
`src-tauri/Cargo.toml`); to upgrade, raise them together, then run
`npm install` and `cargo update` in `src-tauri/`.

**Windows: WebView2 missing on an offline machine** — the installers download
WebView2 when needed. For offline installs, deploy Microsoft's Evergreen
Standalone WebView2 installer first.

**Windows: `link.exe` not found, or `cannot open input file 'kernel32.lib'`** —
install the C++ build tools with the Windows SDK (see
[Prerequisites](#prerequisites)).

**Linux: `cannot find -lsqlite3`** — install the SQLite development package
(`sudo apt install libsqlite3-dev`).

**`npm run dev:web` exits with "could not listen on 127.0.0.1:4317"** — another
bridge is still running. Stop it, or end the process holding the port.

**Anything else** — the app logs to `logs/sonora.log` in its data folder (see
[Desktop app](#desktop-app)) and to the terminal when started with
`npm run app:dev`.
