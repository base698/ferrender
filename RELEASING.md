# Native releases

A version tag such as `v0.1.0` runs `.github/workflows/release.yml`. The tag must agree with the app's Cargo version and have matching notes in `docs/releases/`. The workflow publishes only after all three native builds, tests and packaging checks succeed.

Targets are Apple Silicon macOS, Intel macOS, and Linux x86-64 (Ubuntu 24.04 / glibc 2.39 baseline). The macOS runners are `macos-15` and `macos-15-intel`, as listed in [GitHub's runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners). macOS apps are ad-hoc signed; notarization requires separately configured Apple credentials.

For each release:

1. Update the package versions and lockfile, documentation, and versioned release notes.
2. Run the relevant tests, inspect the source diff, and commit. Never tag a dirty working tree or replace an existing published tag.
3. Push the release commit to the candidate branch and wait for the macOS and Linux CI jobs to pass. Merge that verified commit into main (fast-forward when possible), confirm the remote main commit, then push its `vMAJOR.MINOR.PATCH` tag.
4. Check the Native release workflow and the resulting three downloads and SHA256SUMS.txt.
5. Verify Help > About / `--version` on the packaged apps. The embedded commit must match the tag.

To package locally after committing a clean checkout, run `sh scripts/bundle-macos.sh` on a Mac or the verified native dependency preparation and Cargo command below on Linux. Then run `python3 scripts/package-release.py --platform macos-arm64` (or `macos-x86_64` / `linux-x86_64`). Output goes into ignored `dist/`. The packager refuses mismatched versions, commits, dirty builds or platforms, checks the packaged executable, and includes the tutorial and dependency notices.

For an existing tag, the Native release workflow can also be dispatched from main with its `tag` input. This uses the current workflow definition while checking out the exact tagged source, so packaging automation can be repaired without moving a release tag.

A failed publish may leave a draft release. Rerunning the same tagged workflow can finish that draft. It refuses to overwrite an already published release. Do not upload local settings, recovery files, private designs, or private branches.


### Verified native dependency

CI, release builds and `scripts/bundle-macos.sh` use `scripts/prepare-occt.py` before Cargo. It pins the official cadrum OpenCascade 8.0.1 rev2 archive sizes and SHA256 digests for all three release targets, verifies downloads before extraction, and supplies the resulting `OCCT_ROOT`. The source digests are recorded in the helper from the [official release metadata](https://api.github.com/repos/lzpel/cadrum/releases/tags/occt-8_0_1_rev2). Changing cadrum or its prebuilt revision requires reviewing and updating these pins.

For a manual Linux or macOS release build, run:

```sh
export OCCT_ROOT="$(python3 scripts/prepare-occt.py)"
cargo build --locked --release -p ferrender
```

The helper needs Python 3.12 or a security-backported Python providing `tarfile.data_filter`. Its offline negative tests run with `python3 scripts/test_prepare_occt.py`. Verified archives and extracted libraries live under `target/verified-occt`, separate from existing Cargo/OCCT caches. Package generation rechecks the archive and uses notices from the verified library directory.

Raw `cargo build` without this preparation can still use cadrum's upstream downloader or an explicitly supplied `OCCT_ROOT`; the Cargo lockfile does not authenticate that separately downloaded native archive. The verifier pins native build inputs, and does not provide an OS sandbox for build scripts.
