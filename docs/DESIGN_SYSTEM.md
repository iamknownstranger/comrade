# Comrade design system — Material, glass, and shadcn's discipline

One visual contract, three frontends. This file is the **source of truth**:
`desktop/ui/styles.css`, `android/.../ui/theme/`, and `app/lib/src/theme/`
each implement it in their own idiom, and where one of them *cannot*, the
divergence is written down here rather than discovered later.

## 1. What "hybrid" means, concretely

Three systems contribute, and each owns a different question:

| System | Owns | What we take |
| --- | --- | --- |
| **shadcn/ui** | *How tokens are named* | Paired `X` / `X-foreground` semantics, one `--radius` the rest derives from, an explicit focus `ring`, a fixed variant set |
| **Material 3** | *How a surface reacts* | Tonal elevation, state-layer opacities, motion durations/easings, 48dp touch targets |
| **Liquid glass** | *How floating chrome reads* | Backdrop blur + saturation, a specular top edge, depth shadow — on **chrome only** |

The reason to name the split is that they conflict. M3 says elevation is
tone; glass says elevation is blur; shadcn says elevation is a border and
almost no shadow. The resolution below is a **tier system**: a surface picks
exactly one tier, and the tier decides which of the three is in charge.

## 2. Surface tiers

A surface is in exactly one tier. Nothing is half-glass.

| Tier | Used for | Treatment |
| --- | --- | --- |
| `base` | Page background | Flat `background`. No border, no shadow. |
| `card` | Content that scrolls with the page: list rows, message bubbles, section cards | `card` fill, 1px `border`, **no shadow**. shadcn's posture — the border does the work. |
| `raised` | Content that must separate without floating: selected rows, inline editors | `muted` fill, 1px `border`, M3 state layer on interaction |
| `glass` | Chrome that floats *over* content: nav rail, sidebar, top bar, bottom nav, sheets, dialogs, popovers, toasts, call dock, composer, minimised call tile | Translucent tint + backdrop blur + saturation + specular edge + depth shadow |
| `scrim` | Behind a modal | `background` at 70% with a light blur |

**Glass is never applied to dense or body content.** Not to chat bubbles, not
to list rows, not to the reading column, not to the media viewer (which stays
deliberately black — a photo is judged against black, not against the app).
Text over a blurred, moving backdrop is the single failure mode of this
material, and the rule above is what prevents it.

## 3. Token contract

Every frontend defines these names. The *values* below are the dark ramp,
which stays the default posture; the light ramp is the same names re-valued.

### 3.1 Paired semantics

```
background / foreground      page ground and its text
card       / card-foreground content surfaces
popover    / popover-foreground   glass chrome and its text
muted      / muted-foreground     de-emphasised fills and secondary text
primary    / primary-foreground   the accent, and what reads on top of it
secondary  / secondary-foreground
destructive / destructive-foreground
success    / success-foreground
warning    / warning-foreground
border      hairlines
input       field borders (a step stronger than border)
ring        focus indicator — never the same value as border
```

The pairing rule is load-bearing: **a fill token is never used as text, and a
foreground token is never used as a fill.** `app/lib/src/theme/comrade_theme.dart`
already records what happens when that rule is broken — the Travel accent
reused as light-mode text measured 2.1:1 against its surface. Every foreground
token must clear **4.5:1** against every fill it is paired with.

### 3.2 Radius, derived

```
--radius: 12px          (base)
sm = radius - 4         8px   chips, ticks, small controls
md = radius - 2        10px   inputs, buttons
lg = radius            12px   cards
xl = radius + 6        18px   sheets, dialogs, bubbles
2xl = radius + 16      28px   the vault card, full-screen surfaces
```

One knob. Changing `--radius` re-proportions the whole app, which is the point.

### 3.3 State layers (Material 3, fixed)

Applied as an overlay of the *foreground* colour over the surface:

```
hover     0.08
focus     0.10
pressed   0.10
dragged   0.16
selected  0.12
disabled  content 0.38 · container 0.12
```

These are constants, not per-component choices. A component that wants a
different hover strength is wrong about being a different component.

### 3.4 Glass tier

```
blur        20px      (chrome)   ·  28px (sheets, dialogs)
saturate    180%                 restores colour the blur washes out
tint        the tier's fill at 72% alpha
highlight   inset 0 1px 0 <foreground at 10%>    specular top edge
shadow      0 8px 32px <background at 40%>       depth
border      1px <border at 60%>
```

