/// Comrade's visual system, derived from the two it replaces.
///
///  * Colours, shapes and the type hierarchy come from
///    `android/.../ui/theme/Theme.kt`.
///  * The surface/border ramp, the workspace accent skins (Travel amber,
///    Sakha cyan, Sakhi rose) and the "dark-mode-first" posture come from
///    `desktop/ui/styles.css` `:root` and its `body.theme-*` blocks.
///
/// One deliberate divergence, flagged in `SCREEN_INVENTORY.md`: Android opted
/// into Material You dynamic colour (`dynamicColor = true`), so on Android 12+
/// the app took the system wallpaper palette and the brand colours above were
/// only a fallback. Desktop has no such source and is brand-coloured always.
/// The unified app is **brand-coloured on every platform**: the two frontends
/// otherwise render visibly different products, and the call/crisis/status
/// colours below are load-bearing (a wallpaper-derived "error" container is
/// not guaranteed to read as alarming). Dynamic colour can come back later as
/// an explicit opt-in setting.
library;

import 'package:flutter/material.dart';

/// Identity-stable avatar hues: the same key renders the same colour on every
/// device (Telegram-style), so people become recognisable at a glance.
/// Copied verbatim from `ChatsScreen.kt`'s `AvatarPalette`.
const List<Color> kAvatarPalette = <Color>[
  Color(0xFF6366F1), // indigo
  Color(0xFF0EA5E9), // sky
  Color(0xFF10B981), // emerald
  Color(0xFFF59E0B), // amber
  Color(0xFFEF4444), // coral
  Color(0xFF8B5CF6), // violet
  Color(0xFFEC4899), // rose
  Color(0xFF14B8A6), // teal
];

/// The workspace skins `styles.css` swaps CSS variables for.
///
/// The dark accents are `--accent-2` verbatim. The light ones are **not**
/// `--accent`, which is what the first port of this file used: `styles.css`
/// has no `prefers-color-scheme` block at all, so every value in it was tuned
/// against a near-black background, and there `--accent` is a *fill* carrying
/// dark `--accent-contrast` text — never text itself. Reused as a light-mode
/// `primary` it becomes small text on white, and `SectionCard` titles in the
/// Travel workspace measured 2.1:1 against the surface (WCAG AA wants 4.5:1
/// for body text). A `FilledButton` was worse still: white on `#f59e0b` is
/// 2.15:1. Each light accent is therefore the same hue taken several steps
/// darker, chosen so the colour clears 4.5:1 both as text on every surface in
/// the ramp *and* under white — `theme_test.dart` asserts exactly that, for
/// every skin, so a future palette edit cannot quietly reintroduce it.
enum WorkspaceSkin {
  /// `:root` — indigo.
  base(Color(0xFF818CF8), Color(0xFF4F46E5)),

  /// `body.theme-travel` — warm amber, "off the grid".
  travel(Color(0xFFFBBF24), Color(0xFF92400E)),

  /// `body.theme-couple-sakha` — cool cyan.
  coupleSakha(Color(0xFF7DD3FC), Color(0xFF0369A1)),

  /// `body.theme-couple-sakhi` — warm rose.
  coupleSakhi(Color(0xFFFDA4AF), Color(0xFFBE123C));

  const WorkspaceSkin(this.darkAccent, this.lightAccent);

  /// Used on a dark background: `--accent-2`, bright.
  final Color darkAccent;

  /// Used on a light background: the same hue, dark enough to read as text.
  final Color lightAccent;

  static WorkspaceSkin fromWorkspaceKey(String key) => switch (key) {
        'OffGridTravel' => travel,
        'CoupleSandboxSakha' => coupleSakha,
        'CoupleSandboxSakhi' => coupleSakhi,
        _ => base,
      };
}

/// Colours the call overlay owns outright.
///
/// A call is full-bleed dark on both existing frontends regardless of the app
/// theme (`CallScreen.kt`'s `CallBackground`, `styles.css`'s `.call-overlay`),
/// because a bright call screen at 3am held to a face is a bug. These are not
/// part of the [ColorScheme] for exactly that reason.
abstract final class CallPalette {
  static const Color background = Color(0xFF0E1621);
  static const Color pipBackground = Color(0xFF17212B);
  static const Color accept = Color(0xFF2E7D32);
  static const Color hangup = Color(0xFFC62828);
  static const Color controlIdle = Color(0x33FFFFFF);
  static const Color controlActive = Color(0xFFFFFFFF);
  static const Color weakConnection = Color(0xFFFFA000);
  static const Color secondaryText = Color(0xFFB0BEC5);
}

