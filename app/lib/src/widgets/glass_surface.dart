/// Comrade's floating-chrome material (`docs/DESIGN_SYSTEM.md` §3.4, §5).
///
/// Applied to chrome only — app bars, bottom navigation, modal sheets,
/// dialogs, popovers, snackbars, the composer, the call control dock — never
/// to message bubbles, list rows, feed items, the reading column, or the
/// media viewer (§2: "glass is never applied to dense or body content").
///
/// Flutter gets the **full** material (§5: "Flutter gets the full material.
/// `BackdropFilter` + `ImageFilter.blur` is first-party, so `app/` matches
/// desktop"): a real backdrop blur, saturation to restore the colour the blur
/// washes out, a specular top-edge highlight, and a depth shadow — not the
/// tint-only version Android's `Modifier.blur` is limited to.
library;

import 'dart:ui';

import 'package:flutter/material.dart';

import '../theme/comrade_theme.dart';

/// Which floating-chrome role a [GlassSurface] is playing. The two tiers
/// differ only in blur radius (§3.4): chrome sits directly over content,
/// sheets and dialogs sit further above it again.
enum GlassTier {
  /// App bars, bottom navigation, snackbars, popovers, the composer, the
  /// call control dock.
  chrome(ComradeGlass.blurChrome),

  /// Modal sheets and dialogs.
  sheet(ComradeGlass.blurSheet);

  const GlassTier(this.blurSigma);

  final double blurSigma;
}

/// The rounded-top-corners every modal sheet has, at §3.2's `xl` — "sheets,
/// dialogs, bubbles" — so every `showModalBottomSheet` call that wraps its
/// content in a [GlassSurface] rounds it the same way rather than each
/// picking its own number.
const BorderRadius kGlassSheetRadius = BorderRadius.vertical(
  top: Radius.circular(ComradeRadii.xl),
);

/// The saturation matrix `docs/DESIGN_SYSTEM.md` §3.4 asks for (180%): the
/// identity matrix scaled toward full saturation using the standard
/// luminance-preserving form, `s = 1.8`.
const List<double> _kSaturate180 = <double>[
  1.68, -0.544, -0.136, 0, 0, //
  -0.336, 1.28, -0.208, 0, 0, //
  -0.048, -0.336, 2.184, 0, 0, //
  0, 0, 0, 1, 0, //
];

/// Glass tier chrome: tint + backdrop blur + saturation + a specular top
/// edge + a depth shadow (§3.4), clipped so the blur cannot reach past its
/// own bounds — an unclipped [BackdropFilter] blurs the entire ancestor
/// subtree, which is the classic bug with this widget.
///
/// Falls back to an **opaque** fill (still with its border and shadow, still
/// legible) rather than rendering any transparency at all when the platform
/// asks for less of it (§4). Flutter has no dedicated
/// `prefers-reduced-transparency` signal, so [MediaQueryData.highContrast] is
/// the proxy; [MediaQueryData.disableAnimations] collapses the same way,
/// because "less transparency" and "less motion" are both accessibility
/// needs here, not preferences to average against a default, and a caller
/// that asked for either should never see blur.
class GlassSurface extends StatelessWidget {
  const GlassSurface({
    required this.child,
    this.tier = GlassTier.chrome,
    this.borderRadius = BorderRadius.zero,
    this.tint,
    this.border,
    this.highlight,
    this.shadowColor,
    super.key,
  });

  final Widget child;
  final GlassTier tier;
  final BorderRadius borderRadius;

  /// Overrides the tier's fill (§3.1 `popover`). Screens that must not
  /// follow the ambient [ComradeSurfaces] ramp — the call overlay is
  /// deliberately dark regardless of app theme, per [CallPalette]'s own
  /// doc — pass their own opaque colour here; it is still tinted to
  /// [ComradeGlass.tintAlpha] like every other tier fill.
  final Color? tint;

  /// Overrides the 1px border (default: `surfaces.border` at
  /// [ComradeGlass.borderAlpha]).
  final Color? border;

  /// Overrides the specular top-edge colour (default: `popoverForeground` at
  /// [ComradeGlass.highlightAlpha]).
  final Color? highlight;

  /// Overrides the depth-shadow colour (default: the ambient
  /// `ColorScheme.surface` at [ComradeGlass.shadowAlpha]).
  final Color? shadowColor;

