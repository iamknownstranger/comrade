/// Loading placeholders for lists whose row shape is known before the
/// content arrives (`docs/DESIGN_SYSTEM.md` §7.3).
///
/// Every `CircularProgressIndicator` this replaces was centred over a blank
/// screen — the most dated pattern still in this app, and one that *hides*
/// the layout it is about to reveal, so arrival reads as a jump. A skeleton
/// shows the real row geometry instead: an outline the content fades into
/// rather than a spinner the content interrupts.
///
/// Two rules from §7.3 that are easy to get backwards:
///  * The pulse is a shared opacity fade, never a travelling gradient sweep
///    — a sweep draws the eye to the loader itself, which is the opposite of
///    the point.
///  * [MediaQuery.disableAnimations] freezes it, the same escape hatch
///    `GlassSurface` already honours (§4.3) — still a layout preview, just
///    not animated.
///
/// Reserved for lists; a spinner is still correct for a bounded, blocking,
/// indeterminate action the user just triggered (unlocking the vault,
/// sending) — §7.3 says so explicitly, and this file does not touch those.
library;

import 'package:flutter/material.dart';

import '../theme/comrade_theme.dart';

/// One rounded-rect (or circle) placeholder — an avatar, a line of text, a
/// trailing chip — filled with `muted` (§3.1). The unit every skeleton row
/// below is built out of.
class SkeletonBlock extends StatelessWidget {
  const SkeletonBlock({
    required this.width,
    required this.height,
    this.shape = BoxShape.rectangle,
    super.key,
  });

  final double width;
  final double height;
  final BoxShape shape;

  @override
  Widget build(BuildContext context) => Container(
        width: width,
        height: height,
        decoration: BoxDecoration(
          color: context.surfaces.muted,
          shape: shape,
          borderRadius: shape == BoxShape.rectangle
              ? BorderRadius.circular(ComradeRadii.sm)
              : null,
        ),
      );
}

/// Wraps [rowCount] copies of [rowBuilder] in one shared opacity pulse and
/// lays them out with [separator] between each — the reusable widget §7.3
/// asks for; screens supply their own row shape rather than a generic one,
/// because "real row geometry" means the chat list's skeleton has to look
/// like a chat row and the journal's like a journal card.
///
/// §7.3 bounds [rowCount] at 3–6; nothing enforces that beyond the doc
/// comment on each factory, because a caller mid-transition (a window
/// resize that changes how many rows fit) is a better reason to allow more
/// than a hard assert is a reason to crash it.
class ListSkeleton extends StatefulWidget {
  const ListSkeleton({
    required this.rowCount,
    required this.rowBuilder,
    this.separator,
    super.key,
  });

  final int rowCount;
  final WidgetBuilder rowBuilder;
  final Widget? separator;

  /// The peer-row shape shared by every list keyed on a person — the
  /// conversation list, call history, message requests: an avatar circle, a
  /// name line, a snippet line, and a trailing meta chip, the same four
  /// pieces `_ConversationRow`/`_CallHistoryRow` actually draw.
  factory ListSkeleton.peerRows({
    int rowCount = 5,
    bool divided = true,
    Key? key,
  }) =>
      ListSkeleton(
        key: key,
        rowCount: rowCount,
        rowBuilder: (BuildContext context) => const _SkeletonPeerRow(),
        separator: divided ? const Divider(indent: 76, height: 1) : null,
      );

  /// The card shape journal entries and feed posts share: a short header
  /// line (avatar or mood + timestamp) over two lines of body text, inside
  /// the same [SectionCard]-shaped block those rows actually render in.
  factory ListSkeleton.cardRows({int rowCount = 3, Key? key}) => ListSkeleton(
        key: key,
        rowCount: rowCount,
        rowBuilder: (BuildContext context) => const _SkeletonCardRow(),
        separator: const SizedBox(height: ComradeSpacing.space3),
      );

  @override
  State<ListSkeleton> createState() => _ListSkeletonState();
}

class _ListSkeletonState extends State<ListSkeleton>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 1100),
  );

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final bool reduced = MediaQuery.of(context).disableAnimations;
    // Started/stopped from build rather than initState/didChangeDependencies:
    // MediaQuery is only reliably read here, and toggling reduced motion
    // mid-session (a live accessibility setting) must actually stop the
    // ticker rather than merely ignore its value — an AnimatedBuilder that
    // keeps listening to a controller nobody meant to still run is exactly
    // the kind of state this widget exists to get right.
    if (reduced) {
      _controller.stop();
    } else if (!_controller.isAnimating) {
      _controller.repeat(reverse: true);
    }

    final List<Widget> rows = <Widget>[
      for (int i = 0; i < widget.rowCount; i++) ...<Widget>[
        widget.rowBuilder(context),
        if (widget.separator != null && i != widget.rowCount - 1)
          widget.separator!,
      ],
    ];
    final Widget column = Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: rows,
    );

    if (reduced) {
      // Still a preview of the layout, just not animated — the same
      // reduced-motion contract `GlassSurface` honours (§4.3).
      return Opacity(opacity: 0.55, child: column);
    }

    return AnimatedBuilder(
      animation: _controller,
      builder: (BuildContext context, Widget? child) => Opacity(
        // Pulses between about a third and full strength — never fully
        // transparent, so the shape stays a legible preview at every point
        // in the cycle, and never a travelling gradient (§7.3).
        opacity: 0.35 + (_controller.value * 0.45),
        child: child,
      ),
      child: column,
    );
  }
}

class _SkeletonPeerRow extends StatelessWidget {
  const _SkeletonPeerRow();

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.symmetric(
          horizontal: ComradeSpacing.space4,
          vertical: ComradeSpacing.space3,
        ),
        child: Row(
          children: <Widget>[
            const SkeletonBlock(width: 46, height: 46, shape: BoxShape.circle),
            const SizedBox(width: ComradeSpacing.space3),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: <Widget>[
                  const SkeletonBlock(width: 140, height: 16),
                  const SizedBox(height: ComradeSpacing.space2),
                  SkeletonBlock(
                    width: MediaQuery.sizeOf(context).width * 0.4,
                    height: 12,
                  ),
                ],
              ),
            ),
            const SizedBox(width: ComradeSpacing.space2),
            const SkeletonBlock(width: 32, height: 8),
          ],
        ),
      );
}

class _SkeletonCardRow extends StatelessWidget {
  const _SkeletonCardRow();

  @override
  Widget build(BuildContext context) {
    final ComradeSurfaces surfaces = context.surfaces;
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.all(ComradeSpacing.space3),
      decoration: BoxDecoration(
        color: surfaces.panel,
        border: Border.all(color: surfaces.border),
        borderRadius: BorderRadius.circular(ComradeRadii.medium),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: <Widget>[
          Row(
            children: <Widget>[
              const SkeletonBlock(
                  width: 28, height: 28, shape: BoxShape.circle),
              const SizedBox(width: ComradeSpacing.space2),
              const SkeletonBlock(width: 80, height: 12),
            ],
          ),
          const SizedBox(height: ComradeSpacing.space3),
          const SkeletonBlock(width: double.infinity, height: 14),
          const SizedBox(height: ComradeSpacing.space2),
          SkeletonBlock(
            width: MediaQuery.sizeOf(context).width * 0.6,
            height: 14,
          ),
        ],
      ),
    );
  }
}
