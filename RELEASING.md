# Native releases

A version tag such as `v0.1.0` runs `.github/workflows/release.yml`. The tag must agree with the app's Cargo version and have matching notes in `docs/releases/`. The workflow publishes only after all three native builds, tests and packaging checks succeed.

Targets are Apple Silicon macOS, Intel macOS, and Linux x86-64 (Ubuntu 24.04 / glibc 2.39 baseline). The macOS runners are `macos-15` and `macos-15-intel`, as listed in [GitHub's runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners). macOS apps are ad-hoc signed; notarization requires separately configured Apple credentials.

For each release:

1. Update the package versions and lockfile, documentation, and versioned release notes.
2. Run the relevant tests, inspect the source diff, and commit. Never tag a dirty working tree or replace an existing published tag.
3. Push the commit, then its `vMAJOR.MINOR.PATCH` tag.
4. Check the Native release workflow and the resulting three downloads and SHA256SUMS.txt.
5. Verify Help > About / `--version` on the packaged apps. The embedded commit must match the tag.

To package locally after committing a clean checkout, run `sh scripts/bundle-macos.sh` on a Mac or `cargo build --locked --release -p ferrender` on Linux. Then run `python3 scripts/package-release.py --platform macos-arm64` (or `macos-x86_64` / `linux-x86_64`). Output goes into ignored `dist/`. The packager refuses mismatched versions, commits, dirty builds or platforms, checks the packaged executable, and includes the tutorial and dependency notices.

A failed publish may leave a draft release. Rerunning the same tagged workflow can finish that draft. It refuses to overwrite an already published release. Do not upload local settings, recovery files, private designs, or private branches.
