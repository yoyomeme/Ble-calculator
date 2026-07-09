# UI Layout and Visual Pass

This document explains how the calculator UI is structured, why its dimensions are fixed in several places, and how to run the browser screenshot pass.

## Design Goal

The calculator is the main screen. The side panels are supporting surfaces:

```text
collapsed/open host bench   fixed calculator   collapsed/open peers/history
```

The app should remain usable when the window is resized. The calculator should not shrink below its designed size, the side panels should not overlap it, and long text should truncate or scroll inside its own component.

## Primary Files

| File | Purpose |
| --- | --- |
| `src/renderer/main.tsx` | React components, UI state, API calls, drawer toggles, calculator actions. |
| `src/renderer/styles.css` | Theme tokens, grid layout, fixed dimensions, drawer behavior, overflow rules. |
| `src/renderer/browser-calculator.ts` | Browser-only mock API used when running at `http://127.0.0.1:5173` without Electron. |
| `scripts/browser-visual-ui-pass.mjs` | Chrome DevTools Protocol screenshot and overlap checker. |
| `evolve-calculator-theme.md` | Theme direction used for the facelift. |

## Layout Model

The shell is a grid:

```css
.workspace {
  grid-template-columns: 52px 420px 52px;
}

.workspace.host-open {
  grid-template-columns: 300px 420px 52px;
}

.workspace.history-open {
  grid-template-columns: 52px 420px 360px;
}

.workspace.host-open.history-open {
  grid-template-columns: 300px 420px 360px;
}
```

The center column is always `420px`. The left and right columns are either collapsed drawer rails or open panels.

Both side panels open by default when the viewport is wide enough for the full layout. When the viewport is narrower than the full layout, the app falls back to one open side panel:

- `toggleHostBench()` toggles the left drawer and keeps the right drawer open only when the full layout fits.
- `toggleHistory()` toggles the right drawer and keeps the left drawer open only when the full layout fits.

This keeps the calculator visually dominant, starts desktop users with the full workbench visible, and prevents a three-wide expanded layout from becoming cramped on smaller screens.

## Fixed Dimensions

These fixed sizes are intentional:

| Component | Size | Reason |
| --- | --- | --- |
| Calculator panel | `420px` wide, `620px` high | Keeps display, entry row, and keypad stable. |
| Collapsed drawer | `52px` wide | Leaves only the expand button visible. |
| Host Bench open drawer | `300px` wide | Fits room controls, role segment, facts, and error box. |
| Network open drawer | `360px` wide | Fits discovery rows, the connection card, and history results without crowding. |
| Status rail | `420px` max target | Aligns visually with the calculator. |

On narrow browser widths, the document is allowed to scroll horizontally. This is preferred over shrinking the calculator until text and keypad controls overlap.

## Overflow Rules

The CSS relies on a few repeated patterns:

- Parent grid/flex children use `min-width: 0` so ellipsis can work.
- Long labels use `overflow: hidden`, `text-overflow: ellipsis`, and `white-space: nowrap`.
- The calculator display uses `overflow-wrap: anywhere` so typed expressions stay inside the display.
- `.peer-list` and `.history-list` use `overflow: auto`.
- Panels use `overflow: hidden` so child lists scroll instead of growing the panel.

The history drawer must not extend vertically as events are added. New events are prepended to `state.history`, and `.history-list` scrolls inside `.history-section`.

## Component Responsibilities

| Component | Responsibility |
| --- | --- |
| `App` | Owns renderer state and renders all major surfaces. |
| `PanelHeader` | Draws the Host Bench heading. |
| `SegmentedRole` | Displays whether the app is acting as host or guest. |
| `SessionFacts` | Shows local device, room, pending action, and trust state. |
| `PeerRow` | Shows peer identity/trust and calls `connectGuest()`. |
| `StatusBeacon` | Displays network mode: `Idle`, `Scanning`, or `Signal`. |
| `StatusPill` | Draws bottom status rail items. |

The UI calls only `calculatorApi` commands. It does not load native modules, read files, access BLE, or store secrets.

## Browser Mock

When the renderer runs in Electron, `window.calculator` comes from preload.

When the renderer runs directly in a browser, there is no preload script. `App` falls back to:

```ts
window.calculator ?? createBrowserCalculatorApi()
```

This fallback exists so designers and developers can inspect the UI at `http://127.0.0.1:5173` and run screenshot automation without launching Electron.

Do not put security-sensitive behavior in `browser-calculator.ts`. It is a UI review mock only.

## Visual Screenshot Pass

Start Vite:

```sh
npm run dev
```

Then run the browser pass:

```sh
node scripts/browser-visual-ui-pass.mjs
```

The script expects:

- Vite at `http://127.0.0.1:5173`.
- Google Chrome at `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`.

The script launches headless Chrome with remote debugging, opens the Vite app, toggles drawers, captures screenshots, and records bounding boxes.

## Scenarios

The visual pass currently captures:

| Scenario | Viewport | Drawer state |
| --- | --- | --- |
| `desktop-closed` | `1180x760` | both collapsed |
| `desktop-left-open` | `1180x760` | Host Bench open |
| `desktop-right-open` | `1180x760` | Peers/History open |
| `desktop-both-open` | `1180x760` | Host Bench and Peers/History open |
| `compact-closed` | `520x760` | both collapsed |
| `compact-left-open` | `520x760` | Host Bench open |
| `compact-right-open` | `520x760` | Peers/History open |
| `compact-both-attempt` | `520x760` | Attempts both; app should keep one side open |
| `short-height-right-open` | `850x560` | Peers/History open |

Outputs are written to:

```text
artifacts/browser-visual-ui-pass/
```

The report is:

```text
artifacts/browser-visual-ui-pass/report.json
```

## Overlap Detection

The script measures these selectors:

- `.left-panel`
- `.calculator-panel`
- `.right-panel`
- `.display`
- `.keypad`
- `.history-list`
- `.peer-list`
- `.status-rail`

It flags overlap between:

- left vs center
- center vs right
- left vs right

A clean pass should show `overlaps=0` for every scenario.

## When Changing UI

After changing `src/renderer/main.tsx` or `src/renderer/styles.css`:

1. Run `npm run typecheck`.
2. Run `npm run lint`.
3. Run `npm run build`.
4. Start Vite and run `node scripts/browser-visual-ui-pass.mjs`.
5. Inspect the screenshots, not only `report.json`.

The overlap report catches panel collision. It does not replace human review for visual polish, readability, color contrast, or awkward text truncation.
