# Dotsquares AI (opcode) — Design Theme Reference

A complete description of the visual design system used by this app: tokens, palettes,
typography, layout/chrome, component styling, motion, and the conventions the codebase
actually follows.

**Stack:** Tauri 2 (custom-decorated, transparent window) + React 18 + TypeScript +
Tailwind CSS v4 (CSS-first `@theme`) + Radix UI primitives + `class-variance-authority`
+ Framer Motion + `lucide-react` icons.

**Source of truth:** `src/styles.css` (tokens + globals), `src/contexts/ThemeContext.tsx`
(runtime theme switching), `src/components/ui/*` (primitives), `src-tauri/tauri.conf.json`
(window shell).

---

## 1. Design Language — the short version

| Trait | How it shows up |
|---|---|
| **Dark-first** | The base `@theme` block *is* the dark palette. Light is an override class. `<html class="dark">` and `<meta name="color-scheme" content="dark">` ship in `index.html`. |
| **Near-monochrome** | Every neutral is `oklch(L 0.01 240)` — a fixed hue 240 (blue) at chroma 0.01. The whole UI is a cool, desaturated grey ramp; colour is reserved for meaning. |
| **Frameless / native-feeling** | No OS titlebar. The app draws its own 44px titlebar with macOS traffic lights, and the window is transparent with a 12px rounded corner mask. |
| **IDE / browser metaphor** | Tab strip → tab content. Everything opens as a tab (sessions, settings, usage, agents, MCP, CLAUDE.md, ticket board). |
| **Flat, low-shadow, hairline borders** | 1px `--color-border` separators do most of the work; shadows are subtle and reserved for floating layers. |
| **Quiet motion** | 0.15s is the house duration. Framer Motion for enter/exit, CSS transitions for hover. |
| **No focus rings** | Focus outlines/rings are globally suppressed (see §9 — this is an accessibility caveat). |

---

## 2. Colour System

### 2.1 Token model

All colours are Tailwind v4 `@theme` variables in `src/styles.css`, expressed in **OKLCH**.
Theme switching swaps the values by adding a class to `<html>`:
`theme-dark` / `theme-gray` / `theme-light` / `theme-custom` (plus an unused `theme-white`).

The shadcn-style semantic set:

```
--color-background / --color-foreground
--color-card / --color-card-foreground
--color-popover / --color-popover-foreground
--color-primary / --color-primary-foreground
--color-secondary / --color-secondary-foreground
--color-muted / --color-muted-foreground
--color-accent / --color-accent-foreground
--color-destructive / --color-destructive-foreground
--color-border
--color-input
--color-ring
--color-green-500 / --color-green-600
```

Note the inversion trick: **`primary` is not a brand hue — it is the foreground colour.**
In dark themes `primary` is near-white with a near-black `primary-foreground`; in light
themes it flips. So a "primary button" reads as a high-contrast white-on-dark (or
black-on-light) pill, not a coloured CTA.

### 2.2 Dark theme (base `@theme` block)

| Token | OKLCH | ≈ Hex | Role |
|---|---|---|---|
| `background` | `oklch(0.10 0.01 240)` | `#020405` | App canvas |
| `foreground` | `oklch(0.95 0.01 240)` | `#e9f0f5` | Body text |
| `card` | `oklch(0.14 0.01 240)` | `#060a0d` | Cards, active tab |
| `card-foreground` | `oklch(0.95 0.01 240)` | `#e9f0f5` | Text on cards |
| `popover` | `oklch(0.13 0.01 240)` | `#05080b` | Menus, tooltips, popovers |
| `popover-foreground` | `oklch(0.95 0.01 240)` | `#e9f0f5` | |
| `primary` | `oklch(0.95 0.01 240)` | `#e9f0f5` | Primary button bg, active tab underline, accents |
| `primary-foreground` | `oklch(0.14 0.01 240)` | `#060a0d` | Text on primary |
| `secondary` | `oklch(0.18 0.01 240)` | `#0e1216` | Secondary button |
| `secondary-foreground` | `oklch(0.95 0.01 240)` | `#e9f0f5` | |
| `muted` | `oklch(0.16 0.01 240)` | `#090e11` | Inset surfaces, code chips, TabsList |
| `muted-foreground` | `oklch(0.65 0.01 240)` | `#8a9095` | Secondary/meta text |
| `accent` | `oklch(0.18 0.01 240)` | `#0e1216` | Hover surface |
| `accent-foreground` | `oklch(0.95 0.01 240)` | `#e9f0f5` | |
| `destructive` | `oklch(0.6 0.2 25)` | `#de3b3d` | Errors, delete |
| `destructive-foreground` | `oklch(0.98 0.01 240)` | `#f3faff` | |
| `border` | `oklch(0.20 0.01 240)` | `#12171a` | Hairlines |
| `input` | `oklch(0.20 0.01 240)` | `#12171a` | Field borders |
| `ring` | `oklch(0.50 0.015 240)` | `#5c656b` | Focus ring (suppressed globally) |
| `green-500` | `oklch(0.72 0.20 142)` | `#4ac240` | Success / "installed" |
| `green-600` | `oklch(0.64 0.22 142)` | `#05aa00` | Success, stronger |

