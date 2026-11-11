# Releases

One folder per release series (`major.minor`). Each holds the documents of that series:

| File | What it is |
|---|---|
| `<version>-release-notes.md` | The notes published with the tag; a point release adds its own, such as `0.4.1-release-notes.md`, in the same folder. The release workflow reads `docs/releases/<major.minor>/<version>-release-notes.md`. |
| `plan.md`, `plan-<topic>.md` | What the release intends to build, written before the work. A series that is not yet started has only plans. |
| `test-plan.md` | The full catalogue of manual checks for the series. |
| `acceptance.md` | The short ordered session that decides whether to ship, with its results. |
| `review.md`, `evaluation.md` | Independent review and workflow evaluations recorded against the series. |

| Series | Contents |
|---|---|
| [0.1](0.1/) | release notes |
| [0.2](0.2/) | release notes for 0.2.0, 0.2.1 and 0.2.2 |
| [0.3](0.3/) | release notes, test plan, projection retest |
| [0.4](0.4/) | plan, release notes, test plan, acceptance, review, large-assembly evaluation |
| [0.5](0.5/) | plans only: [scripts](0.5/plan-scripts.md) and [modeling](0.5/plan-modeling.md) |

Design decisions live in [docs/adrs](../adrs/README.md); the file format reference is [docs/FILE_FORMAT.md](../FILE_FORMAT.md).