/// Extra surface tokens `styles.css` has and Material's [ColorScheme] does
/// not — the panel/border ramp the desktop shell is built out of, plus the
/// rest of `docs/DESIGN_SYSTEM.md` §3.1's paired semantics that neither
/// `styles.css` nor [ColorScheme] had a slot for: `card`, `popover`, `muted`,
/// `input` and `ring`, and the `*Foreground` partners `good`/`warn`/`bad`
/// needed to be usable as fills rather than only as text.
///
/// `panel`/`panelAlt`/`border`/`borderStrong`/`good`/`warn`/`bad` are the
/// original names and have call sites across `app/lib/src/`; they are kept
/// exactly as they were rather than renamed out from under those call sites.
/// `card`/`popover`/`muted`/`input` are added alongside as §3.1's vocabulary
/// — mostly the same fills under a second name, because desktop and Android
/// reason in shadcn's naming and a third frontend using a third vocabulary
/// for the same ramp is its own kind of drift.
@immutable
class ComradeSurfaces extends ThemeExtension<ComradeSurfaces> {
  const ComradeSurfaces({
    required this.panel,
    required this.panelAlt,
    required this.border,
    required this.borderStrong,
    required this.good,
    required this.warn,
    required this.bad,
    required this.cardForeground,
    required this.popoverForeground,
    required this.mutedForeground,
    required this.goodForeground,
    required this.warnForeground,
    required this.badForeground,
    required this.ring,
  });

  /// `--panel` — sidebar, cards, the conversation list.
  final Color panel;

  /// `--panel-2` — a hover/selected step above [panel].
  final Color panelAlt;

  /// `--border`.
  final Color border;

  /// `--border-strong`.
  final Color borderStrong;

  /// `--good` / `--warn` / `--bad` status pills, rendered as *text* over a
  /// low-alpha wash of the same colour — the existing, unchanged use.
  final Color good;
  final Color warn;
  final Color bad;

  /// §3.1 `card` — content surfaces. Numerically [panel]; `SectionCard` and
  /// `CardThemeData` are exactly what the contract means by "card".
  Color get card => panel;

  /// §3.1 `card-foreground` — text on [card]. Stored rather than derived from
  /// [ColorScheme] because [ComradeSurfaces] does not carry one; matches
  /// `onSurface` in both ramps (`theme_test.dart` locks the pairing so the
  /// two cannot drift apart under a future palette edit).
  final Color cardForeground;

  /// §3.1 `popover` — the glass tier's base fill, before the tint alpha,
  /// blur, specular edge and shadow in §3.4 are layered on top (see
  /// `GlassSurface`). Numerically [panelAlt] today: the ramp has one
  /// elevated step, not two, and glass is meant to read as distinct through
  /// the *material*, not through owning a unique flat colour.
  Color get popover => panelAlt;

  /// §3.1 `popover-foreground` — text/icons on [popover], including the
  /// opaque fallback glass renders under §4's escape hatches.
  final Color popoverForeground;

  /// §3.1 `muted` — de-emphasised fills. Numerically [panelAlt]; kept as its
  /// own name because "a hover step" and "a muted fill" are different
  /// justifications for the same colour, and call sites should say which one
  /// they mean.
  Color get muted => panelAlt;

  /// §3.1 `muted-foreground` — de-emphasised text. Matches
  /// `ColorScheme.onSurfaceVariant` in both ramps.
  final Color mutedForeground;

  /// §3.1 `input` — field borders, "a step stronger than border". Exactly
  /// [borderStrong], which was already documented that way.
  Color get input => borderStrong;

  /// The `*Foreground` partners for [good]/[warn]/[bad] (§3.1's
  /// `success`/`warning`/`destructive`, under the names this ramp already
  /// uses): what to paint on top when one of them is a *fill* — a filled
  /// status badge — rather than text over a wash. Each equals the matching
  /// `on*` role in [ComradeTheme]'s [ColorScheme] (`good`↔`secondary`,
  /// `warn`↔`tertiary`, `bad`↔`error` already share a value by design; see
  /// "the status ramp agrees with the scheme beside it" in `theme_test.dart`).
  final Color goodForeground;
  final Color warnForeground;
  final Color badForeground;