### 2.3 Gray theme — **the actual default at runtime**

`ThemeContext` initialises to `'gray'` and applies `gray` when no saved preference
exists. This is a lifted, softer dark — the palette most users see.

| Token | OKLCH | ≈ Hex |
|---|---|---|
| `background` | `oklch(0.18 0.01 240)` | `#0e1216` |
| `foreground` | `oklch(0.95 0.01 240)` | `#e9f0f5` |
| `card` | `oklch(0.23 0.01 240)` | `#191e21` |
| `popover` | `oklch(0.21 0.01 240)` | `#14191c` |
| `primary` | `oklch(0.95 0.01 240)` | `#e9f0f5` |
| `primary-foreground` | `oklch(0.23 0.01 240)` | `#191e21` |
| `secondary` / `muted` / `accent` | `oklch(0.27 0.01 240)` | `#22272b` |
| `muted-foreground` | `oklch(0.65 0.01 240)` | `#8a9095` |
| `destructive` | `oklch(0.6 0.2 25)` | `#de3b3d` |
| `border` / `input` | `oklch(0.32 0.01 240)` | `#2e3437` |
| `ring` | `oklch(0.55 0.015 240)` | `#6a737a` |
| `green-500` / `green-600` | `0.72 0.20 142` / `0.64 0.22 142` | `#4ac240` / `#05aa00` |

### 2.4 Light theme (`.theme-light`)

| Token | OKLCH | ≈ Hex |
|---|---|---|
| `background` | `oklch(0.98 0.01 240)` | `#f3faff` |
| `foreground` | `oklch(0.12 0.01 240)` | `#030609` |
| `card` | `oklch(0.96 0.01 240)` | `#ecf3f8` |
| `popover` | `oklch(0.98 0.01 240)` | `#f3faff` |
| `primary` | `oklch(0.12 0.01 240)` | `#030609` |
| `primary-foreground` | `oklch(0.98 0.01 240)` | `#f3faff` |
| `secondary` / `muted` / `accent` | `oklch(0.94 0.01 240)` | `#e5ecf1` |
| `muted-foreground` | `oklch(0.45 0.01 240)` | `#50565a` |
| `destructive` | `oklch(0.6 0.2 25)` | `#de3b3d` |
| `border` / `input` | `oklch(0.90 0.01 240)` | `#d8dfe4` |
| `ring` | `oklch(0.52 0.015 240)` | `#616a71` |
| `green-500` / `green-600` | `0.62 0.20 142` / `0.54 0.22 142` | `#21a215` / `#008a00` |

### 2.5 White theme (`.theme-white`) — defined in CSS, **not reachable from the UI**

High-contrast light variant. `ThemeMode` in `ThemeContext.tsx` is
`'dark' | 'gray' | 'light' | 'custom'`, so this class is dead CSS today — useful if you
ever want a true-white / AA-contrast mode.

| Token | OKLCH | ≈ Hex |
|---|---|---|
| `background` | `oklch(0.99 0 240)` | `#fcfcfc` |
| `foreground` | `oklch(0.10 0 240)` | `#030303` |
| `card` / `popover` | `oklch(1.0 0 240)` | `#ffffff` |
| `secondary` / `accent` | `oklch(0.95 0.01 240)` | `#e9f0f5` |
| `muted` | `oklch(0.93 0.01 240)` | `#e2e9ee` |
| `muted-foreground` | `oklch(0.40 0.01 240)` | `#43494d` |
| `destructive` | `oklch(0.55 0.25 25)` | `#df000d` |
| `border` / `input` | `oklch(0.88 0.01 240)` | `#d2d8dd` |
| `ring` | `oklch(0.45 0.015 240)` | `#4e575d` |

### 2.6 Custom theme

`.theme-custom` is an empty rule; `ThemeContext.applyTheme()` writes 17 CSS variables
directly onto `document.documentElement` via `style.setProperty`, camelCase → kebab
(`cardForeground` → `--color-card-foreground`). Defaults seed from a slightly-lifted dark:

```
background  oklch(0.12 0.01 240) #030609    foreground  oklch(0.98 0.01 240) #f3faff
card        oklch(0.14 0.01 240) #060a0d    primary     oklch(0.98 0.01 240) #f3faff
secondary/muted/accent/border/input oklch(0.16 0.01 240) #090e11
mutedForeground oklch(0.65 0.01 240) #8a9095  destructive oklch(0.6 0.2 25) #de3b3d
ring        oklch(0.98 0.01 240) #f3faff
```

