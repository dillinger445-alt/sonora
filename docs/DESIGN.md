# Interface design

SONORA is meant to read like a measuring instrument. The interface is
graphite and cool grey, and it uses colour the way metering hardware uses
LEDs: as a signal, not as decoration.

## Principles

1. **Colour is a status.** Lime means signal, pass, a primary action or
   keyboard focus. Amber means caution. Red means a fault such as clipping
   or an error. Charts use a separate set of steel-blue data colours that
   never carry status. Musical keys carry a muted hue from their place on
   the Camelot wheel, so neighbouring keys (which mix) look alike; it is
   only ever a small dot or a rim on the wheel, at low saturation, so it
   never reads as a status.
2. **Numbers are the content.** Values are set in a condensed monospace
   with tabular figures and right-aligned in columns. Units go in column
   headers where possible.
3. **One time axis.** On the track view, the waveform, spectrogram and
   loudness curve share a time axis and a cursor, and the playhead while
   the track plays. This, and the Camelot wheel of the key filter, are
   where the interface spends its visual boldness; everything around them
   stays quiet.
4. **Evidence is visible.** Each finding shows its evidence level as four
   pips (confirmed, strong, weak, inconclusive). Inconclusive results are
   shown, not hidden.
5. **Quiet structure.** Tonal layers and hairlines instead of cards. Only
   floating layers (menus, dialogs, toasts) cast a shadow, to lift them off
   the content beneath. Corner radii are small (3 px on controls, 6 px on
   dialogs). There are no gradients except in the loudness area fill, which
   follows the data.
6. **Stable layouts.** Selecting rows, opening the details panel, setting
   filters or receiving live updates never moves content under the pointer.
   The library toolbar has two fixed rows (search, score and actions; then
   the filters) for that reason. The player takes its own row above the
   status bar only while a track is loaded.

## Tokens

All tokens live in `src/styles/tokens.css`.

| Group | Tokens |
| --- | --- |
| Surfaces | `--c-rail` #16181b, `--c-canvas` #1b1e21, `--c-panel` #202327, `--c-raised` #262a2f, `--c-hover`, `--c-active`, `--c-well` |
| Lines | `--c-line` #31363c, `--c-line-strong` #434a52, `--c-line-faint` |
| Text | `--c-text` #e3e6e9, `--c-text-2` #a3abb3, `--c-text-3` #8c949d, `--c-text-off` |
| Signal | `--c-lime` #c0e84e, `--c-amber` #e9ae40, `--c-red` #f06a5e, each with `-dim` (fills) and `-line` (borders) variants |
| Data | `--c-band-low`, `--c-band-mid`, `--c-band-high` (waveform bands), `--c-data-1`, `--c-data-2`, `--c-grid`, `--c-unlit` |
| Type | `--font-sans` Archivo (variable width; interface text at 94 %), `--font-mono` Martian Mono (numbers at 78 % width); sizes 11 / 12 / 13 / 15 / 18 px, score numerals 30 px |
| Space and shape | 4 px grid (`--s-1` to `--s-8`), `--r-ctl` 3 px, `--r-dlg` 6 px, control height 28 px, table rows 32 px |

Text colours meet WCAG AA contrast (4.5:1) on the surfaces they are used
on. Tertiary text and red reach at least 4.7:1 even on raised surfaces;
where tertiary text sits on a hover or selected surface (menu hints, tab
counts) it steps up to secondary. Keyboard focus, including the active row
of a focused table, is drawn in lime, well above the 3:1 required for
focus indicators. The
fonts are bundled (`src/assets/fonts`, SIL Open Font License) and subset to
Latin scripts. Other scripts in track titles fall back to system fonts.

## Components

The building blocks are in `src/components/`:

- **Buttons and controls**: `Button`, `IconButton`, `Segmented` (a radio
  group), `TabList` (tabs that switch a panel, with arrow keys), `Checkbox`,
  `SearchField`, `Switch`, `ProgressBar` (determinate only).
- **Overlays**: `Menu` (action and checkbox menus with full keyboard
  support), `ContextMenu` (the same menu opened at the pointer by a
  right-click, or at a row by Shift+F10 or the Menu key; Escape puts focus
  back), `Popover` (a panel of controls under a button; Escape, a click
  outside or tabbing past its ends closes it), `Dialog` and `useConfirm`
  (native `<dialog>`; destructive confirmations focus Cancel first),
  `TooltipLayer` (one delegated tooltip for every `data-tip` attribute).
- **DJ**: `KeyWheel` (the Camelot wheel as a picker: one Tab stop, arrows
  go round it and between the major and minor rings), `BpmFilter` and
  `KeyFilter`, `KeyBadge`, `BpmText` and `SourceNote` (where a value came
  from), and `PlayerBar` (transport, waveform seek bar as a slider, volume;
  for a Soulseek preview, a Preview badge, how it is going, how far the file
  has arrived on the seek bar, Download and the next preview).
- **Marks**: `ScoreMeter` (20-segment ladder), `ScoreCell` (number plus a
  10-segment mini ladder), `EvidenceMark`, `SeverityIcon`, `FlagList`,
  `StatusMark`. On the Soulseek page, each folder of results carries a
  five-bar availability ladder, rising like the wordmark's bars: lime when
  the person has a free upload slot (more bars for a faster connection),
  amber when the download would wait in a queue.
- **Tables and layout**: `TrackTable` (virtualized and paged from the core;
  columns drop by priority as space shrinks), `Sidebar`, `StatusBar`,
  `ViewHeader`, `Section`, `EmptyState`, `ErrorState`.
- **Charts** (`src/charts/`): `Lanes` (canvas), `SpectrumChart` (uPlot),
  `BandBars`.

Icons come from Lucide (`react-icons/lu`).

## Motion

Motion is limited to transitions that answer an action (button states,
switches, toasts), progress bars easing to their new value, the spinner of a
track being analyzed, and the blinking indexing dot. `prefers-reduced-motion`
turns all of it off.