  /// §3.1 `ring` — the focus indicator. Explicitly not tied to
  /// [WorkspaceSkin]: [ComradeSurfaces] carries no skin parameter, and glass
  /// chrome, dialogs and anything themed without a skin in scope still need
  /// a ring. It is [WorkspaceSkin.base]'s accent, so an unskinned focus ring
  /// still reads as this app's colour rather than an arbitrary one, and it
  /// is never [border] — the pairing rule (§3.1) requires the two to differ,
  /// and here they differ by more than a shade.
  final Color ring;

  static const ComradeSurfaces dark = ComradeSurfaces(
    panel: Color(0xFF131B2E),
    panelAlt: Color(0xFF1A2438),
    border: Color(0xFF243049),
    borderStrong: Color(0xFF34425F),
    good: Color(0xFF34D399),
    warn: Color(0xFFFBBF24),
    bad: Color(0xFFF87171),
    cardForeground: Color(0xFFE6EBF5),
    popoverForeground: Color(0xFFE6EBF5),
    mutedForeground: Color(0xFF9AA7C2),
    // Same literals as `onSecondary`/`onTertiary`/`onError` in the dark
    // `ColorScheme` below, because `good`/`warn`/`bad` already equal
    // `secondary`/`tertiary`/`error` there.
    goodForeground: Color(0xFF022C22),
    warnForeground: Color(0xFF2A1B06),
    badForeground: Color(0xFF3B0A0A),
    ring: Color(0xFF818CF8),
  );

  /// Same note as [WorkspaceSkin]'s light accents: `good`/`warn`/`bad` are
  /// rendered as *text* (the sidebar status pills in `home_shell.dart` colour
  /// their label with them, over an 18%-alpha wash of the same colour), so the
  /// dark ramp's `--good`/`--warn`/`--bad` are too light to reuse here. These
  /// are the 700/800 steps of the same hues.
  static const ComradeSurfaces light = ComradeSurfaces(
    panel: Color(0xFFF4F6FB),
    panelAlt: Color(0xFFE8ECF6),
    border: Color(0xFFD5DBE8),
    borderStrong: Color(0xFFB6BFD2),
    good: Color(0xFF065F46),
    warn: Color(0xFF92400E),
    bad: Color(0xFFB91C1C),
    cardForeground: Color(0xFF141A28),
    popoverForeground: Color(0xFF141A28),
    mutedForeground: Color(0xFF4A5468),
    // `onSecondary`/`onTertiary`/`onError` are all white in the light
    // scheme, for the same reason as the dark ramp's note above.
    goodForeground: Colors.white,
    warnForeground: Colors.white,
    badForeground: Colors.white,
    ring: Color(0xFF4F46E5),
  );

  static ComradeSurfaces forBrightness(Brightness brightness) =>
      brightness == Brightness.dark ? dark : light;

  @override
  ComradeSurfaces copyWith({
    Color? panel,
    Color? panelAlt,
    Color? border,
    Color? borderStrong,
    Color? good,
    Color? warn,
    Color? bad,
    Color? cardForeground,
    Color? popoverForeground,
    Color? mutedForeground,
    Color? goodForeground,
    Color? warnForeground,
    Color? badForeground,
    Color? ring,
  }) =>
      ComradeSurfaces(
        panel: panel ?? this.panel,
        panelAlt: panelAlt ?? this.panelAlt,
        border: border ?? this.border,
        borderStrong: borderStrong ?? this.borderStrong,
        good: good ?? this.good,
        warn: warn ?? this.warn,
        bad: bad ?? this.bad,
        cardForeground: cardForeground ?? this.cardForeground,
        popoverForeground: popoverForeground ?? this.popoverForeground,
        mutedForeground: mutedForeground ?? this.mutedForeground,
        goodForeground: goodForeground ?? this.goodForeground,
        warnForeground: warnForeground ?? this.warnForeground,
        badForeground: badForeground ?? this.badForeground,
        ring: ring ?? this.ring,
      );

