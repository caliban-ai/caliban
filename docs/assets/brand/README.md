# caliban brand assets

The caliban mark is an **eye of the storm**: five cyclone blades spinning
counter-clockwise around a calm center, enclosed in a containing ring. It
expresses *the tempest harnessed* — chaotic forces (many models, providers,
sub-agents) churned into controlled work, with the operator as the still
point at the eye. See the full rationale in
[`docs/superpowers/specs/2026-06-03-caliban-logo-design.md`](../../superpowers/specs/2026-06-03-caliban-logo-design.md).

## Files

| File | Use |
| --- | --- |
| `mark.svg` | Master mark. Strokes/fills use `currentColor` — set the color via CSS `color` or a parent `fill`/`color`. |
| `mark-white.svg` | Mark in off-white `#e6edf3`. For dark backgrounds. |
| `mark-ink.svg` | Mark in ink `#1b2330`. For light backgrounds. |
| `lockup-horizontal.svg` | Primary lockup: mark + `caliban` in monospace. `currentColor`. |
| `lockup-stacked.svg` | Square/avatar companion: mark over wide-tracked `caliban`. `currentColor`. |
| `favicon.svg` | Self-contained favicon. Adapts to light/dark via `prefers-color-scheme`; heavier strokes for small-size legibility. The **primary** favicon. |
| `favicon-tile.svg` | Source for the raster fallbacks: off-white mark on the dark brand tile (`#0d1117`, rounded). |
| `favicon-16.png` / `favicon-32.png` / `favicon-48.png` | Raster fallbacks for legacy contexts, baked from `favicon-tile.svg`. |
| `apple-touch-icon.png` | 180×180 home-screen / social icon, baked from `favicon-tile.svg`. |

`currentColor` assets inherit their color. In HTML/CSS:

```html
<span style="color:#e6edf3"><!-- inline the SVG, or --></span>
<img src="mark.svg" alt="caliban"><!-- img: use mark-white.svg / mark-ink.svg for a baked color -->
```

## Color

| Context | Foreground | Background |
| --- | --- | --- |
| Default (dark / terminal) | off-white `#e6edf3` | `#0d1117` |
| Light-mode fallback | ink `#1b2330` | `#f6f8fa` |

The mark is monochrome by design — terminal-native and theme-agnostic. Do not
introduce chromatic color.

## Don'ts

- Don't recolor with chromatic hues.
- Don't rotate the cyclone to spin clockwise (it spins **counter-clockwise**).
- Don't add drop shadows or gradients.
- Don't crowd it — keep clear space around the mark equal to the ring radius.

## Minimum sizes

- Standard mark: **24px**.
- `favicon.svg`: legible down to **16px** (carries heavier strokes).

## Regenerating raster favicons

The PNGs are baked from `favicon-tile.svg` (the dark tile keeps the monochrome
mark legible on any tab-bar background — a transparent PNG of the adaptive
`favicon.svg` would vanish on one theme or the other). With `rsvg-convert`
(librsvg — `brew install librsvg`):

```bash
rsvg-convert -w 16  -h 16  favicon-tile.svg -o favicon-16.png
rsvg-convert -w 32  -h 32  favicon-tile.svg -o favicon-32.png
rsvg-convert -w 48  -h 48  favicon-tile.svg -o favicon-48.png
rsvg-convert -w 180 -h 180 favicon-tile.svg -o apple-touch-icon.png
```

Prefer `favicon.svg` (vector, theme-adaptive) wherever SVG favicons are
supported; ship the PNGs as fallbacks.
