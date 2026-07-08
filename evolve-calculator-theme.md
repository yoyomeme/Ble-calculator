# Evolve-Inspired Desktop Calculator Theme

Source reviewed: `https://evolve.enclavelabs.org`

## Extracted Website Theme

The site has a warm, high-energy food and fitness identity. It combines oversized condensed display type, casual handwritten accents, cream backgrounds, charcoal overlays, and saturated orange highlights. The mood is bold, fresh, and tactile rather than sterile or purely technical.

### Core Palette

```css
:root {
  --evolve-brick: #c8401a;
  --evolve-cream: #f0dcc8;
  --evolve-cream-soft: #f7ecdf;
  --evolve-char: #1a1614;
  --evolve-char-soft: #2a2320;
  --evolve-orange: #ff6a1a;
  --evolve-leaf: #6b8e3d;
  --evolve-yolk: #f2b441;
}
```

### Type System

- Display: `Anton`, fallback `Archivo Black`, used for huge uppercase hero and section titles.
- Script accent: `Caveat`, used sparingly for warm handwritten callouts.
- Brand and controls: `Poppins`, used for uppercase labels, buttons, and compact UI text.
- Body and dense content: `Inter`, used for readable supporting copy.
- Editorial serif: `Fraunces`, available but not central in the visible UI.

### Visual Traits

- Cream and soft cream page backgrounds.
- Charcoal foregrounds and dark image overlays.
- Orange primary action color, with yolk hover/accent states.
- Brick red and leaf green as secondary accent colors.
- Large, compressed uppercase headings with tight line-height.
- Rounded media panels and pill-shaped primary CTAs.
- High-contrast overlays: charcoal gradient over visual content.
- Motion language: reveal-on-scroll, subtle scale on hover, horizontal momentum sections.

## Calculator Theme Direction

The calculator should feel like a focused desktop utility wearing the Evolve identity. Use the warmth and contrast of the website, but reduce the marketing scale. The result should be functional first: clear numeric hierarchy, strong operator distinction, and stable key sizing.

Recommended product name: `Evolve Calc`

Theme mood: warm, bold, protein-label precision, dark-workbench shell.

## Desktop Layout

### Window

- Recommended size: `420px` wide by `620px` high.
- Background: charcoal app shell using `--evolve-char`.
- Outer radius: `8px` for desktop utility restraint.
- Use a thin `1px` border in `rgba(240, 220, 200, 0.16)`.
- Shadow: deep, soft, warm black shadow such as `0 24px 60px rgba(26, 22, 20, 0.35)`.

### Header

- Height: `48px`.
- Left: small circular mark or text label `Evolve Calc`.
- Label font: `Poppins`, `12px`, uppercase, `600`, letter spacing `0.16em`.
- Header color: cream text at 70 percent opacity.
- Optional status chip: `STANDARD`, `SCIENTIFIC`, or `HISTORY`, styled as a compact cream outline pill.

### Display Area

- Background: `--evolve-cream-soft`.
- Text: `--evolve-char`.
- Radius: `8px`.
- Padding: `20px 24px`.
- Height: `128px` minimum.
- Expression line: `Inter`, `14px`, char at 60 percent opacity.
- Main result: `Poppins` or `Inter`, `48px`, `700`, tabular numbers.
- Use `font-variant-numeric: tabular-nums;` for all numeric display.
- Avoid Anton for result digits; it is strong for branding but weak for dense calculator reading.

### Keypad

- Grid: `4 columns`, `6 rows` for standard calculator.
- Gap: `10px`.
- Key size: stable `72px` by `56px`; no resizing on hover or active state.
- Radius: `8px`.
- Key labels: `Poppins`, `18px`, `700`, tabular numbers.

## Key Roles

### Number Keys

- Background: `rgba(240, 220, 200, 0.10)` on charcoal shell.
- Text: `--evolve-cream`.
- Hover: `rgba(240, 220, 200, 0.16)`.
- Active: inset shadow plus slight brightness lift.

### Operator Keys

- Background: `--evolve-orange`.
- Text: `--evolve-char`.
- Hover: `--evolve-yolk`.
- Use for `+`, `-`, `x`, `÷`, `%`.

### Equals Key

- Background: `--evolve-yolk`.
- Text: `--evolve-char`.
- Recommended layout: double-height key on the right column.
- Active state: `--evolve-orange`.