The specular highlight is what separates glass from "a translucent box": real
glass catches light on its top edge. It is one inset shadow, and skipping it
is the most common way this material ends up looking cheap.

### 3.5 Motion

```
fast     120ms   state layers, ticks
base     200ms   most transitions
slow     320ms   sheets, dialogs, tier changes
easing   cubic-bezier(0.2, 0, 0, 1)   (M3 emphasised-decelerate)
```

## 4. Accessibility, and the three escape hatches

Glass has failure modes that are not cosmetic. All three frontends implement
all three fallbacks; a frontend that cannot is listed in §5.

1. **No backdrop-filter support** → the glass tier renders as an **opaque**
   `popover` fill with its border and shadow. It must stay legible, not
   merely stay visible.
2. **`prefers-reduced-transparency: reduce`** → same opaque fallback. This is
   a stated accessibility need, not a preference to average against a default.
3. **`prefers-reduced-motion: reduce`** → drift/ambient animations stop and
   transitions collapse to `fast`. The Focus tab's drifting colour fields are
   the specific thing this exists for.

Focus is never suppressed. Every interactive element shows a 2px `ring`
offset 2px on `:focus-visible`, in every tier including glass.

That is the rule; it is not yet the state of the code on two of the three
frontends. CSS applies `:focus-visible` to every focusable element at once, so
desktop satisfies it by construction — Compose and Flutter need the ring
applied per call site, and today it reaches the nav bar, the composer and the
call controls, not the long tail. `AUDIT.md` V5 tracks the gap and names the
two cases that need more than a call site. Treat the rule as binding on new
code rather than as a description of what already ships.

## 5. Where the frontends diverge, and why

**Desktop** gets the full material. The Tauri webview (WebKitGTK / WebView2 /
WKWebView) supports `backdrop-filter` on all three platforms, so glass there
is real backdrop blur with a `@supports` fallback.

**Flutter** gets the full material. `BackdropFilter` + `ImageFilter.blur` is
first-party, so `app/` matches desktop.

**Android (Compose) does not get true backdrop blur, deliberately.** Compose's
`Modifier.blur` blurs a composable's *own content*, not what is behind it;
backdrop blur needs either `RenderEffect` plumbing that only exists on API 31+
or a third-party haze library. A new Android dependency has to be added in two
places — `android/app/build.gradle.kts` *and* `app/android/app/build.gradle.kts`,
because `stagePreservedServices` recompiles `android/`'s Kotlin into the
Flutter app — and a haze library is a large surface for a visual effect. So
the Android glass tier is **translucent tint + specular edge + shadow, without
blur**: the same tokens, the same tier rules, one missing ingredient.

This is a real difference and it is allowed to stay one. Revisit if a
first-party backdrop-blur modifier ships in Compose, or if minSdk reaches 31
and `RenderEffect` becomes unconditional.

**Android also cannot implement §4's second hatch, because the signal does not
exist.** Reduced transparency is an iOS accessibility setting with a CSS
counterpart (`prefers-reduced-transparency`); Android exposes no public
equivalent, so there is nothing to read. Desktop and Flutter honour it; Android
has no way to.

What Android *can* read is reduced **motion** — `Settings.Global.ANIMATOR_DURATION_SCALE`
at `0f`, which is the same setting that backs Flutter's `MediaQuery.disableAnimations`
on Android. That hatch is implemented, and it is deliberately wired to do more
work there than elsewhere: as well as collapsing durations, it makes the glass
tint **opaque**. Android's glass has no blur, but it is still translucent, so
content still moves behind it; turning it opaque under the one signal Android
does expose recovers most of what the missing transparency hatch would have
bought. A user who asks for no motion gets a still, solid surface.

So the honest count is: desktop 3 of 3, Flutter 3 of 3, Android 2 of 3, with the
third unavailable rather than skipped. Revisit if Android ever ships a
reduced-transparency setting.

## 6. What this does not change

Behaviour. This is a visual system: token definitions, a tier applied to
chrome, and the primitives that carry them. Screen layouts, navigation
structure, and every decision function (`TogetherDecisions.kt`,
`call_decisions.mjs`, and the rest) are untouched. A screen picks up the new
look by being made of the primitives, not by being rewritten.

