# 3D archive and local test assets

When working on the maintainer's Mac, the canonical 3D archive is the Synology **Backup/3d** share (singular `Backup`). It is mounted at `/Volumes/Backup/3d` and is also accessible through the existing `~/Documents/3d` symlink. Verify that the SMB share is mounted before writing; do not create a local substitute beneath `/Volumes` when it is disconnected.

## Storage layout

| Relative to `Backup/3d` | Contents |
|---|---|
| `ferrender/models/chess/` | Bishop, rook, pawn, king, bishop tutorial, unique variants and historical generators |
| `ferrender/models/sea-turtle/` | Organic turtle design, STL, previews and sculpting seed |
| `ferrender/models/face-relief/` | Private portrait relief design, STL, depth image and construction images |
| `ferrender/testing/0.4-review/` | Current review, acceptance plan, builds, evidence and native test fixtures |
| `meshes/large-imports/` | Bearded Man, Cupid, Beethoven and their attribution/checksums |
| `meshes/large-imports/review-synthetic/` | 5M/12M synthetic meshes and editable designs |
| `references/` | Stroller, mailbox, Mark5, unidentified parts and design-idea references |
| `fusion/` | Existing Fusion archive, organized independently; preserve its layout |
| `_organization/` | Migration records, original indexes and source/destination hash manifests |

The former local `~/Documents/ferr-tests` and `~/Documents/3d print references` collections were organized here on 9 October 2026. Use the archive's README indexes to find files; do not recreate the retired local test-pack layout. Historical reports may still quote the paths used when tests originally ran.

## Handling files

- Keep useful models, distinct variants, source meshes and attribution. Matching names or sizes do not establish duplicates; compare contents before removing a copy.
- Keep current test evidence separate from the model library. Test packs through 0.3.0 were retired; the useful older chess models/tutorials were preserved.
- Keep runtime settings, recovery data, sockets and installation cache keys local to the Mac, not in the shared archive. For isolated acceptance testing use `~/Library/Application Support/Ferrender-Acceptance-0.4`; only test designs/results belong under the archive.
- Keep generated models, large fixtures, reference photos and personal images out of the source repository and public releases. Commit code and documentation only.
- Verify copied file contents before retiring originals. Preserve recoverable originals and migration records; do not empty Trash as part of routine organization.
- Update the archive indexes and relevant repository/test notes when locations change. Refresh a checksum manifest if its files or paths change.
- Historical Python generators may contain old absolute paths. Update their output paths deliberately before running them; moving a generator does not change those assumptions.

## Local Ferrender installation

- The normal macOS installation is `/Applications/Ferrender.app`. The desktop icon is a Finder alias to it; the Codex task's `outputs/Ferrender.app` links to it. Update the Applications installation when delivering a new local build, then verify Help → About against the intended commit. Updating only the desktop copy leaves Spotlight launching an older build.
- Keep historical app bundles in non-indexed archive folders, and use distinct names/bundle IDs for isolated review apps. Preserve settings, recovery files and user designs when updating. Do not replace the desktop alias with an independent build.
- The 9 October 2026 Spotlight repair preserved old bundles in adjacent `archived-builds.noindex` folders. Its location/hash records are in the Codex task's `work/spotlight-repair/`.