  @override
  ComradeSurfaces lerp(ThemeExtension<ComradeSurfaces>? other, double t) {
    if (other is! ComradeSurfaces) return this;
    return ComradeSurfaces(
      panel: Color.lerp(panel, other.panel, t)!,
      panelAlt: Color.lerp(panelAlt, other.panelAlt, t)!,
      border: Color.lerp(border, other.border, t)!,
      borderStrong: Color.lerp(borderStrong, other.borderStrong, t)!,
      good: Color.lerp(good, other.good, t)!,
      warn: Color.lerp(warn, other.warn, t)!,
      bad: Color.lerp(bad, other.bad, t)!,
      cardForeground: Color.lerp(cardForeground, other.cardForeground, t)!,
      popoverForeground:
          Color.lerp(popoverForeground, other.popoverForeground, t)!,
      mutedForeground: Color.lerp(mutedForeground, other.mutedForeground, t)!,
      goodForeground: Color.lerp(goodForeground, other.goodForeground, t)!,
      warnForeground: Color.lerp(warnForeground, other.warnForeground, t)!,
      badForeground: Color.lerp(badForeground, other.badForeground, t)!,
      ring: Color.lerp(ring, other.ring, t)!,
    );
  }
}

/// Convenience accessor: `context.surfaces.border`.
extension ComradeThemeX on BuildContext {
  /// The fallback follows the ambient [Brightness] rather than always being
  /// the dark ramp. A widget built under a theme that never registered the
  /// extension — a stock `ThemeData`, a local `Theme` override, a dialog
  /// someone re-themes — would otherwise paint dark panels and dark borders
  /// onto a light scaffold, which is the exact failure this ramp exists to
  /// avoid. It should never happen; when it does it stays legible.
  ComradeSurfaces get surfaces =>
      Theme.of(this).extension<ComradeSurfaces>() ??
      ComradeSurfaces.forBrightness(Theme.of(this).brightness);
  ColorScheme get colors => Theme.of(this).colorScheme;
  TextTheme get texts => Theme.of(this).textTheme;
}

/// Soft, generous corner radii — cards, dialogs and sheets read as one
/// rounded, modern surface system instead of the sharper M3 defaults
/// (`Theme.kt`'s `ComradeShapes`).
const ShapeBorder kCardShape = RoundedRectangleBorder(
  borderRadius: BorderRadius.all(Radius.circular(16)),
);

/// Radius, derived (§3.2): one base constant, the rest a fixed offset from
/// it. "Changing `--radius` re-proportions the whole app" only holds if
/// everything actually reads from [base] — these do.
abstract final class ComradeRadii {
  /// `--radius`. The one knob.
  static const double base = 12;

  /// `sm` — chips, ticks, small controls.
  static const double sm = base - 4;

  /// `md` — inputs, buttons.
  static const double md = base - 2;

  /// `lg` — cards. Equal to [base] itself.
  static const double lg = base;

  /// `xl` — sheets, dialogs, bubbles.
  static const double xl = base + 6;

  /// `2xl` — the vault card, full-screen surfaces. Named `xxl`: Dart
  /// identifiers cannot start with a digit.
  static const double xxl = base + 16;

  // ── Pre-existing names ──────────────────────────────────────────────────
  // These had call sites across `app/lib/src/` before §3.2 existed and are
  // kept working rather than renamed out from under them. Each is mapped to
  // the §3.2 tier that matches what it is actually used for — not to
  // whichever numeral happened to be closest to the old one, which is why
  // `small` (`sm`) and `large` (`md`) both change value here.
  /// Was a hardcoded 8 — unchanged; message-bubble reaction chips and the
  /// "mDNS off" pill are exactly `sm`'s "small controls".
  static const double extraSmall = sm;

  /// Was a hardcoded 12; now `sm`, not `lg` — attachment thumbnails and
  /// media corners (`message_bubble.dart`, `media_attachment.dart`,
  /// `attachment_preview.dart`) are chips and small controls, not cards.
  static const double small = sm;

  /// Was a hardcoded 16; now `lg` — `SectionCard` and `CardThemeData` are
  /// exactly §3.2's "cards".
  static const double medium = lg;

