# Desktop UI visual standards

This document defines layout and interaction invariants for the Camellia Nexus desktop UI. It does
not require the sibling administration console to share desktop styling. Cupertino, Material, and
Aurora retain their own density, radii, surfaces, typography, and motion through theme tokens.

## Controls

- Use the shared control tokens and components. Page-local vertical padding must not compensate for
  the geometry of a shared control.
- Within one theme and control size, inputs and selects have one block size and one optical center.
  `md` is for compact toolbars and dense rows; `lg` is for standard forms.
- A select declares its control typography instead of inheriting incidental label or toolbar weight.
  Centered closed values use control geometry as well as text alignment; start alignment is reserved
  for long entity values.
- A short enumerated value is centered. A long entity value, path-like label, identity, or compound
  name is start-aligned. The closed select and its picker use the same alignment.
- Application-rendered single-select options use one text column. They do not show a checkmark or
  reserve leading and trailing mirror columns. Selection is communicated by the theme's selected
  surface, text color, font weight, and the native selected state.
- A native picker used on touch devices or unsupported WebViews may retain operating-system chrome.
- Genuine booleans and multi-select collections remain checkboxes. Do not make a checkbox look like
  a command button merely to align it with adjacent actions.

## Fields and text

- Field labels, controls, help text, and errors use shared gaps. Metadata such as `(Optional)` is
  inline, non-italic, tertiary, and separated from the primary label by the metadata gap.
- Placeholders and examples use regular weight and lower emphasis than entered values. They never
  carry required instructions or become the only accessible label.
- Semantic versions use the shared version-value treatment. Dates use body text, opaque identifiers
  use compact monospace text, and state uses status components.
- Editable paths and URLs remain single-line inputs. When a value is visually truncated, an
  overflow-aware field exposes the complete value on hover and keyboard focus without changing the
  row height or stealing focus.
- Examples use normal single spacing around separators, for example
  `config.json · /etc/proxy/config.json`.

## Icons and actions

- Text-bearing controls use a fixed icon box, block-level SVG, a non-shrinking icon, and the shared
  icon-to-text gap.
- Icons in one navigation group share an optical artboard and stroke weight. Correct the source path
  instead of adding page-local negative margins or transforms.
- Icon-only buttons, vertically stacked illustrations, status symbols, and program artwork are
  explicit exceptions and require an accessible name.
- Separate filters from commands. A section heading may place primary and secondary actions together,
  while list filters and usage metadata belong to a distinct contextual row.
- A transient status beside a command uses the command row's optical center and reserves a stable
  readable region; it must not jump above the adjacent button or shift the command between states.

## Responsive and conditional states

- Re-evaluate every surface at all supported UI scales, in Chinese and English, and in all three
  themes and color modes. Text zoom must wrap or stack controls rather than clip primary actions.
- Permission, loading, empty, error, conflict, retry, secret-display, and read-only states must not
  leave unexplained empty columns or move unrelated controls.
- Team surfaces must be checked for every role-derived view. UI visibility reflects service state but
  never substitutes for service authorization.
- Keep keyboard traversal, focus restoration, visible focus, reduced motion, screen-reader labels,
  and non-color-only meaning intact.

## Review checklist

1. Confirm that the control uses a shared size and has no local vertical compensation.
2. Confirm that alignment follows value semantics rather than page preference.
3. Test short and long Chinese and English values, disabled and busy states, and the native fallback.
4. Verify compact laptop and minimum window widths with no document-level horizontal overflow.
5. Add or update Playwright geometry, accessibility, and representative screenshot coverage.