Persistence: theme mode → setting key `theme_preference`; custom colours → JSON under
`theme_custom_colors`, both via the Tauri settings API.

### 2.7 Brand accent — Anthropic terracotta

Not a token; hard-coded where it appears.

- **`#d97757`** — primary brand accent (terracotta / clay)
- **`#ff9a7a`** — lighter highlight used mid-gradient
- `rgba(217, 119, 87, 0.35–0.4)` — shimmer sweeps

Used in exactly three places:
1. `.trailing-border::after` — a conic-gradient border that orbits a card on hover.
2. `.shimmer-once` / `.shimmer-hover` / `.brand-text-shimmer` (`src/assets/shimmer.css`) — a diagonal light sweep across text and cards.
3. `UsageDashboard.original.tsx` — bar-chart fill `bg-[#d97757]`.

`.rotating-symbol` in `shimmer.css` also carries a violet **`#8B5CF6`** (the `◐ ◓ ◑ ◒`
spinner). The `styles.css` copy of that class inherits `currentColor` instead — the two
definitions conflict; `shimmer.css` is imported first in `main.tsx`, so `styles.css` wins.

### 2.8 Semantic / status colours (raw Tailwind, outside the token set)

Used mainly by the ticket board (`TaskBoard.tsx`), stream output and status chips. House
pattern for a status pill: **`bg-<hue>-500/15  text-<hue>-400  border-<hue>-500/30`**.

| Meaning | Classes |
|---|---|
| Pending / warning | `amber-400` dot · `bg-amber-500/15 text-amber-400 border-amber-500/30` |
| Approved / info | `blue-400` dot |
| Completed / open / success | `emerald-400` dot · `bg-emerald-500/15 text-emerald-400 border-emerald-500/30` |
| Critical / closed / error | `bg-red-500/15 text-red-400 border-red-500/30` |
| Low / draft / neutral | `bg-slate-500/15 text-slate-400 border-slate-500/30` |
| Running agent | `text-violet-400` + `animate-spin` loader |
| Claude installed / missing | `fill-green-500 text-green-500` / `fill-red-500 text-red-500` |

Inline banners follow: `rounded-lg border border-<hue>-500/30 bg-<hue>-500/10 px-3 py-2 text-sm text-<hue>-400`.

### 2.9 Markdown editor colour bridge (GitHub palette)

`@uiw/react-md-editor` gets its own variable set, keyed off `[data-color-mode]` +
theme class, so the editor matches GitHub rather than the app tokens:

- **Dark:** canvas `rgb(13,17,23)`, subtle `rgb(22,27,34)`, border `rgb(48,54,61)`, fg `rgb(201,209,217)`, muted `rgb(139,148,158)`, accent `rgb(88,166,255)`, danger `rgb(248,81,73)`
- **Light:** canvas `#ffffff`, subtle `rgb(246,248,250)`, border `rgb(216,222,228)`, fg `rgb(31,35,40)`, muted `rgb(101,109,118)`, accent `rgb(9,105,218)`, danger `rgb(207,34,46)`

The editor chrome is then force-overridden back to app tokens (`!important` on toolbar,
content, preview) so only the *content* uses GitHub colours.

---

## 3. Typography

### 3.1 Families

```css
--font-sans: "Inter", -apple-system, BlinkMacSystemFont, "Segoe UI", "Roboto",
             "Oxygen", "Ubuntu", "Cantarell", "Fira Sans", "Droid Sans",
             "Helvetica Neue", sans-serif;
--font-mono: ui-monospace, SFMono-Regular, "SF Mono", Consolas,
             "Liberation Mono", Menlo, monospace;
```

**Inter** is bundled locally as a variable font (`src/assets/fonts/inter/Inter.ttf`,
weight axis `100–900`, `font-display: swap`) — no network fetch, important for an offline
desktop app. Mono is used for paths, versions, code and terminal output.

### 3.2 Scale

| Token | rem | px |
|---|---|---|
| `--text-xs` | 0.75 | 12 |
| `--text-sm` | 0.875 | 14 |
| `--text-base` | 1 | 16 |
| `--text-lg` | 1.125 | 18 |
| `--text-xl` | 1.25 | 20 |
| `--text-2xl` | 1.5 | 24 |
| `--text-3xl` | 1.875 | 30 |
| `--text-4xl` | 2.25 | 36 |
| `--text-5xl` | 3 | 48 |

Weights `100–900` as `--font-weight-*`. Line heights: `none 1`, `tight 1.25`,
`snug 1.375`, `normal 1.5`, `relaxed 1.625`, `loose 1.75`. Tracking: `tighter -0.05em`
→ `widest 0.1em`.

### 3.3 Semantic type utilities

Defined in `styles.css` and used across screens — prefer these over ad-hoc size+weight combos.

