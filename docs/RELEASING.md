# Releasing and updates

A release is a GitHub release of this repository with installers for macOS
(Apple silicon and Intel) and Windows x64, plus `latest.json`, which running
copies of SONORA read to learn about it. Pushing a tag `v<version>` makes
one; the [Release workflow](../.github/workflows/release.yml) does the rest.

## Once: the signing key

Updates are signed. The desktop app installs only packages signed with the
private key that belongs to the public key in
[`src-tauri/tauri.conf.json`](../src-tauri/tauri.conf.json)
(`plugins > updater > pubkey`, key id `4D9B2CDE36854BC2`).

1. On GitHub, open the repository's **Settings → Secrets and variables →
   Actions** and add a repository secret named `TAURI_SIGNING_PRIVATE_KEY`
   whose value is the whole content of the private key file (one long line
   of base64; a line break pasted with it is ignored).
2. The key has no password. If you ever replace it with one that has, also
   add `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
3. Keep a copy of the private key somewhere safe and private (a password
   manager). It never goes into the repository.

Losing the key, or publishing it, has consequences: installed copies trust
only that key. With a new key pair, the new public key has to go into
`tauri.conf.json`, and copies installed before then must be updated once by
hand (download and install the new version).

Without the secret, the Release workflow stops at its first step and says
so; nothing is published.

## Each release

1. Set the new version in all three places (they must agree):
   - `package.json` → `version`
   - `src-tauri/Cargo.toml` → `[package] version`
   - `src-tauri/tauri.conf.json` → `version`
2. Add a section `## <version>` to [CHANGELOG.md](../CHANGELOG.md). It
   becomes the release notes on GitHub and in the app's update dialog, so
   write it for the people using SONORA.
3. Commit, push to `main` and wait for CI to pass.
4. Tag and push the tag:

   ```sh
   git tag v0.2.0
   git push origin v0.2.0
   ```

   Or from the GitHub website: **Releases → Draft a new release**, type the
   tag (`v0.2.0`) under *Choose a tag* and pick *Create new tag on
   publish*, with `main` as the target, then **Publish release**. Publishing
   creates the tag, which starts the same workflow; it takes the release back
   to a draft until the installers are in place, and fills in the title and
   notes from CHANGELOG.md if they were left empty.

The workflow then:

1. checks that the tag matches the three versions and that CHANGELOG.md has
   a section for it, and opens a draft release;
2. builds the three installers with FFmpeg, signs the update packages
   (`.app.tar.gz` for macOS, the NSIS `.exe` and the `.msi` for Windows) and
   adds them, their `.sig` files and the macOS `.dmg` files to the draft;
3. writes `latest.json` from the signatures, adds it, and publishes the
   release as the latest one.

Running copies see the update only after step 3, when every file
`latest.json` names is in place. A failed run leaves a draft that nobody
sees. Fix the cause, then run the failed jobs again; when the fix is in the
workflow itself, use **Actions → Release → Run workflow** with the tag
instead, which builds the tag with the workflow as it is on `main`. Or
delete the draft and the tag and start over.

`node scripts/release.ts check v<version>` and
`node scripts/release.ts notes <version>` run the workflow's checks locally.

## How a running copy updates

- **Looking**: on launch (after a few seconds) and every six hours, SONORA
  fetches `https://github.com/dillinger445-alt/sonora/releases/latest/download/latest.json`.
  The request carries no information about the user or their music; its user
  agent names the updater plugin and its version. Settings → Updates turns
  automatic checks off; **Check now** works either way.
- **Offering**: a newer version shows in the status bar and, once per
  version, as a notification. The dialog shows its release notes. Nothing is
  downloaded until the user chooses **Install and restart**.
- **Installing**: the package for the platform is downloaded and its
  signature checked against the public key. SONORA stops its background work
  (analysis, exports, lookups), then:
  - **Windows**: the installer runs in passive mode (a progress window, no
    questions), replaces the installed version, and starts the new one. An
    installation from the `.exe` updates per user, without an administrator
    prompt; one from the `.msi` updates through the MSI and asks for
    administrator rights, as installing it did.
  - **macOS**: the app bundle is replaced in place and SONORA restarts.
- **What stays**: the library database (tracks, results, history, settings,
  edits, the Discogs token), logs and caches live in the app data folder,
  which updates do not touch, and the window's own preferences live in the
  web view's storage, which is kept too. See the folder for each system in
  [BUILDING.md](BUILDING.md#development).

Copies older than 0.2.0 have no updater: install 0.2.0 once by hand, and
later versions arrive in the app.