  /// Was a hardcoded 22; now `md` — the only call sites are
  /// `FilledButton`/`OutlinedButton` shapes in [ComradeTheme], and §3.2 says
  /// buttons are `md`.
  static const double large = md;

  /// Was a hardcoded 28 — unchanged. Nothing under `app/lib/src/` used this
  /// outside [ComradeTheme] itself, which now points its dialog shape at
  /// [xl] directly (§3.2: dialogs are `xl`, not `2xl`), so `extraLarge` keeps
  /// its old value and its old meaning — the `2xl` "full-screen surfaces"
  /// tier — for whatever reaches for the biggest rounding on the scale.
  static const double extraLarge = xxl;

  /// Chat bubbles: 18 everywhere except the "tail" corner, which is 6 — the
  /// one documented exception to the derived scale (§3.2).
  static const double bubble = 18;
  static const double bubbleTail = 6;
}

/// State-layer opacities (§3.3, Material 3, fixed): an overlay of the
/// *foreground* colour over a surface, at a strength that names the
/// interaction rather than the component. A component that wants a
/// different hover strength is wrong about being a different component.
abstract final class ComradeStateLayers {
  static const double hover = 0.08;
  static const double focus = 0.10;
  static const double pressed = 0.10;
  static const double dragged = 0.16;
  static const double selected = 0.12;
  static const double disabledContent = 0.38;
  static const double disabledContainer = 0.12;
}

/// Motion (§3.5). `easing` is M3's emphasised-decelerate curve.
abstract final class ComradeMotion {
  /// State layers, ticks.
  static const Duration fast = Duration(milliseconds: 120);

  /// Most transitions.
  static const Duration base = Duration(milliseconds: 200);

  /// Sheets, dialogs, tier changes.
  static const Duration slow = Duration(milliseconds: 320);

  static const Curve easing = Cubic(0.2, 0, 0, 1);
}

/// The glass tier's material constants (§3.4). Consumed by `GlassSurface`
/// (`app/lib/src/widgets/glass_surface.dart`); kept here with the rest of the
/// token layer rather than inline in the widget, same as every other number
/// on this page.
abstract final class ComradeGlass {
  /// Chrome that sits directly over content: app bars, bottom navigation,
  /// snackbars, popovers, the composer, the call control dock.
  static const double blurChrome = 20;

  /// Sheets and dialogs, which sit further above the content than chrome
  /// does.
  static const double blurSheet = 28;

  /// 180% — restores the colour the blur washes out.
  static const double saturation = 1.8;

  /// The tier's fill, at this alpha, is the tint.
  static const double tintAlpha = 0.72;

  /// The specular top-edge highlight, over `popoverForeground`.
  static const double highlightAlpha = 0.10;

  /// The depth shadow, over the page background.
  static const double shadowAlpha = 0.40;

  /// The 1px border, over `border`.
  static const double borderAlpha = 0.60;
}

abstract final class ComradeTheme {
  static ThemeData dark({WorkspaceSkin skin = WorkspaceSkin.base}) =>
      _build(Brightness.dark, skin);

  static ThemeData light({WorkspaceSkin skin = WorkspaceSkin.base}) =>
      _build(Brightness.light, skin);