| Class | Size / Weight / Leading / Tracking |
|---|---|
| `.text-display-1` | 48px · bold · 1.25 · −0.025em |
| `.text-display-2` | 36px · bold · 1.25 · −0.025em |
| `.text-heading-1` | 30px · semibold · 1.25 · −0.025em |
| `.text-heading-2` | 24px · semibold · 1.375 |
| `.text-heading-3` | 20px · semibold · 1.375 |
| `.text-heading-4` | 18px · medium · 1.5 |
| `.text-body-large` | 18px · normal · 1.625 |
| `.text-body` | 16px · normal · 1.5 |
| `.text-body-small` | 14px · normal · 1.5 |
| `.text-caption` | 12px · normal · 1.5 |
| `.text-label` | 14px · medium · 1.25 · +0.025em |
| `.text-button` | 14px · medium · 1.25 · +0.025em |
| `.text-overline` | 12px · semibold · 1.25 · +0.05em · UPPERCASE |

### 3.4 Practical typography rules

- Page titles: `text-3xl font-bold tracking-tight` (Projects, tab pages); board titles `text-2xl font-bold tracking-tight`.
- Card titles: `font-semibold leading-none tracking-tight`.
- Body/UI default: `text-sm`; metadata and chips: `text-xs`.
- Secondary text is *always* `text-muted-foreground` — by far the most-used class in the codebase (~550 occurrences).
- Placeholders: `--color-muted-foreground` at `opacity: 0.6`.
- Prose (`react-markdown` output): `max-width: 65ch`, `line-height 1.75`; `.prose-sm` at 14px/1.714. Inline code = 0.875em semibold on `--color-muted`, 4px radius; `pre` = `--color-card`, 6px radius, 1px border. Blockquote = 4px left border in `--color-border`, italic.

---

## 4. Shape, Elevation, Spacing

### 4.1 Radii

```css
--radius-sm:   0.25rem  /*  4px */
--radius-base: 0.375rem /*  6px */
--radius-md:   0.5rem   /*  8px */
--radius-lg:   0.75rem  /* 12px */
--radius-xl:   1rem     /* 16px */
```

Observed usage: `rounded-lg` (110×) for cards/panels/banners, `rounded-md` (83×) for
buttons/inputs/menu items, `rounded-full` (60×) for badges, dots and traffic lights,
`rounded-sm` (9×) for tiny icon buttons. **The window itself uses `--radius-lg` (12px)**
on `html`, `body` and `#root`.

### 4.2 Elevation

Deliberately shallow. `shadow-xs` on cards/inputs, `shadow-sm` on default buttons,
`shadow-md` on popovers and tooltips, `shadow-lg` on modals, dropdowns and toasts,
`shadow-2xl` reserved for full-screen overlays. Active tab in `TabsTrigger` uses an
explicit `0 1px 2px rgba(0,0,0,0.1)`.

On macOS the transparent window gets a hairline instead of a shadow:
```css
html.is-macos body { box-shadow: inset 0 0 0 1px var(--color-border); }
```

### 4.3 Spacing rhythm

4px base grid. Dominant values in the codebase:

- **Gaps:** `gap-2` (most common) > `gap-1` > `gap-3` > `gap-4`
- **Padding:** `p-3` and `p-4` for compact blocks; `p-6` for page sections and `Card` header/content/footer; `px-3 py-2` for rows and menu items; `px-4 py-3` for the toolbar and toasts
- **Page container:** `max-w-6xl mx-auto p-6` for tab pages and lists
- **Section spacing:** `mb-6` under a page header, `space-y-1` for dense lists

---

## 5. Window Shell & Chrome

### 5.1 The Tauri window (`src-tauri/tauri.conf.json`)

```json
{ "title": "Dotsquares AI", "width": 800, "height": 600,
  "decorations": false, "transparent": true, "shadow": true,
  "center": true, "resizable": true }
```
plus `"macOSPrivateApi": true` (required for a genuinely transparent macOS window).

Because there is no OS frame, the CSS does the framing:

```css
html, body { background-color: rgba(0,0,0,0); }         /* let the window be transparent */
html { border-radius: var(--radius-lg); overflow: hidden;
       clip-path: inset(0 round var(--radius-lg)); }     /* clips fixed/backdrop-filter kids */
body { border-radius: var(--radius-lg); overflow: hidden;
       background-color: var(--color-background); }
#root { height:100%; width:100%; border-radius: inherit; overflow: hidden; }
```

Drag regions are opt-in helpers: `.tauri-drag { -webkit-app-region: drag }` and
`.tauri-no-drag { -webkit-app-region: no-drag }`.

### 5.2 App layout skeleton

```
<div class="h-screen flex flex-col">          ← App root
  ├── <CustomTitlebar />                      ← 44px, fixed height, z-[200]
  └── <div class="flex-1 overflow-hidden">    ← content region
        └── <div class="h-full flex flex-col">
              ├── <TabManager class="flex-shrink-0" />   ← 32px tab strip
              └── <div class="flex-1 overflow-hidden"><TabContent /></div>
            </div>
      </div>
  + <NFOCredits> / <ClaudeBinaryDialog> / FilePicker overlay / <ToastContainer>
</div>
```