## 7. Layout, rhythm and state — the polish contract

§3 fixed what things are *coloured* with. This section fixes what makes the
same tokens read as finished rather than assembled: spacing rhythm, type
hierarchy, and the states a screen shows when it has nothing to show. It was
written after an owner review that landed on two words — "dated" and
"unfinished" — and each rule below names the measurement behind it.

### 7.1 Spacing is a 4dp grid, and drift is the bug

Android alone carried **138 off-grid values** (10, 6, 18, 14, 3, 1, 7, 9, 11)
against 52 uses of 16 and 28 of 8. Nobody sees "14dp"; what they see is edges
that fail to line up between two screens, which is precisely the "unfinished"
read.

```
space-1   4      space-5  20      space-10  40
space-2   8      space-6  24      space-12  48
space-3  12      space-8  32      space-16  64
```

Every padding, gap and inset resolves to one of these. Values that are not
multiples of 4 are permitted in exactly two places, and both must carry a
comment saying which: a 1px/1dp hairline, and an optical correction that
compensates for a glyph or icon's own bearing.

### 7.2 Type: fewer sizes, and size is not the only signal

A hierarchy built only from size needs many sizes. One built from size *and*
weight *and* colour needs few. Cap the scale at six roles; separate adjacent
roles by weight or by `mutedForeground`, not by two points of size.

```
display  32 / 700 / 1.15 / -0.02em    title    20 / 600 / 1.30 / -0.01em
heading  24 / 700 / 1.20 / -0.02em    body     15 / 400 / 1.50 /  0
                                      label    13 / 500 / 1.40 /  0.01em
                                      caption  12 / 400 / 1.35 /  0.02em
```

Negative tracking on large text and positive on small is what separates a
current-looking type stack from a default one. Line-height is part of the
token, not a per-call-site decision.

### 7.3 Lists load as skeletons, not spinners

**All 14 loading states in `android/app/src/main/java/mullu/comrade/ui` are a
`CircularProgressIndicator`; there is not one skeleton in the tree.** A
centred spinner over a blank screen is the most dated pattern still shipping,
and it also *hides* the layout, so arrival is a jump.

- Content whose shape is known before it arrives — chat rows, comrades, feed
  items, journal entries, tasks, call history — renders **skeleton
  placeholders in the real row geometry**, 3–6 rows, at `muted` with a slow
  opacity pulse (never a travelling gradient sweep; it draws the eye to the
  loader instead of the content).
- A spinner is correct only for a bounded, blocking, indeterminate action the
  user just triggered (unlocking the vault, sending). Not for lists.
- Under reduced motion the pulse stops and the skeleton is static — still a
  layout preview, just not animated.

### 7.4 Every list has three states, and all three are designed

Empty is a state, not the absence of one. One pattern everywhere: a single
line saying what belongs here in `mutedForeground`, and — when there is an
action that would fill it — exactly one button. No illustration, no headline
plus subhead plus two buttons. First-run empty and filtered-to-nothing empty
say different things and must not share a string.

Error states say what failed and offer retry. A list that can be empty, load,
or fail needs all three branches present before the screen is done.

### 7.5 Touch targets and hit area

Minimum **48dp** on every interactive element, expanding the hit area rather
than the drawn size where the visual is smaller. An icon button drawn at 24dp
still takes 48dp of touch. This is the accessibility floor, and it is also
most of why an interface feels confident rather than fiddly.

### 7.6 Motion is short, and it explains

Durations come from §3.5. Enter/exit for the same element must be the same
gesture reversed. Nothing on a routine path runs longer than `base`; nothing
at all exceeds 400ms. Motion earns its place by showing where something came
from — a sheet rising from the edge it will return to — and never as
decoration on arrival. §4.3's reduced-motion hatch collapses all of it.

### 7.7 The bar carries five

`MainTab` reached six, and the enum's own comment recorded the cost at the
time: "a NavigationBar is comfortable at five and tight at six — labels shrink
rather than wrap." Travel moves to the drawer beside Ride: it is a place you
go deliberately, not a daily surface, which is the same test that put Feed
there. Five stay: Chats, Journal, Together, Focus, Tara.

This is a navigation change and it is deliberately the *only* one — which of
the remaining five is least used is a question for usage data, not for a
polish pass.
