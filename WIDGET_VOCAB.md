# `nopeek_widgets` — Vocabulary Reference

Single-file reference for app developers and AI code-generators. Build
GUI apps for nopeekOS from the prefabs and modifiers below; never reach
for raw pixels, fonts, or RGB values.

The vocabulary mirrors Tailwind/shadcn at the conceptual level: a small
spacing/radius/elevation scale, a tokenized colour palette, pseudo-state
modifiers (`Hover`/`Focus`/`Active`/`Disabled`), container queries via
`WhenDensity`. AI familiar with that pattern will produce idiomatic
nopeekOS UI on the first try.

---

## App lifecycle

```rust
#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let mut state = MyApp::load();
    commit_tree(&state);                      // first render
    loop {
        match poll_event() {
            PollResult::Event(ev) => match handle(&mut state, ev) {
                Outcome::Idle      => {}
                Outcome::Rerender  => commit_tree(&state),
                Outcome::Exit      => { close_self(); return; }
            },
            PollResult::Empty      => unsafe { let _ = npk_sleep(16); },
            PollResult::WindowGone => return,
        }
    }
}
```

The app builds a fresh `Widget` tree on every `commit_tree`, serializes
it via `wire::encode`, and ships the bytes through `npk_scene_commit`.
The compositor diffs and re-rasterizes only what changed.

---

## Layout containers

| Widget   | Use                                              |
|----------|--------------------------------------------------|
| `Column` | Stack children vertically with `spacing` + `align` |
| `Row`    | Same, horizontal                                 |
| `Stack`  | Z-order overlay (children share rect)            |
| `Scroll` | Clip + scroll a child along `axis`               |

`Spacer { flex }` inside a `Row`/`Column` eats remaining main-axis
space — flex weights distribute pro-rata.

`Align::Start | Center | End | Stretch` controls cross-axis placement.

---

## Leaves

| Widget     | Notes                                                          |
|------------|----------------------------------------------------------------|
| `Text`     | `style: TextStyle` picks weight + size                         |
| `Icon`     | `id: IconId`, `size`. Atlas-native sizes: 16/24/32/48/64 px    |
| `Button`   | Hardcoded label + optional icon + `on_click: ActionId`         |
| `Input`    | Text field; renders value or placeholder. App routes editing.  |
| `Checkbox` | Bool toggle; `on_toggle: ActionId`                             |
| `Divider`  | Single-pixel `Border`-token line                               |
| `Canvas`   | Reserved — app-supplied BGRA pixels (P10.10, not yet enabled)  |

`Widget::Input` is **not yet self-editing** — the compositor renders
the static value, the app handles `Event::Key(_)` and rebuilds the
tree with the new string.

---

## Modifiers — the styling vocabulary

### Core (always render)

| Modifier                            | Effect                                          |
|-------------------------------------|-------------------------------------------------|
| `Padding(u16)`                      | Inner padding (px). Use `Padding::Md.as_u16()`. |
| `Margin(u16)`                       | Reserved — currently logged but not applied.    |
| `Background(Token)`                 | Filled rect under the widget.                   |
| `Border { token, width, radius }`   | Stroked rounded rect on top.                    |
| `Rounded(u8)`                       | Outer corner radius without a stroke. Wins over Border's radius. |
| `Opacity(u8)`                       | 0..=255 post-paint dampening.                   |
| `Tint(Token)`                       | Re-colour an Icon (default OnSurface).          |

### Layout

| Modifier        | Effect                                                  |
|-----------------|---------------------------------------------------------|
| `MinWidth(u16)` | Clamp intrinsic width up.                               |
| `MaxWidth(u16)` | Clamp intrinsic width down.                             |
| `Flex(u8)`      | CSS-style flex-grow on the parent Row/Column main axis. Widget keeps its intrinsic size as a basis and absorbs a proportional share of leftover space alongside any `Spacer { flex }` siblings. `Flex(0)` = no flex. |

### Interactive states

| Modifier                                | Trigger                                                  |
|-----------------------------------------|----------------------------------------------------------|
| `Hover(Vec<Modifier>)`                  | Cursor over this node *or any descendant* (CSS `:hover`).|
| `Focus(Vec<Modifier>)`                  | Tab-nav landed here, or click-to-focus.                  |
| `Active(Vec<Modifier>)`                 | Mouse-button held down on this node.                     |
| `Disabled(Vec<Modifier>)`               | Presence-based: this widget is disabled. Eats clicks + hovers. Wins over Hover/Focus/Active. |
| `WhenDensity(Density, Vec<Modifier>)`   | Window size bucket matches (`Compact` <600 px, `Regular` 600–1200, `Spacious` >1200). |