  @override
  Widget build(BuildContext context) {
    final ComradeSurfaces surfaces = context.surfaces;
    final MediaQueryData mq = MediaQuery.of(context);
    final bool opaqueFallback = mq.highContrast || mq.disableAnimations;

    final Color baseTint = tint ?? surfaces.popover;
    final Color resolvedTint = opaqueFallback
        ? baseTint
        : baseTint.withValues(alpha: ComradeGlass.tintAlpha);
    final Color resolvedBorder =
        (border ?? surfaces.border).withValues(alpha: ComradeGlass.borderAlpha);
    final Color resolvedHighlight = (highlight ?? surfaces.popoverForeground)
        .withValues(alpha: ComradeGlass.highlightAlpha);
    final Color resolvedShadow = (shadowColor ?? context.colors.surface)
        .withValues(alpha: ComradeGlass.shadowAlpha);

    final Widget surface = DecoratedBox(
      decoration: BoxDecoration(
        color: resolvedTint,
        borderRadius: borderRadius,
        border: Border.all(color: resolvedBorder),
        boxShadow: <BoxShadow>[
          BoxShadow(
            color: resolvedShadow,
            blurRadius: 32,
            offset: const Offset(0, 8),
          ),
        ],
      ),
      // The specular top edge (§3.4: "inset 0 1px 0 <foreground at 10%>").
      // `BoxDecoration` has no inset-shadow primitive, so it is a 1px stripe
      // pinned to the top instead — the same read, without an extra
      // compositing layer. Skipping it is the most common way this material
      // ends up looking like a plain translucent box (§3.4).
      child: Stack(
        children: <Widget>[
          // `ListTile`, `InkWell` and friends paint their ink on the nearest
          // Material ancestor. The DecoratedBox above is not one, so without
          // this every tappable row inside a glass sheet or dialog loses its
          // ripple — and Flutter asserts about it in debug, which is how this
          // was caught. `MaterialType.transparency` supplies the ink surface
          // without painting a fill that would sit on top of the tint.
          Material(type: MaterialType.transparency, child: child),
          Positioned(
            top: 0,
            left: 0,
            right: 0,
            child: Container(height: 1, color: resolvedHighlight),
          ),
        ],
      ),
    );

    if (opaqueFallback) {
      return ClipRRect(borderRadius: borderRadius, child: surface);
    }

    return ClipRRect(
      borderRadius: borderRadius,
      child: Stack(
        children: <Widget>[
          // Samples and filters only what is *behind* this widget; it paints
          // nothing of its own, so it sits under `surface` rather than
          // wrapping it — the tint above is composited on top of the
          // blurred+saturated backdrop, not filtered along with it, which is
          // what keeps the tint's own alpha meaningful.
          Positioned.fill(
            child: ColorFiltered(
              colorFilter: const ColorFilter.matrix(_kSaturate180),
              child: BackdropFilter(
                filter: ImageFilter.blur(
                  sigmaX: tier.blurSigma,
                  sigmaY: tier.blurSigma,
                ),
                child: const SizedBox.expand(),
              ),
            ),
          ),
          surface,
        ],
      ),
    );
  }
}

/// The explicit 2px `ring` (§3.1, §4): every interactive element shows this
/// on `:focus-visible`'s Flutter equivalent — keyboard/mouse focus, not a
/// touch tap that happens to focus something — in every tier including
/// glass. Focus is never suppressed by any of §4's escape hatches.
///
/// The 2px offset is reserved as a transparent 2px margin at all times
/// rather than added only while focused, so a focus change never reflows the
/// element it rings.
class ComradeFocusRing extends StatefulWidget {
  const ComradeFocusRing({
    required this.focusNode,
    required this.child,
    this.borderRadius = const BorderRadius.all(Radius.circular(4)),
    super.key,
  });

  /// The **same** [FocusNode] the interactive [child] itself is focused
  /// through — e.g. `IconButton(focusNode: node)` inside
  /// `ComradeFocusRing(focusNode: node, ...)`.
  ///
  /// This does not wrap [child] in a second, independent `Focus`. An
  /// already-focusable Material widget (`IconButton`, `InkWell`) owns a
  /// `Focus` of its own; nesting another one *around* it — the first version
  /// of this widget did exactly that — gives keyboard traversal two stops
  /// for what is visually one control, and the outer stop cannot show a ring
  /// for a focus it never actually receives. Sharing the node instead means
  /// there is exactly one focusable thing in the tree, and this only
  /// listens to it.
  final FocusNode focusNode;

