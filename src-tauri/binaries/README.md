# src-tauri/binaries

This folder holds the FFmpeg program that release builds of SONORA ship with.
Everything in it except this README is generated and ignored by git.

SONORA decodes WAV, AIFF and FLAC itself. For every other format (MP3, AAC/M4A,
ALAC, Ogg Vorbis, Opus, WMA, WavPack, APE, CAF…) it runs FFmpeg as a separate
program and reads the decoded audio from a pipe.

## Contents

| File | Purpose |
| --- | --- |
| `ffmpeg-<target-triple>[.exe]` | The program, named the way Tauri's `bundle.externalBin` expects, e.g. `ffmpeg-aarch64-apple-darwin`, `ffmpeg-x86_64-pc-windows-msvc.exe`. |
| `ffmpeg-<target-triple>[.exe].LICENSE` | The licence published with that build. |
| `ffmpeg-LICENSE.txt` | Notice bundled into the app as `licenses/FFmpeg.txt`: origin, version, SHA-256, licence and build notes of the binaries. |

A universal macOS build needs three binaries: `ffmpeg-aarch64-apple-darwin`,
`ffmpeg-x86_64-apple-darwin` (Tauri compiles each architecture separately) and
`ffmpeg-universal-apple-darwin` (merged with `lipo`, used by the bundler).

## Filling it

```sh
node scripts/fetch-ffmpeg.mjs                     # this machine's target
node scripts/fetch-ffmpeg.mjs --target universal-apple-darwin
node scripts/fetch-ffmpeg.mjs --from-system       # copy an installed FFmpeg (local builds only)
node scripts/fetch-ffmpeg.mjs --help
```

`npm run app:build` runs the script and then
`tauri build --config src-tauri/tauri.bundle-ffmpeg.json`. That overlay adds
`externalBin` and the licence notice to the bundle. Tauri copies the binary next
to the SONORA executable under the plain name `ffmpeg` (`SONORA.app/Contents/MacOS/ffmpeg`
on macOS, `ffmpeg.exe` in the install folder on Windows), where SONORA finds it.

A build without the overlay bundles no FFmpeg. SONORA then uses an FFmpeg it
finds on the machine (`SONORA_FFMPEG`, `PATH`, Homebrew, MacPorts, winget,
Scoop, Chocolatey); without one, files that need it fail with a message saying
FFmpeg is required. Debug builds (`npm run app:dev`) use the binary in this
folder when it exists.

## Where the downloads come from

By default the script downloads the static builds published as GitHub release
assets of [eugeneware/ffmpeg-static](https://github.com/eugeneware/ffmpeg-static)
(release `b6.1.1`; override with `--tag`). That project repackages builds from:

- macOS x64: [evermeet.cx](https://evermeet.cx/ffmpeg/)
- macOS arm64: [osxexperts.net](https://www.osxexperts.net/)
- Windows x64: [gyan.dev](https://www.gyan.dev/ffmpeg/builds/) (essentials build)
- Linux: [johnvansickle.com](https://johnvansickle.com/ffmpeg/)

The FFmpeg version inside differs by platform; the script prints it together with
the file's SHA-256, and both are recorded in `ffmpeg-LICENSE.txt`.

## Licensing

FFmpeg's own code is licensed under the LGPL 2.1 or later, with optional GPL
parts. The static builds above enable GPL components (such as libx264), so the
binaries as a whole are distributed under the GPL (version 3 for the Windows
and Linux builds; see each `.LICENSE` file).

SONORA does not link FFmpeg; it starts it as a separate program and exchanges
data through pipes. Shipping the two together does not change SONORA's own
licence, but whoever distributes a SONORA bundle containing FFmpeg is
distributing FFmpeg and must meet its licence terms:

- include the licence text — done automatically: release bundles contain
  `licenses/FFmpeg.txt`;
- make the corresponding source code available for the exact build shipped
  (FFmpeg itself and the external libraries listed in the build notes). The
  providers above link their sources; FFmpeg's source is at
  <https://ffmpeg.org/download.html>. Keeping a copy of those sources alongside
  each release is the simplest way to honour this.

SONORA only needs FFmpeg's decoders, which are all LGPL. To avoid the GPL
entirely, place an LGPL-only build here instead, using the naming scheme
above: for example the `lgpl` variants of
[BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds) (Windows, Linux),
or a static FFmpeg you build yourself without `--enable-gpl` and
`--enable-nonfree`.
LGPL builds still require the licence text and source availability, but impose
fewer conditions.

This summary is not legal advice; check the licences that apply to the build you
ship before distributing it.