### Clear and Delete

- `AC`: brick background, cream text.
- `DEL`: charcoal-soft background, cream text, cream border at 14 percent opacity.
- Hover for destructive actions: deepen toward `--evolve-brick`.

### Memory and Utility Keys

- Use `--evolve-leaf` for memory-confirming actions like `M+`, `MR`, or successful copied result.
- Use cream outline styles for secondary utility controls.

## Suggested CSS Tokens

```css
:root {
  --calc-bg: #1a1614;
  --calc-panel: #2a2320;
  --calc-display: #f7ecdf;
  --calc-display-alt: #f0dcc8;
  --calc-text: #f0dcc8;
  --calc-text-dark: #1a1614;
  --calc-muted: rgba(240, 220, 200, 0.62);
  --calc-border: rgba(240, 220, 200, 0.16);

  --calc-primary: #ff6a1a;
  --calc-primary-hover: #f2b441;
  --calc-danger: #c8401a;
  --calc-success: #6b8e3d;

  --calc-radius: 8px;
  --calc-gap: 10px;
  --calc-shadow: 0 24px 60px rgba(26, 22, 20, 0.35);

  --font-display: "Anton", "Archivo Black", sans-serif;
  --font-control: "Poppins", system-ui, sans-serif;
  --font-body: "Inter", system-ui, sans-serif;
}
```

## Component CSS Sketch

```css
.calculator {
  width: 420px;
  min-height: 620px;
  background: var(--calc-bg);
  color: var(--calc-text);
  border: 1px solid var(--calc-border);
  border-radius: var(--calc-radius);
  box-shadow: var(--calc-shadow);
  padding: 18px;
  font-family: var(--font-body);
}

.calculator__brand {
  font-family: var(--font-control);
  font-size: 12px;
  font-weight: 700;
  letter-spacing: 0.16em;
  text-transform: uppercase;
  color: var(--calc-muted);
}

.calculator__display {
  min-height: 128px;
  background: var(--calc-display);
  color: var(--calc-text-dark);
  border-radius: var(--calc-radius);
  padding: 20px 24px;
  display: flex;
  flex-direction: column;
  justify-content: flex-end;
  gap: 8px;
}

.calculator__expression {
  font: 500 14px/1.3 var(--font-body);
  color: rgba(26, 22, 20, 0.58);
  font-variant-numeric: tabular-nums;
}

.calculator__result {
  font: 700 48px/1 var(--font-control);
  font-variant-numeric: tabular-nums;
  text-align: right;
}

.calculator__keys {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: var(--calc-gap);
  margin-top: 16px;
}

.key {
  height: 56px;
  border-radius: var(--calc-radius);
  border: 1px solid transparent;
  background: rgba(240, 220, 200, 0.10);
  color: var(--calc-text);
  font: 700 18px/1 var(--font-control);
  font-variant-numeric: tabular-nums;
  cursor: pointer;
  transition: background-color 160ms ease, color 160ms ease, box-shadow 160ms ease;
}

.key:hover {
  background: rgba(240, 220, 200, 0.16);
}

.key:active {
  box-shadow: inset 0 2px 8px rgba(0, 0, 0, 0.24);
}

.key--operator {
  background: var(--calc-primary);
  color: var(--calc-text-dark);
}

.key--operator:hover {
  background: var(--calc-primary-hover);
}

.key--equals {
  background: var(--calc-primary-hover);
  color: var(--calc-text-dark);
  grid-row: span 2;
  height: 122px;
}

.key--danger {
  background: var(--calc-danger);
  color: var(--calc-text);
}

.key--utility {
  background: var(--calc-panel);
  border-color: var(--calc-border);
  color: var(--calc-text);
}
```

## Interaction Notes

- Use quick, restrained transitions around `160ms`.
- Press states should use inset shadow, not layout movement.
- Hover may change color, but key dimensions must remain fixed.
- Use orange focus outlines on keyboard navigation: `2px solid #ff6a1a` with `3px` offset.
- Add a compact copied-result toast using leaf green, then fade it out.

## What Not To Copy From The Site

- Do not use huge hero typography inside the calculator body.
- Do not use food imagery as a calculator background.
- Do not use long scroll animation patterns in a desktop utility.
- Do not use handwritten script for numbers, operators, or critical state labels.
- Do not let decorative branding reduce display readability.