  final Widget child;
  final BorderRadius borderRadius;

  @override
  State<ComradeFocusRing> createState() => _ComradeFocusRingState();
}

class _ComradeFocusRingState extends State<ComradeFocusRing> {
  bool _visible = false;

  @override
  void initState() {
    super.initState();
    widget.focusNode.addListener(_recompute);
    FocusManager.instance.addHighlightModeListener(_onHighlightModeChange);
  }

  @override
  void didUpdateWidget(ComradeFocusRing oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.focusNode != widget.focusNode) {
      oldWidget.focusNode.removeListener(_recompute);
      widget.focusNode.addListener(_recompute);
    }
  }

  @override
  void dispose() {
    widget.focusNode.removeListener(_recompute);
    FocusManager.instance.removeHighlightModeListener(_onHighlightModeChange);
    super.dispose();
  }

  // Focus and highlight-mode change independently — switching from touch to
  // keyboard while a field is already focused should still bring the ring
  // up, not wait for the next focus change.
  void _onHighlightModeChange(FocusHighlightMode _) => _recompute();

  void _recompute() {
    // `traditional` is keyboard/mouse — the only input CSS calls
    // `:focus-visible` for. A touch tap that happens to focus a field should
    // not ring it.
    final bool show = widget.focusNode.hasFocus &&
        FocusManager.instance.highlightMode == FocusHighlightMode.traditional;
    if (show != _visible) setState(() => _visible = show);
  }

  @override
  Widget build(BuildContext context) {
    final Color ring = context.surfaces.ring;
    return Container(
      margin: const EdgeInsets.all(2),
      decoration: BoxDecoration(
        borderRadius: widget.borderRadius,
        border: Border.all(
          color: _visible ? ring : Colors.transparent,
          width: 2,
        ),
      ),
      child: widget.child,
    );
  }
}

/// The rounded corners a centred dialog has, at §3.2's `xl` — same tier as
/// [kGlassSheetRadius], all four corners rather than just the top two.
const BorderRadius kGlassDialogRadius = BorderRadius.all(
  Radius.circular(ComradeRadii.xl),
);

/// Wraps whatever [builder] returns in glass tier chrome (§2, §3.4) and
/// hands the result to [showDialog] — the shared way every `showDialog`
/// call site in this app gets the glass tier.
///
/// This has to happen per call site rather than through `DialogThemeData`
/// (`ComradeTheme`'s own advice for the rest of the token layer) because
/// `DialogThemeData` has no slot for a widget: no `BackdropFilter`, and no
/// way to react to §4's escape hatches, which need a live `BuildContext`
/// — `ThemeData` is built once, well before any dialog exists, and cannot
/// read `MediaQuery` at all. [GlassSurface] already reads it on every
/// build; this only has to put one in the tree.
///
/// The `AlertDialog` (or whatever [builder] returns) still gets its shape
/// from the app's [ComradeTheme] — [kGlassDialogRadius] matches it — but its
/// own fill, elevation and shadow are turned off here, scoped to this one
/// dialog, so [GlassSurface] is the only thing painting a background. No
/// other `showDialog` call site is affected.
Future<T?> showGlassDialog<T>(
  BuildContext context, {
  required WidgetBuilder builder,
  bool barrierDismissible = true,
}) =>
    showDialog<T>(
      context: context,
      barrierDismissible: barrierDismissible,
      builder: (BuildContext dialogContext) => GlassSurface(
        tier: GlassTier.sheet,
        borderRadius: kGlassDialogRadius,
        child: Theme(
          data: Theme.of(dialogContext).copyWith(
            dialogTheme: Theme.of(dialogContext).dialogTheme.copyWith(
                  backgroundColor: Colors.transparent,
                  elevation: 0,
                  shadowColor: Colors.transparent,
                  surfaceTintColor: Colors.transparent,
                ),
          ),
          child: builder(dialogContext),
        ),
      ),
    );