There is **no persistent sidebar and no bottom status bar.** Vertical space is: titlebar →
tab strip → scrollable content. Individual tabs may render their own internal header row
and split panes (`src/components/ui/split-pane.tsx`).

### 5.3 Custom titlebar (`CustomTitlebar.tsx`) — the "navbar"

```
h-11 (44px) · bg-background/95 · backdrop-blur-sm · border-b border-border/50
z-[200] · select-none · tauri-drag · data-tauri-drag-region
flex items-center justify-between
```

- **Left (`pl-5`, `space-x-2`):** macOS traffic lights, hand-drawn — three `w-3 h-3 rounded-full` buttons: close `bg-red-500`/`hover:bg-red-600`, minimize `bg-yellow-500`/`600`, maximize `bg-green-500`/`600`. Glyphs (`X`, `Minus`, `Square` at 6–8px, `opacity-60 → 100`) appear only while the titlebar is hovered. Each is `tauri-no-drag`, and calls `getCurrentWindow().close/minimize/maximize`.
- **Centre:** title is intentionally commented out — the bar reads as empty space and doubles as the drag handle.
- **Right (`pr-5`, `gap-3`):** two icon groups separated by a `w-px h-5 bg-border/50` divider.
  - Group 1: Agents (`Bot`), Usage Dashboard (`BarChart3`)
  - Group 2: Settings (`Settings`), overflow (`MoreVertical`)
  - Icon buttons: `p-2 rounded-md hover:bg-accent hover:text-accent-foreground transition-colors`, 16px lucide icons, `whileTap={{ scale: 0.97 }}` at 0.15s, wrapped in `TooltipSimple side="bottom"`.
- **Overflow dropdown:** `absolute right-0 mt-2 w-48 bg-popover border border-border rounded-lg shadow-lg z-[250]`, items `w-full px-4 py-2 text-left text-sm hover:bg-accent`, 14px icon + `gap-3` — CLAUDE.md (`FileText`), MCP Servers (`Network`), About (`Info`). Closes on outside `mousedown`.

**Legacy:** `Topbar.tsx` still exists (a 2nd bar with the Claude-version status dot,
`px-4 py-3 border-b bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/60`,
sliding in from `y:-20` over 0.3s) but it is commented out in `App.tsx` — navigation moved
into the titlebar. Keep it as the reference for the status-indicator pattern.

### 5.4 Tab strip (`TabManager.tsx`)

Strip: `flex items-stretch bg-muted/15 border-b border-border/50`, row height `h-8`,
horizontally scrollable with `scrollbar-hide`, chevron scroll buttons at each end and
edge fade masks (`w-8 bg-gradient-to-r from-muted/15 to-transparent`). A `+` button
(`Plus`, 16px) appends a tab. Tabs are draggable/reorderable via Framer Motion
`Reorder.Item` (0.1s transition).

Tab item:
```
min-w-[120px] max-w-[220px] h-8 px-3 · gap-2 · text-sm · border-r border-border/20
before:absolute before:bottom-0 before:h-0.5     ← 2px active underline
active   → bg-card text-card-foreground before:bg-primary
inactive → bg-transparent text-muted-foreground
           hover:bg-muted/40 hover:text-foreground before:bg-transparent
dragging → bg-card border-primary/50 shadow-sm z-50
```
Contents: 16px lucide icon → `flex-1 truncate text-xs font-medium` title → status slot
(`w-6`, spinner `Loader2 animate-spin` / `AlertCircle text-red-500` / a
`w-1.5 h-1.5 bg-primary rounded-full` unsaved dot) → close button
(`w-4 h-4 rounded-sm hover:bg-destructive/20 hover:text-destructive`, `opacity-0` until
hover or active).

### 5.5 Z-index ladder

| Layer | z |
|---|---|
| Titlebar dropdown | `z-[250]` |
| Custom titlebar | `z-[200]` |
| Tooltips | `z-[100]` |
| Startup intro overlay | `z-[60]` |
| Modals, dialogs, popovers, toasts, dragged tab | `z-50` |
| Tab-strip scroll buttons / sticky headers | `z-40`, `z-30` |
| Card content over `.trailing-border` glow | `z-1` (glow at `z-index: -1`) |

---

## 6. Component Styling Recipes

### 6.1 Button (`ui/button.tsx`, CVA)

Base: `inline-flex items-center justify-center whitespace-nowrap rounded-md text-sm font-medium transition-colors disabled:pointer-events-none disabled:opacity-50`