  static ThemeData _build(Brightness brightness, WorkspaceSkin skin) {
    final bool isDark = brightness == Brightness.dark;
    final ColorScheme scheme = isDark
        ? ColorScheme.dark(
            primary: skin.darkAccent,
            onPrimary: const Color(0xFF1E1B4B),
            primaryContainer: const Color(0xFF3730A3),
            onPrimaryContainer: const Color(0xFFE0E7FF),
            secondary: const Color(0xFF34D399),
            onSecondary: const Color(0xFF022C22),
            secondaryContainer: const Color(0xFF1A2438),
            onSecondaryContainer: const Color(0xFFE6EBF5),
            tertiary: const Color(0xFFFBBF24),
            onTertiary: const Color(0xFF2A1B06),
            surface: const Color(0xFF0F1525),
            onSurface: const Color(0xFFE6EBF5),
            surfaceContainerLowest: const Color(0xFF0A0E1A),
            surfaceContainerLow: const Color(0xFF0F1525),
            surfaceContainer: const Color(0xFF131B2E),
            surfaceContainerHigh: const Color(0xFF1A2438),
            surfaceContainerHighest: const Color(0xFF1A2438),
            onSurfaceVariant: const Color(0xFF9AA7C2),
            // Was `#6B7894` in both schemes, which is the one thing a shared
            // value cannot be: legible on a near-black surface *and* on a white
            // one. It reached 3.5:1 over `panelAlt` — under AA even for large
            // text — while carrying real information in small type (a message's
            // clock time, delivery ticks, the "mDNS off" pill). Each scheme now
            // gets its own step, still quieter than `onSurfaceVariant` so the
            // hierarchy holds, but no longer quiet to the point of unreadable.
            outline: const Color(0xFF8794B0),
            outlineVariant: const Color(0xFF34425F),
            error: const Color(0xFFF87171),
            onError: const Color(0xFF3B0A0A),
            errorContainer: const Color(0xFF5A1A1A),
            onErrorContainer: const Color(0xFFFFE0E0),
          )
        : ColorScheme.light(
            primary: skin.lightAccent,
            onPrimary: Colors.white,
            primaryContainer: const Color(0xFFE0E7FF),
            onPrimaryContainer: const Color(0xFF1E1B4B),
            // Kept in step with `ComradeSurfaces.light`'s good/warn, for the
            // same reason: both roles can end up as text on a light surface.
            secondary: const Color(0xFF065F46),
            onSecondary: Colors.white,
            secondaryContainer: const Color(0xFFE8ECF6),
            onSecondaryContainer: const Color(0xFF1E1B4B),
            tertiary: const Color(0xFF92400E),
            onTertiary: Colors.white,
            surface: const Color(0xFFFBFCFF),
            onSurface: const Color(0xFF141A28),
            surfaceContainerLowest: Colors.white,
            surfaceContainerLow: const Color(0xFFF7F9FE),
            surfaceContainer: const Color(0xFFF4F6FB),
            surfaceContainerHigh: const Color(0xFFE8ECF6),
            surfaceContainerHighest: const Color(0xFFE8ECF6),
            onSurfaceVariant: const Color(0xFF4A5468),
            // The light counterpart of the dark scheme's `outline` note: darker
            // rather than lighter, same reason, same 4.5:1 bar.
            outline: const Color(0xFF566072),
            outlineVariant: const Color(0xFFD5DBE8),
            // Stated rather than inherited. `ColorScheme.light`'s default error
            // is Material's own `#b00020`, which left the light theme's error
            // colour unrelated to `ComradeSurfaces.light.bad` even though
            // `ErrorText` reads the first and the status pills read the second.
            // The dark scheme already matched its ramp; now both do.
            error: const Color(0xFFB91C1C),
            onError: Colors.white,
            errorContainer: const Color(0xFFFEE2E2),
            onErrorContainer: const Color(0xFF7F1D1D),
          );

    // `.white` for dark, `.black` for light — then recoloured to `onSurface`
    // anyway. The `.apply` below already covers all fifteen styles, so the
    // choice is invisible today; it is made correctly so that removing the
    // recolour later cannot silently produce black text on a dark surface.
    final Typography typography = Typography.material2021(
      platform: TargetPlatform.android,
      colorScheme: scheme,
    );
    final TextTheme text = _typography(
      (isDark ? typography.white : typography.black).apply(
        bodyColor: scheme.onSurface,
        displayColor: scheme.onSurface,
      ),
    );

    final ComradeSurfaces surfaces =
        isDark ? ComradeSurfaces.dark : ComradeSurfaces.light;

    return ThemeData(
      useMaterial3: true,
      brightness: brightness,
      colorScheme: scheme,
      scaffoldBackgroundColor:
          isDark ? const Color(0xFF0A0E1A) : const Color(0xFFFBFCFF),
      textTheme: text,
      extensions: <ThemeExtension<dynamic>>[surfaces],
      // §4: the baseline focus colour for stock Material widgets that paint
      // their own focus state layer. `ComradeFocusRing`
      // (`widgets/glass_surface.dart`) is the explicit 2px `ring` for
      // components that draw their own chrome, including glass; this keeps
      // the two consistent rather than picking a different colour by accident.
      focusColor: surfaces.ring.withValues(alpha: ComradeStateLayers.focus),
      cardTheme: CardThemeData(
        clipBehavior: Clip.antiAlias,
        elevation: 0,
        margin: EdgeInsets.zero,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(ComradeRadii.medium),
        ),
      ),
      dialogTheme: DialogThemeData(
        // §3.2: dialogs are `xl`, not the old hardcoded 28 (`extraLarge` is
        // now the `2xl` tier — see `ComradeRadii.extraLarge`'s doc).
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(ComradeRadii.xl),
        ),
      ),
      inputDecorationTheme: InputDecorationTheme(
        // §3.2: `md`, not `small`/`sm` — an input field is exactly `md`'s
        // "inputs, buttons", not `sm`'s "chips, ticks, small controls".
        border: OutlineInputBorder(
          borderRadius: BorderRadius.circular(ComradeRadii.md),
        ),
        isDense: true,
      ),
      filledButtonTheme: FilledButtonThemeData(
        style: FilledButton.styleFrom(
          minimumSize: const Size(0, 44),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(ComradeRadii.large),
          ),
        ),
      ),
      outlinedButtonTheme: OutlinedButtonThemeData(
        style: OutlinedButton.styleFrom(
          minimumSize: const Size(0, 44),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(ComradeRadii.large),
          ),
        ),
      ),
      navigationBarTheme: const NavigationBarThemeData(height: 68),
      // §2 lists toasts and popovers as glass tier chrome, but neither
      // `SnackBarThemeData` nor `PopupMenuThemeData` has a slot for a widget
      // — no `BackdropFilter`, and `ThemeData` is built once, with no live
      // `BuildContext`, so it cannot react to §4's escape hatches the way
      // `GlassSurface` does per frame. `showGlassDialog`
      // (`widgets/glass_surface.dart`) works around that for dialogs by
      // wrapping each call site's builder; snackbars and popup menus are not
      // built through a call site this app controls the same way (a
      // `PopupMenuButton`'s menu surface is assembled by the framework, not
      // handed back to a builder), so both stay the one thing `ThemeData`
      // *can* give them: the §4 opaque-fallback look, permanently — the
      // glass tier's fill, border and text colour, without the blur.
      snackBarTheme: SnackBarThemeData(
        behavior: SnackBarBehavior.floating,
        backgroundColor: surfaces.popover,
        contentTextStyle:
            text.bodyMedium?.copyWith(color: surfaces.popoverForeground),
        actionTextColor: scheme.primary,
        elevation: 6,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(ComradeRadii.small),
          side: BorderSide(color: surfaces.border),
        ),
      ),
      popupMenuTheme: PopupMenuThemeData(
        color: surfaces.popover,
        surfaceTintColor: Colors.transparent,
        textStyle: text.bodyMedium?.copyWith(color: surfaces.popoverForeground),
        elevation: 8,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(ComradeRadii.md),
          side: BorderSide(color: surfaces.border),
        ),
      ),
      dividerTheme: DividerThemeData(
        color: scheme.outlineVariant.withValues(alpha: 0.4),
        space: 1,
        thickness: 1,
      ),
    );
  }

  /// Default M3 type with a firmer title hierarchy: names and headings sit
  /// semi-bold so lists scan by name first, metadata second (`Theme.kt`'s
  /// `ComradeTypography`).
  static TextTheme _typography(TextTheme base) => base.copyWith(
        headlineMedium:
            base.headlineMedium?.copyWith(fontWeight: FontWeight.bold),
        titleLarge: base.titleLarge?.copyWith(fontWeight: FontWeight.w600),
        titleMedium: base.titleMedium?.copyWith(fontWeight: FontWeight.w600),
        titleSmall: base.titleSmall?.copyWith(
          fontWeight: FontWeight.w600,
          letterSpacing: 0.1,
        ),
        labelSmall: base.labelSmall?.copyWith(letterSpacing: 0.2),
      );
}

/// The monospace style keys are rendered in everywhere (`FontFamily.Monospace`
/// on Android, `--mono` on desktop). Keys are compared by eye; a proportional
/// font makes that harder than it needs to be.
TextStyle? monoStyle(TextStyle? base) => base?.copyWith(
      fontFamily: 'monospace',
      fontFamilyFallback: const <String>['Menlo', 'Consolas', 'Roboto Mono'],
    );
