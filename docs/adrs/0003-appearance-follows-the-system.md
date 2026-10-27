# ADR 0003: Appearance follows the system, with a remembered override

| | |
|---|---|
| **Status** | Accepted; shipped in 0.2.1 |
| **Date** | 2026-10-07 |

## Context

Ferrender drew one light theme. Users on dark desktops asked for the app to follow the operating system, and some wanted a fixed choice regardless of it.

## Decision

View › Appearance offers System, Light and Dark. System reads the macOS appearance directly and the desktop portal on Linux, and updates while the app is open; Light and Dark are explicit overrides. The choice is saved for the next launch. Appearance covers chrome, sketch drawing, the timeline and the viewport background, and never the document, images or exports.

## Consequences

Theme is a display concern separate from the design, which is the line later drawn again for per-component appearance and materials in the 0.5 modeling plan. Native OS-event delivery and file-picker appearance need desktop checks, which the record's manual plan lists.

---

## Original document

The design document as written for the release, kept in full. Headings are demoted one level; links were updated when the documents were reorganised on 9 October 2026.

## Appearance in Ferrender 0.2.1

Choose **View → Appearance → System, Light, or Dark**.

- **System** is the default. Ferrender follows the operating system's appearance and updates while the app is open.
- **Light** and **Dark** keep the selected appearance regardless of the OS setting.
- Your choice is saved for the next launch. Choose **System** again to resume following the OS.

The setting covers menus, panels, dialogs, sketch geometry and dimensions, the timeline, and the viewport background. It does not change document geometry, image colors, or exported STL/STEP files. Native file pickers are provided by the operating system and may follow its appearance independently.

System mode reads the macOS appearance directly and uses the desktop settings portal on Linux. Allow a few seconds for an OS change to appear. If the desktop does not provide an appearance preference, Ferrender uses Light. You can always select Light or Dark explicitly.

### Manual test plan

Save any open work before restarting. Use **Help → About Ferrender → Copy build info** when reporting a failure. Check that the version is **0.2.1** and include its commit.

1. **Switch manually.** Open a model and select Dark, then Light, from View → Appearance. Expect the whole window to update immediately, with one selected appearance in the menu. Check the browser, toolbar, timeline, viewport, menus, and About dialog for unreadable text or leftover bright/dark panels.

2. **Follow the OS live.** Select System. While Ferrender stays open, change the OS from Light to Dark and back. On macOS, use System Settings → Appearance. Allow a few seconds and expect Ferrender to follow both changes without reopening the document or restarting. If it does not, report the OS and desktop/window system, including whether manual switching works.

3. **Keep a manual override.** Set Ferrender to Dark and the OS to Light; Ferrender should stay dark. Repeat with Ferrender Light and OS Dark. Select System again; it should immediately match the current OS appearance.

4. **Remember the choice.** Select Dark, quit normally, and reopen. Expect Dark to remain selected. Repeat with Light and System. A saved System setting should use the OS appearance at the next launch.

5. **Read a sketch.** Edit a sketch with dimensions, constraints, a circle, and an open line. In both themes, inspect selected and unselected geometry, fully constrained geometry, X/Y axes, the grid, dimension labels, constraint badges, orange open-end rings, and the drawing preview. Open Point Coordinates and enter an invalid value such as `1 / 0`; expect readable fields and a readable error with the design preserved.

6. **Trace an image.** Open a sketch containing a reference image. Switch themes while editing. Expect the photo's colors, placement, scale, rotation, and opacity to remain unchanged. Check Reference Image and its two-point calibration markers for legibility over both light and dark portions of the photo. Cancel calibration afterward.

7. **Inspect a solid.** In both themes, orbit a model, select a face, open an Extrude preview, and inspect a measurement or section view. Expect visible faces, edge highlights, handles, labels, and error messages. There should be no blank viewport or stale image after switching.

8. **Keep the design unchanged.** Save a model so its unsaved marker disappears. Switch appearances repeatedly and return to System. Expect the model, camera, history position, and saved status to remain unchanged. Switching appearance should not add an Undo step. Save and reopen a copy, then export STL or STEP; appearance should have no effect on its dimensions or shape.

Report by number, for example: “1–4 pass; 5: the selected dimension is hard to read in Dark.” Include a screenshot for visual problems. Native OS-event delivery, restart persistence, and file-picker appearance need these desktop checks; synthetic UI tests do not establish that a particular desktop reports its setting correctly.