State mods carry a nested `Vec<Modifier>` — those inner mods apply only
when the state is active. The wire format stays static across state
changes; the compositor merges at render time.

### Click / hover routing

| Modifier               | Effect                                                      |
|------------------------|-------------------------------------------------------------|
| `OnClick(ActionId)`    | Fires `Event::Action(id)` on left-click.                    |
| `OnHover(ActionId)`    | Fires `Event::Action(id)` when hover target changes to here.|

### Reserved — wire-frozen but no-op in v1

`Blur`, `Shadow`, `Effect`, `Scale`, `Transition`, `RoleOverride`. They
serialize and round-trip cleanly so apps can use them; the CPU
rasterizer ignores them today. Don't rely on visual effect.

---

## Tokens

### `Token` (palette indices)

```
Surface | SurfaceElevated | SurfaceMuted
OnSurface | OnSurfaceMuted | OnAccent
Accent | AccentMuted
Border | Success | Warning | Danger
```

The compositor resolves these against the active palette
(`theme dark|light|auto` + wallpaper-derived accent). Apps never
specify hex values.

### Spacing scale (all `u16` px at 1× HiDPI)

| Token         | px   | Token         | px   |
|---------------|-----:|---------------|-----:|
| `Spacing::None` | 0  | `Spacing::Lg`   | 16   |
| `Spacing::Xxs`  | 2  | `Spacing::Xl`   | 24   |
| `Spacing::Xs`   | 4  | `Spacing::Xxl`  | 32   |
| `Spacing::Sm`   | 8  |               |      |
| `Spacing::Md`   | 12 |               |      |

Same scale for `Padding::*` (omits `Xxl`).

### Radius scale (`u8`)

```
Radius::None=0  Sm=4  Md=8  Lg=12  Xl=16  Pill=255
```

### Elevation (semantic shadow tier — currently visual no-op)

```
Elevation::Flat | Subtle | Raised | Floating
```

### Motion (transition duration token)

```
Motion::Instant=0  Quick=120  Normal=200  Slow=400  (ms)
```

`Motion::Quick.as_transition()` builds a `Transition::Linear { ms: 120 }`
ready for `Modifier::Transition(...)`. Static state changes don't
animate yet — the modifier round-trips but the compositor does not
schedule keyframes (Phase 10.8).

### Density (compositor-classified)

```
Density::Compact   < 600 px window width
Density::Regular   600..=1200
Density::Spacious  > 1200
```

### `IconId`

28 icons in v0.78.0 — see `abi.rs` enum. Atlas-native sizes
16/24/32/48/64 px; pick the one your layout uses to avoid scale
artifacts at 4K.

### `TextStyle`

```
Body | Title | Caption | Muted | Mono
```

`Mono` routes to the Spleen bitmap font (terminal aesthetic);
everything else uses Inter Variable.

---

## Prefabs — the cookbook (prefer over raw widgets)

Prefabs encode the design system. App code should compose these and
only reach for raw `Widget`/`Modifier` when no prefab fits.

### Containers / surfaces

| Prefab                   | Use                                                         |
|--------------------------|-------------------------------------------------------------|
| `panel(children)`        | Plain root container — Md spacing, no decoration.           |
| `card(content, kind)`    | `CardKind::Inset / Panel / Sheet`. SurfaceElevated + Rounded-Lg + Padding-Lg. |
| `sidebar_pane(sections)` | Vertical SurfaceMuted column with Padding-Sm. Trailing flex-Spacer auto-appended. |
| `dialog(title, body, footer_hint, min_w)` | Sheet wrapper with Title, Divider, body, optional footer. Compact-density tightens padding. |

### Form / input

| Prefab                                            | Use                                  |
|---------------------------------------------------|--------------------------------------|
| `button(label, ButtonStyle, on_click)`            | Primary / Secondary / Ghost / Destructive — themed bg + Hover/Active/Focus states. |
| `input(value, placeholder, InputKind, on_submit, trailing)` | Themed text input. Search prepends Magnifier; `NO_ACTION` opts out of submit; `trailing` is right-aligned. |

### Lists / navigation

