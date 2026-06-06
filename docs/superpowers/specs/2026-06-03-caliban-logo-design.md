# Caliban logo — design spec

**Date:** 2026-06-03
**Status:** Approved (brainstorming complete)

## Overview

A primary identity mark for the `caliban` agent harness, plus its color
treatment and wordmark lockups. The mark is an "eye of the storm" cyclone:
five blades spinning counter-clockwise around a calm center, enclosed in a
containing ring.

## Concept & rationale

Caliban is the creature of the storm-wracked island in Shakespeare's *The
Tempest*. The logo expresses **the tempest harnessed** — chaotic forces
(many models, providers, sub-agents, routing) churned into controlled work,
with the operator as the still point at the eye. This stays true to the name
without being a literal storm cloud or a generic dev-tool glyph.

The design was chosen over three alternatives explored during brainstorming:
a literal creature mask (C3), a bare three-blade cyclone (D1), and a
storm-eye fusion (D2). The ringed five-blade cyclone won for being on-theme,
distinctive, balanced, and legible at favicon scale.

## The mark

- **Grid:** 120 × 120 viewBox.
- **Blades:** five cyclone arms, gentle curl, **counter-clockwise** (the
  current curl reflected across the vertical axis). Stroke weight 7 at full
  size, round caps. Placed at 0°/72°/144°/216°/288°.
- **Ring:** a thin containing circle, radius 44, stroke weight 3 — a bounded
  system.
- **Center:** a solid filled dot, radius 6.5 — the calm eye / operator in
  control.
- **Color binding:** all strokes and fills use `currentColor` so a single
  master file adapts to any context via CSS `color`.

### Master SVG (source of truth)

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 120" fill="none" role="img" aria-label="caliban">
  <circle cx="60" cy="60" r="44" stroke="currentColor" stroke-width="3"/>
  <g stroke="currentColor" stroke-width="7" stroke-linecap="round" transform="translate(120,0) scale(-1,1)">
    <path d="M60 56 C80 47 93 60 85 75" transform="rotate(0 60 60)"/>
    <path d="M60 56 C80 47 93 60 85 75" transform="rotate(72 60 60)"/>
    <path d="M60 56 C80 47 93 60 85 75" transform="rotate(144 60 60)"/>
    <path d="M60 56 C80 47 93 60 85 75" transform="rotate(216 60 60)"/>
    <path d="M60 56 C80 47 93 60 85 75" transform="rotate(288 60 60)"/>
  </g>
  <circle cx="60" cy="60" r="6.5" fill="currentColor"/>
</svg>
```

> **Favicon note:** at ≤32px, bump blade stroke to ~9 and ring to ~4 so the
> form stays legible. A dedicated favicon variant carries those weights.

## Color

| Context | Foreground | Background |
| --- | --- | --- |
| Default (dark / terminal) | off-white `#e6edf3` | `#0d1117` |
| Light-mode fallback | ink `#1b2330` | `#f6f8fa` |

No chromatic color. The mark is terminal-native and theme-agnostic; chromatic
palettes (Uranus cyan, Rust amber, electric violet, two-tone) were explored
and set aside in favor of monochrome.

## Wordmark lockups

- **Primary — horizontal.** Mark to the left of `caliban`, set lowercase in a
  **monospace** typeface (`ui-monospace, "SF Mono", Menlo, Consolas`), light
  letter-spacing. CLI-honest; matches the tool it lives in.
- **Companion — stacked.** Mark above a wide-tracked `caliban` for square
  contexts (GitHub org avatar, app tiles). Complements, does not replace, the
  horizontal lockup. Case can be set at production time; lowercase matches the
  project's house style.
- **Icon-only.** The bare mark, for favicons and tight square placements.

Exact monospace family may be finalized at asset-production time; the lockup
is robust across common monospace fonts.

## Deliverables

All derived from the single master mark, in `docs/assets/brand/`:

1. `mark.svg` — master, `currentColor`-driven.
2. `mark-white.svg` / `mark-ink.svg` — color-baked convenience variants.
3. `lockup-horizontal.svg` — mark + monospace `caliban`.
4. `lockup-stacked.svg` — square/avatar companion.
5. `favicon.svg` — adaptive (`prefers-color-scheme`), heavier strokes for
   small-size legibility. Primary favicon.
6. `favicon-tile.svg` + `favicon-16/32/48.png` + `apple-touch-icon.png` —
   raster fallbacks baked onto the dark brand tile (legible on any tab-bar
   theme). See the brand README for regeneration.

## Usage guidelines

- Maintain clear space around the mark equal to the ring radius.
- Do not recolor with chromatic hues, rotate the cyclone to clockwise, or
  add a drop shadow.
- On busy backgrounds, place the mark on a solid `#0d1117` or `#f6f8fa`
  field.
- Minimum size: 16px (favicon variant only); 24px for the standard mark.