| Variant | Classes |
|---|---|
| `default` | `bg-primary text-primary-foreground shadow hover:bg-primary/90` |
| `destructive` | `bg-destructive text-destructive-foreground shadow-xs hover:bg-destructive/90` |
| `outline` | `border border-input bg-background shadow-xs hover:bg-accent hover:text-accent-foreground` |
| `secondary` | `bg-secondary text-secondary-foreground shadow-xs hover:bg-secondary/80` |
| `ghost` | `hover:bg-accent hover:text-accent-foreground` |
| `link` | `text-primary underline-offset-4 hover:underline` |

| Size | Classes |
|---|---|
| `default` | `h-9 px-4 py-2` |
| `sm` | `h-8 rounded-md px-3 text-xs` |
| `lg` | `h-10 rounded-md px-8` |
| `icon` | `h-9 w-9` |

Dense contexts (ticket cards, banners) use ad-hoc `h-7 gap-1 px-2 text-xs`.

### 6.2 Card

`rounded-lg border shadow-xs` + inline styles binding `--color-border` / `--color-card` /
`--color-card-foreground`. Sections: `CardHeader` `flex flex-col space-y-1.5 p-6`,
`CardTitle` `font-semibold leading-none tracking-tight`, `CardDescription`
`text-sm text-muted-foreground`, `CardContent` `p-6 pt-0`, `CardFooter`
`flex items-center p-6 pt-0`.

> Several primitives (Card, Input, Tabs) set colours through inline `style` with
> `var(--color-*)` instead of Tailwind classes. That's intentional — it guarantees the
> runtime custom theme applies — so follow the same pattern when adding primitives that
> must respect `.theme-custom`.

### 6.3 Input / Textarea

`flex h-9 w-full rounded-md border px-3 py-1 text-sm shadow-sm transition-colors`,
transparent background, `borderColor: var(--color-input)`, `color: var(--color-foreground)`,
`disabled:opacity-50 disabled:cursor-not-allowed`.

### 6.4 Badge

`inline-flex items-center rounded-full border px-2.5 py-0.5 text-xs font-semibold transition-colors`
· variants `default` (primary), `secondary`, `destructive` — all `border-transparent` with
`hover:bg-*/80` — and `outline` (`text-foreground`).

### 6.5 Dialog (Radix)

Overlay `fixed inset-0 z-50 bg-black/80` with fade in/out.
Content `fixed left-[50%] top-[50%] translate-x-[-50%] translate-y-[-50%] z-50 grid w-full max-w-lg gap-4 border bg-background p-6 shadow-lg sm:rounded-lg`,
animating `fade + zoom-95 + slide-from-top-[48%]` over `duration-200`.
Close button `absolute right-4 top-4 rounded-sm opacity-70 hover:opacity-100`, 16px `X`.
Title `text-lg font-semibold leading-none tracking-tight`; description `text-sm text-muted-foreground`.
Footer `flex flex-col-reverse sm:flex-row sm:justify-end sm:space-x-2` — destructive/confirm on the right.

Ad-hoc modals (e.g. the FilePicker) use the same language by hand:
`fixed inset-0 z-50 flex items-center justify-center bg-background/80 backdrop-blur-sm`
around a `max-w-2xl h-[600px] bg-background border rounded-lg shadow-lg` panel.

### 6.6 Popover / Tooltip / Dropdown

- **Popover:** `absolute z-50 min-w-[200px] rounded-md border border-border bg-popover p-4 text-popover-foreground shadow-md`
- **Tooltip** (`tooltip-modern.tsx`): `z-[100] overflow-hidden rounded-lg border border-border bg-popover px-3 py-2 text-xs text-popover-foreground shadow-md`, `sideOffset={6}`
- **Dropdown items:** `w-full px-4 py-2 text-left text-sm hover:bg-accent hover:text-accent-foreground transition-colors flex items-center gap-3`

### 6.7 Tabs (in-page, `ui/tabs.tsx`)

`TabsList`: `flex h-9 items-center justify-start rounded-lg p-1` on `--color-muted`, text `--color-muted-foreground`.
`TabsTrigger`: `inline-flex items-center rounded-md px-3 py-1 text-sm font-medium transition-all`;
selected → background `--color-background`, colour `--color-foreground`, `box-shadow: 0 1px 2px rgba(0,0,0,0.1)`.
`TabsContent`: `mt-2`.

### 6.8 Toast

`flex items-center space-x-3 rounded-lg border border-border bg-card px-4 py-3 shadow-lg`,
16px `CheckCircle` / `AlertCircle` / `Info` in success/error/info colour, message `text-sm`,
dismiss `X` in `text-muted-foreground hover:text-foreground`.
Container: `fixed bottom-0 left-0 right-0 z-50 flex justify-center p-4 pointer-events-none` —
**toasts are bottom-centre.**

### 6.9 List rows (Projects, Sessions)

`w-full text-left px-3 py-2 rounded-md hover:bg-accent/50 transition-colors flex items-center justify-between`,
label `text-body-small font-medium`, right-aligned metadata `text-caption text-muted-foreground font-mono`.
Rows sit in `space-y-1` inside a `Card p-6`; empty state is a `Card p-12`.