| Prefab                                                   | Use                                              |
|----------------------------------------------------------|--------------------------------------------------|
| `list_row(icon, title, sub, selected, on_click, on_hover)` | List item with title + subtitle. Selected = Accent fill. Non-selected: Hover-bg + Focus-border. |
| `nav_row(icon, label, selected, on_click, on_hover)`     | Sidebar navigation item. Same selected-vs-state pattern as list_row, single label only. |
| `sidebar_section(title, items)`                          | "PLACES" / "DEVICES" group header above a list of nav_rows. |
| `breadcrumb(segments)`                                   | Path with caret separators. Each segment carries an ActionId. |
| `scroll_list(items)`                                     | Scrollable Column wrap.                          |
| `grid_item(icon, label, selected, on_click, on_hover)`   | Square cell — large icon over single-line label. |
| `grid(items, per_row)`                                   | Wrap a flat list into N-column rows.             |

### Chrome

| Prefab                                  | Use                                     |
|-----------------------------------------|-----------------------------------------|
| `toolbar(children)`                     | Horizontal row with Padding-Sm.         |
| `menu_bar(labels)`                      | Top menu strip.                         |
| `icon_button(icon, size, on_click, on_hover)` | Square tap target with Hover/Focus states. |
| `footer(left, right)`                   | Bottom status row.                      |
| `title_bar(title)`                      | Bare Title-styled text with padding.    |
| `badge(text)`                           | Caption-style chip.                     |
| `body(text) / muted(text)`              | Convenience constructors.               |
| `empty_state(text)`                     | Centred Muted message for empty lists.  |

### Constants

`prefab::NO_ACTION` — `ActionId(u32::MAX)` sentinel for "no submit /
no callback". Apps must not use this id for their own actions.

---

## Pseudo-state semantics

CSS-style `:hover` / `:focus` / `:active` apply to a node **and all of
its ancestors**. So `Modifier::Hover(...)` on a Row triggers when the
cursor is over a descendant Icon — the whole Row gets the hover style.

`Modifier::Disabled(...)` is presence-based: if a widget has it, the
widget IS disabled (and renders with the inner mods applied). Disabled
overrides Hover/Focus/Active. Disabled subtrees swallow click +
hover events for the whole subtree, not just the disabled node.

State priority (last-write-wins on conflicting mods like `Background`):

1. Base modifiers
2. `WhenDensity` matches (always orthogonal to interactive states)
3. `Hover`
4. `Focus`
5. `Active`
6. `Disabled` (overrides 3–5 entirely if present)

---

## Tab navigation

The compositor intercepts Tab / Shift-Tab on focused widget windows
and walks focus through every focusable widget in document order.
Wraparound at the end. "Focusable" = `Button` / `Input` / `Checkbox`,
or any widget with `OnClick`. Disabled subtrees are skipped.

If a window has zero focusable widgets, Tab falls through to the app
as a regular `Event::Key(KeyCode::Tab)`.

---

## What's NOT supported (yet)

- **`Widget::Input` self-editing** — apps still route `Char`/`Backspace` themselves.
- **Password masking** — `InputKind::Password` exists but renders plaintext.
- **Animations** — `Modifier::Transition(...)` round-trips, but the compositor doesn't interpolate yet.
- **Drop shadows, blur, gradients** — `Shadow`/`Blur`/`Effect` are wire-frozen but no-op in the CPU rasterizer.
- **Scale (static)** — only meaningful inside Hover/Focus/Active mods today; root-level `Scale` is ignored until a compositing-layer pass lands.
- **Free pixel values** — every padding/radius/spacing must use a token from the scales above. Adding a new step → SDK PR, not magic numbers.
- **Custom fonts / hex colours** — system fonts only, palette tokens only.

---

## Example: minimal launcher

```rust
use nopeek_widgets::{prefab, *};

fn render(state: &State) -> Widget {
    let badge = prefab::badge(env!("CARGO_PKG_NAME"));
    let search = prefab::input(
        &state.query,
        "Type to search…",
        prefab::InputKind::Search,
        prefab::NO_ACTION,
        Some(badge),
    );

    let rows: Vec<Widget> = state.results.iter().enumerate().map(|(i, r)| {
        prefab::list_row(
            r.icon, &r.title, &r.subtitle,
            i == state.selected,
            Some(ActionId(i as u32 + 1)),         // +1 keeps 0 as NO_ACTION
            Some(ActionId(10_000 + i as u32)),    // hover offset
        )
    }).collect();

    let footer = prefab::footer("↑↓  ↵  esc", &format!("{} results", state.results.len()));

    prefab::panel(alloc::vec![
        search,
        Widget::Divider,
        prefab::scroll_list(rows),
        Widget::Spacer { flex: 1 },
        Widget::Divider,
        footer,
    ])
}
```

Visible behaviour from this code alone, without writing any CSS or
animation: hover-bg on rows, focus borders on Tab nav, accent fill on
the selected row, focused search-input gets an Accent border, theme
swap repaints the whole tree. The styling is in the prefabs.
