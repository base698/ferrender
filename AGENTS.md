# Working in this repository

## Models, fixtures and personal files

- Keep generated models, large fixtures, reference photos and personal images out of the source repository and public releases. Commit code and documentation only.
- The maintainer's model library and test evidence live outside the repository. Do not recreate test packs here; historical reports may still quote the paths used when tests originally ran.
- Keep runtime settings, recovery data, sockets and installation cache keys local to the machine. For isolated acceptance testing use `~/Library/Application Support/Ferrender-Acceptance-0.4`.
- Matching names or sizes do not establish duplicates; compare contents before removing a copy, and verify copied file contents before retiring originals.
- Historical Python generators may contain old absolute paths. Update their output paths deliberately before running them.

## Local Ferrender installation

- The normal macOS installation is `/Applications/Ferrender.app`. The desktop icon is a Finder alias to it; the Codex task's `outputs/Ferrender.app` links to it. Update the Applications installation when delivering a new local build, then verify Help → About against the intended commit. Updating only the desktop copy leaves Spotlight launching an older build.
- Keep historical app bundles in non-indexed archive folders, and use distinct names/bundle IDs for isolated review apps. Preserve settings, recovery files and user designs when updating. Do not replace the desktop alias with an independent build.
- The 9 October 2026 Spotlight repair preserved old bundles in adjacent `archived-builds.noindex` folders. Its location/hash records are in the Codex task's `work/spotlight-repair/`.
