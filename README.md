# mmvp

**English** | [繁體中文](README_zh-tw.md)

Mabinogi Mobile Voice Pack (for TW): replaces the Korean voice-over of the Taiwanese *Mabinogi Mobile* client with the Japanese one.

- Only voices are replaced. No original game file is modified, and you can restore at any time.
- The first download is about 100 MB; after that only changed voices are downloaded.
- You are prompted to re-apply after a game update or a voice update. The program updates itself.

The interface is in English or Traditional Chinese, following the system language.

## Install

1. Download `mmvp.exe` from [Releases](https://github.com/miaulightouch/mmvp/releases/latest), put it in a writable folder and run it (no installer).
2. Close the game, check the game folder, then press **Apply JP voices**.
3. To go back to the original voices, press **Restore original voices**.

### Clients of other regions (experimental)

mmvp finds the Taiwanese client installed by the Nexon Launcher on its own. A client of another region can be tried as well, as long as it has the same file layout (`AssetPackages\index` inside the game folder): press **Change…** and pick its game folder. The interface then marks it **Experimental**.
Only the Taiwanese client has been verified in game. Whether other clients work is unknown.

## How it works

Game resources are a set of `NNNN.blob` files (each holding Unity asset bundles) plus an `index` (the location table of assets and of the bundles they depend on).
mmvp finds the bundles that contain voices, puts the Japanese voices in, repacks them into `9876.blob` (alternating with `9877.blob` on re-apply), and points every `index` location of those bundles to the new file.
The original `index` is kept as `index.bak`.

## Restoring by hand

1. In `AssetPackages` and `AssetPackages\Starter`, rename `index.bak` back to `index`.
2. In the same two folders, delete `9876.blob` (or `9877.blob`) and `index.mmvp.json`.

## Development

A standard Tauri 2 project:

```
src/                  GUI front end (plain HTML/JS)
src-tauri/            Rust workspace root and the mmvp app (lib.rs: Tauri commands, patcher/: apply logic, net.rs: HTTP)
src-tauri/shared/     mmvp-shared: blob / index, UnityFS, SerializedFile, AudioClip, voice site format
src-tauri/dumper/     mmvp-dumper: extra tool that dumps the voice site from the Japanese client
scripts/              publish-voices.mjs: dump, then deploy when something changed
docs/                 technical notes
```

```sh
npm ci
npm run dev                  # start the GUI in development mode
npm test                     # cargo test; set MMVP_TEST_ROOT / MMVP_TEST_SOURCE to run the end-to-end test
npm run dump -- <jp_root> --date YYYY-MM-DD   # only dump the voice site into work/pages/<date>
npm run publish:voices       # dump -> compare with the live site -> deploy if changed (reads deploy.local.json; -- --dry-run / --force)
```

Commit messages and release notes follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `refactor:`, `docs:`, `ci:` …; `!` or a `BREAKING CHANGE:` footer for breaking changes).

Releasing (the version is in `src-tauri/Cargo.toml`):

1. Draft a GitHub release for the tag `vX.Y.Z` and write its notes, grouped by commit type.
2. `git tag vX.Y.Z && git push origin vX.Y.Z`. GitHub Actions builds `mmvp.exe`, uploads it to the draft and publishes it.

Technical details are in [docs/research.md](docs/research.md) (Traditional Chinese).

## Disclaimer

Unofficial tool, not affiliated with NEXON or devCAT. Use at your own risk.