### 6.10 Ticket board card (`TaskBoard.tsx`)

Column headers pair a label with a status dot (`bg-amber-400` / `bg-blue-400` /
`bg-emerald-400`). A ticket card: title `text-sm font-semibold leading-snug`, meta row
`mt-2 text-xs text-muted-foreground`, priority/status pills using the `/15 · 400 · /30`
recipe, external links `flex items-center gap-1 text-muted-foreground hover:text-foreground`
with a 10px `ExternalLink`, destructive delete revealed on hover
(`opacity-0 group-hover:opacity-100 hover:text-red-400`), and a `h-7 px-2 text-xs` action
button with a `Sparkles` / `Loader2 animate-spin` icon.

### 6.11 Icons

`lucide-react` throughout. Sizes: **14px** inside menus/chips, **16px** (`h-4 w-4`) for
standard buttons and titlebar actions, **12px** (`h-3 w-3`) for inline status,
**24px** (`h-6 w-6`) for large loaders. Stroke inherits `currentColor`.

---

## 7. Motion

| Purpose | Spec |
|---|---|
| House duration | **0.15s** (56 occurrences) — button taps, tab hovers, icon presses |
| Secondary | 0.2s (menus), 0.3s (bar entrances), 0.35s (overlay fade) |
| CSS transitions | `duration-200` most common, then `duration-100` for tab hover |
| Easing tokens | `--ease-smooth: cubic-bezier(0.4, 0, 0.2, 1)` · `--ease-bounce: cubic-bezier(0.68, -0.55, 0.265, 1.55)` |
| Tap feedback | `whileTap={{ scale: 0.97 }}` |
| Entrance | `initial={{opacity:0, y:-20}} → {opacity:1, y:0}`, 0.3s |
| Radix enter/exit | `animate-in` / `animate-out` utilities, 150ms, `both` fill |

Named animations in `styles.css` / `shimmer.css`:

| Keyframes / class | Effect |
|---|---|
| `enter` / `exit` | Generic Radix-style opacity + translate + scale + rotate |
| `rotate-symbol` + `.rotating-symbol` | `◐ ◓ ◑ ◒` quarter-circle spinner, `1.6s steps(4)` infinite, 1.5rem |
| `shimmer` / `.shimmer-hover` | Diagonal light sweep on hover (white 5% in `styles.css`, terracotta 40% in `shimmer.css`) |
| `shimmer-text` / `.shimmer-once` | One-shot terracotta sweep across text |
| `shimmer-overlay` / `.brand-text-shimmer` | Layered brand-text sweep that fades out (avoids flicker) |
| `trail-rotate` + `.trailing-border` | Conic-gradient terracotta border orbiting a card on hover, 2s linear, via `@property --angle` and a mask-composite ring |
| `scanlines` / `.animate-scanlines` | CRT scanline drift, 8s linear infinite (NFO credits) |
| `shutterFlash` / `.shutter-flash` | 0.5s screenshot flash |
| `moveToInput` / `.image-move-to-input` | 0.8s scale-down + drop of a captured image into the prompt box |
| `fade-in` | 0.2s opacity + scale 0.8→1 |

### Startup intro (`StartupIntro.tsx`)

Full-screen `fixed inset-0 z-[60] bg-background` overlay, ~2s, dismissed by a setting
(`startup_intro_enabled`, cached in `localStorage` to avoid a flash). Layers: a radial
`--color-primary`/8 glow at 50%/55%, a vignette
`radial-gradient(1200px circle at 50% 40%, transparent 60%, rgba(0,0,0,0.25))`, the
Dotsquares wordmark (`h-14`) sliding left with a `bg-primary/15 blur-2xl` halo, and the
word "AI" (`text-5xl font-extrabold tracking-tight`) revealed left-to-right via
`clip-path: inset(0 100% 0 0) → inset(0 0% 0 0)` with the terracotta shimmer overlay on top.

---

## 8. Branding Assets

- **Product name:** "Dotsquares AI" (`tauri.conf.json` `productName`, window title, `<title>`); binary name remains `opcode`; bundle id `opcode.asterisk.so`.
- **Wordmark:** `src/assets/dots-logo.svg` (light ink, for dark themes) and `dots-logo-ink.svg` (dark ink, for the light theme). Swapping is CSS-driven:
  ```css
  .brand-logo-on-light { display: none; }
  .theme-light .brand-logo-on-light { display: block; }
  .theme-light .brand-logo-on-dark  { display: none; }
  ```
  Note this only covers `.theme-light` — `.theme-white`, if ever enabled, would need the same rule.
- **Favicon:** injected at runtime in `main.tsx` from `dots-logo.svg` (no `/public` dir).
- **App icons:** full Tauri icon set in `src-tauri/icons/` (macOS `.icns`, Windows `.ico`, PNG 32→512, iOS + Android mipmaps).
- **DMG layout:** 540×380 window, app at (140, 200), Applications at (400, 200).

---

## 9. Global Behaviours & Caveats

**Universal border colour.** `* { border-color: var(--color-border) }` — any `border`
utility picks up the theme hairline without an explicit colour class.

**Focus styling is globally removed.** This is the most significant deviation from the
shadcn baseline:

```css
* { outline: none !important; outline-offset: 0 !important; }
*:focus, *:focus-visible, *:focus-within { outline: none !important; box-shadow: none !important; }
input:focus, textarea:focus, button:focus, [role="combobox"]:focus, … {
  outline: none !important; box-shadow: none !important;
  border-color: var(--color-input) !important;
}
.ring, .ring-0…2, .ring-offset-* { box-shadow: none !important; }
```

`--color-ring` is therefore defined but effectively unused. **Keyboard-only users get no
visible focus indicator** — if accessibility is ever in scope, this block is the first
thing to revisit (replace with a `:focus-visible`-only ring rather than removing it).

**Cursor policy.** `button, a, [role=button|link|menuitem|tab], [tabindex]:not([tabindex="-1"]), .cursor-pointer` → `cursor: pointer`; `:disabled, [disabled], .disabled` → `cursor: not-allowed !important`.

**Scrollbars — three tiers:**

| Tier | Spec |
|---|---|
| Global | 3px wide, transparent track, thumb `rgba(156,163,175,0.5)` → `0.6` on hover, 2px radius; Firefox `scrollbar-width: thin` with `rgba(156,163,175,0.3)` |
| Code / editor (`pre`, `code`, `.w-md-editor-content`) | 8px, thumb `rgba(156,163,175,0.4)` → `0.6`, 4px radius |
| Utilities | `.scrollbar-hide` (fully hidden, used by the tab strip) · `.scrollbar-thin` (6px, `--color-border` thumb → `--color-muted-foreground` on hover) |

**`color-scheme: dark`** is set on `html` in CSS *and* via the meta tag — native form
controls stay dark even in the light theme. Worth overriding per-theme if light mode
matters.

**Line clamping:** custom `@utility line-clamp-2`.

---

## 10. Conventions for New Work

1. **Use semantic tokens, never raw greys.** `bg-card`, `text-muted-foreground`, `border-border` — not `bg-gray-800`. Raw Tailwind hues are only for *status* meaning (§2.8).
2. **Remember `primary` == foreground.** For a coloured CTA you need the brand terracotta explicitly; a `default` Button is a high-contrast neutral.
3. **Reach for the type utilities** (`.text-heading-3`, `.text-caption`, `.text-label`) before hand-rolling size + weight.
4. **Shape:** `rounded-lg` for containers, `rounded-md` for controls, `rounded-full` for pills/dots.
5. **Hover = `hover:bg-accent hover:text-accent-foreground`** for icon buttons and menu items; `hover:bg-accent/50` for list rows; `hover:bg-muted/40` for tabs.
6. **Motion:** 0.15s + `--ease-smooth` unless there's a reason. `whileTap={{scale:0.97}}` on icon buttons.
7. **New primitives that must respect the custom theme** should bind colours via inline `style={{ …: "var(--color-*)" }}`, matching Card/Input/Tabs.
8. **Status pills:** `bg-<hue>-500/15 text-<hue>-400 border-<hue>-500/30`. **Banners:** `rounded-lg border border-<hue>-500/30 bg-<hue>-500/10 px-3 py-2 text-sm text-<hue>-400`.
9. **Anything interactive in the titlebar needs `tauri-no-drag`**, or the click becomes a window drag.
10. **Adding a theme** = add a `.theme-<name>` block in `styles.css` overriding the full token set, extend `ThemeMode` in `ThemeContext.tsx`, and add a swatch button in `Settings.tsx`.

---

## 11. Known Inconsistencies (worth cleaning up)

| Issue | Detail |
|---|---|
| `.theme-white` is unreachable | Fully styled in CSS but absent from `ThemeMode`, `Settings.tsx` and the brand-logo swap rule. |
| Duplicate `.rotating-symbol` / `@keyframes shimmer` | Defined in both `shimmer.css` and `styles.css` with different colours (`#8B5CF6` vs `currentColor`) and different animation semantics. `styles.css` wins by import order. |
| Two navigation bars | `Topbar.tsx` is fully maintained but commented out of `App.tsx`. |
| Brand terracotta isn't a token | `#d97757` / `#ff9a7a` are hard-coded in three files; promoting them to `--color-brand` / `--color-brand-light` would make them themeable. |
| Duplicate FilePicker overlay | The same modal block appears twice in `App.tsx`'s JSX. |
| `--color-ring` is dead | Defined per theme, then suppressed by the global focus reset. |
| `color-scheme: dark` is unconditional | Native controls stay dark in the light/white themes. |
