/// Small shared pieces every screen reuses: cards, empty states, error text,
/// the transport-precedence control, and a two-pane layout primitive.
library;

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../data/models.dart';
import '../state/providers.dart';
import '../theme/breakpoints.dart';
import '../theme/comrade_theme.dart';
import '../util/transport_precedence.dart';

/// The bordered panel `styles.css` calls `.ledger-panel`/`.composer` and
/// Compose calls `OutlinedCard`.
class SectionCard extends StatelessWidget {
  const SectionCard({
    required this.child,
    this.title,
    this.elevated = false,
    this.padding = const EdgeInsets.all(16),
    super.key,
  });

  final Widget child;
  final String? title;
  final bool elevated;
  final EdgeInsets padding;

  @override
  Widget build(BuildContext context) {
    final ComradeSurfaces surfaces = context.surfaces;
    return Container(
      width: double.infinity,
      decoration: BoxDecoration(
        color: elevated
            ? Theme.of(context).colorScheme.surfaceContainer
            : surfaces.panel,
        border: Border.all(color: surfaces.border),
        borderRadius: BorderRadius.circular(ComradeRadii.medium),
      ),
      padding: padding,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: <Widget>[
          if (title != null) ...<Widget>[
            Text(
              title!,
              style: Theme.of(context).textTheme.titleSmall?.copyWith(
                    color: Theme.of(context).colorScheme.primary,
                  ),
            ),
            const SizedBox(height: 8),
          ],
          child,
        ],
      ),
    );
  }
}

/// Inline, non-blocking error copy. Both frontends put failures next to the
/// control that caused them rather than in a modal.
class ErrorText extends StatelessWidget {
  const ErrorText(this.message, {super.key});

  final String? message;

  @override
  Widget build(BuildContext context) {
    if (message == null || message!.isEmpty) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.only(top: 4),
      child: Text(
        message!,
        style: Theme.of(context)
            .textTheme
            .bodySmall
            ?.copyWith(color: Theme.of(context).colorScheme.error),
      ),
    );
  }
}

/// Centred empty state with an optional call to action.
///
/// §7.4: no illustration, no headline-plus-subhead, at most one action — and
/// the copy reads as "what belongs here", in `mutedForeground` (the doc's own
/// words), not as a heading in the page's ordinary text colour. `title` and
/// `body` still exist as two params for source compatibility with call sites
/// written before §7.4, but both render at the same muted weight now, so a
/// caller passing only `title` already gets the one-line pattern the doc
/// asks for.
class EmptyState extends StatelessWidget {
  const EmptyState({
    required this.title,
    this.body,
    this.action,
    super.key,
  });

  final String title;
  final String? body;
  final Widget? action;

  @override
  Widget build(BuildContext context) => Center(
        child: Padding(
          padding: const EdgeInsets.all(32),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: <Widget>[
              Text(
                title,
                textAlign: TextAlign.center,
                style: Theme.of(context).textTheme.titleMedium?.copyWith(
                      color: Theme.of(context).colorScheme.onSurfaceVariant,
                    ),
              ),
              if (body != null)
                Padding(
                  padding: const EdgeInsets.only(top: 4, bottom: 16),
                  child: Text(
                    body!,
                    textAlign: TextAlign.center,
                    style: Theme.of(context).textTheme.bodySmall?.copyWith(
                          color: Theme.of(context).colorScheme.onSurfaceVariant,
                        ),
                  ),
                ),
              if (action != null) action!,
            ],
          ),
        ),
      );
}

/// A button that shows a spinner in place of its label while busy — the
/// `setBusy()` affordance the desktop SPA used on every submit.
class BusyButton extends StatelessWidget {
  const BusyButton({
    required this.label,
    required this.onPressed,
    this.busy = false,
    this.busyLabel,
    this.filled = true,
    this.expand = false,
    super.key,
  });

  final String label;
  final String? busyLabel;
  final VoidCallback? onPressed;
  final bool busy;
  final bool filled;
  final bool expand;

  @override
  Widget build(BuildContext context) {
    final Widget child = busy
        ? Row(
            mainAxisSize: MainAxisSize.min,
            children: <Widget>[
              const SizedBox(
                width: 16,
                height: 16,
                child: CircularProgressIndicator(strokeWidth: 2),
              ),
              if (busyLabel != null) ...<Widget>[
                const SizedBox(width: 8),
                Text(busyLabel!),
              ],
            ],
          )
        : Text(label);
    final Widget button = filled
        ? FilledButton(onPressed: busy ? null : onPressed, child: child)
        : OutlinedButton(onPressed: busy ? null : onPressed, child: child);
    return expand ? SizedBox(width: double.infinity, child: button) : button;
  }
}

/// Which transport Comrade tries first, as two glyphs in the app bar.
///
/// This replaces the always-on "Local mesh" banner. That banner spent a full
/// row of every screen on a line the user could read but not act on, and it
/// only ever appeared in the one workspace — so the signal that matters most
/// with no signal at all was both intrusive and, most of the time, absent.
///
/// Here the two routes are drawn in the order they are tried: the preferred one
/// leads at full strength (badged with how many devices are actually nearby),
/// the fallback follows, smaller and dimmed. That ordering *is* the setting.
/// Tapping opens a menu that swaps it.
///
/// Precedence is an order, not an exclusion — whichever route leads, a message
/// the preferred one cannot carry still takes the other (see
/// `docs/OFFLINE_DELIVERY.md`). Nothing here can strand a message.
///
/// The ordering itself lives in [TransportPrecedence], shared with the Android
/// frontend and tested on both.
class TransportPrecedenceAction extends ConsumerWidget {
  const TransportPrecedenceAction({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final WorkspaceInfo? workspace = ref.watch(workspaceProvider).value;
    if (workspace == null || !TransportPrecedence.isSwitchable(workspace.key)) {
      return const SizedBox.shrink();
    }
    final MeshStatus mesh =
        ref.watch(meshStatusProvider).value ?? const MeshStatus.idle();
    final LocalMeshState meshState = TransportPrecedence.meshStateOf(
      active: mesh.active,
      peerCount: mesh.peerCount,
    );
    final List<TransportRoute> order =
        TransportPrecedence.orderOf(workspace.key);
    final bool localFirst = order.first == TransportRoute.localNetwork;
    final ColorScheme colors = Theme.of(context).colorScheme;

    final Widget lead = Icon(
      _iconFor(order.first, meshState),
      key: const Key('transport-lead'),
      size: 22,
      color: _leadTint(colors, order.first, meshState),
    );
    final Widget fallback = Icon(
      _iconFor(order.last, meshState),
      key: const Key('transport-fallback'),
      size: 14,
      color: colors.onSurfaceVariant.withValues(alpha: 0.55),
    );

    return PopupMenuButton<TransportRoute>(
      key: const Key('transport-precedence'),
      tooltip: localFirst
          ? 'Sending over this network first'
          : 'Sending over the internet first',
      position: PopupMenuPosition.under,
      constraints: const BoxConstraints(minWidth: 240, maxWidth: 340),
      onSelected: (TransportRoute lead) => _select(context, ref, lead),
      itemBuilder: (BuildContext context) => <PopupMenuEntry<TransportRoute>>[
        PopupMenuItem<TransportRoute>(
          enabled: false,
          child: Text(
            _nearbyLabel(meshState, mesh.peerCount),
            style: Theme.of(context).textTheme.labelMedium?.copyWith(
                  color: colors.onSurfaceVariant,
                ),
          ),
        ),
        const PopupMenuDivider(),
        _choice(
          context,
          key: const Key('precedence-relay'),
          value: TransportRoute.relay,
          selected: !localFirst,
          icon: _iconFor(TransportRoute.relay, meshState),
          label: 'Internet first',
          detail: 'Relays, then nearby devices',
        ),
        _choice(
          context,
          key: const Key('precedence-local'),
          value: TransportRoute.localNetwork,
          selected: localFirst,
          icon: _iconFor(TransportRoute.localNetwork, meshState),
          label: 'This network first',
          detail: 'Nearby devices, then relays',
        ),
      ],
      child: Padding(
        // Matches an IconButton's hit box, so it lines up with the hamburger
        // on the other end of the bar.
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
        child: Badge.count(
          count: mesh.peerCount,
          isLabelVisible: meshState == LocalMeshState.reaching,
          child: Row(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.center,
            children: <Widget>[lead, const SizedBox(width: 2), fallback],
          ),
        ),
      ),
    );
  }

  /// Two silhouettes, not five: the local network's *live* state rides in
  /// [_leadTint] and the peer badge instead, because a glyph that morphs
  /// between shapes is harder to read at a glance than one whose colour
  /// changes. Kept identical to Android's `transportGlyph` for that reason.
  static IconData _iconFor(TransportRoute route, LocalMeshState mesh) =>
      switch (route) {
        TransportRoute.relay => Icons.cloud_outlined,
        TransportRoute.localNetwork => mesh == LocalMeshState.off
            ? Icons.portable_wifi_off
            : Icons.wifi_tethering,
      };

  /// The leading glyph's colour, which is where the local network's live state
  /// goes: the accent while it is actually reaching someone, the ordinary
  /// foreground while it looks, dimmed when the engine is not running.
  ///
  /// The relay glyph never changes. The workspace's `relayConnected` flag is a
  /// label, not a reachability probe, and drawing it as one would be a lie.
  static Color _leadTint(
    ColorScheme colors,
    TransportRoute route,
    LocalMeshState mesh,
  ) {
    if (route == TransportRoute.relay) return colors.onSurface;
    return switch (mesh) {
      LocalMeshState.reaching => colors.primary,
      LocalMeshState.searching => colors.onSurface,
      LocalMeshState.off => colors.onSurface.withValues(alpha: 0.45),
    };
  }

  static String _nearbyLabel(LocalMeshState mesh, int peerCount) =>
      switch (mesh) {
        LocalMeshState.off => 'Local network off',
        LocalMeshState.searching => 'Looking for nearby devices…',
        LocalMeshState.reaching =>
          peerCount == 1 ? '1 device nearby' : '$peerCount devices nearby',
      };

  PopupMenuItem<TransportRoute> _choice(
    BuildContext context, {
    required Key key,
    required TransportRoute value,
    required bool selected,
    required IconData icon,
    required String label,
    required String detail,
  }) {
    final ColorScheme colors = Theme.of(context).colorScheme;
    return PopupMenuItem<TransportRoute>(
      key: key,
      value: value,
      child: Row(
        children: <Widget>[
          Icon(icon,
              size: 20,
              color: selected ? colors.primary : colors.onSurfaceVariant),
          const SizedBox(width: 12),
          Flexible(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: <Widget>[
                Text(
                  label,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(color: selected ? colors.primary : null),
                ),
                Text(
                  detail,
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.labelSmall?.copyWith(
                        color: colors.onSurfaceVariant,
                      ),
                ),
              ],
            ),
          ),
          if (selected) ...<Widget>[
            const SizedBox(width: 8),
            Icon(Icons.check, size: 18, color: colors.primary),
          ],
        ],
      ),
    );
  }

  Future<void> _select(
    BuildContext context,
    WidgetRef ref,
    TransportRoute lead,
  ) async {
    final String target = TransportPrecedence.workspaceFor(lead);
    // The core's state machine rejects a self-transition, so re-picking the
    // order already in force must be a no-op rather than an error the user
    // caused by confirming what they already had.
    if (ref.read(workspaceProvider).value?.key == target) return;
    final ScaffoldMessengerState? messenger =
        ScaffoldMessenger.maybeOf(context);
    await ref.read(workspaceProvider.notifier).toggle(target);
    final Object? error = ref.read(workspaceProvider).error;
    if (error != null) {
      messenger?.showSnackBar(
        SnackBar(content: Text('Could not switch transport: $error')),
      );
    }
  }
}

/// List beside detail above [Breakpoints.expanded]; detail replaces list
/// below it.
///
/// This is the single primitive that makes the chat tab behave like the
/// desktop `.view-vault` grid on a wide window and like Android's pushed
/// conversation on a phone, from one widget tree.
class ListDetailPane extends StatelessWidget {
  const ListDetailPane({
    required this.list,
    required this.detail,
    required this.hasSelection,
    this.placeholder,
    super.key,
  });

  final Widget list;

  /// Built only when [hasSelection]; on a narrow window it fully replaces the
  /// list, so it owns the whole screen exactly like Compose's conversation
  /// view did.
  final Widget Function() detail;
  final bool hasSelection;
  final Widget? placeholder;

  @override
  Widget build(BuildContext context) {
    final ComradeWindowClass windowClass = windowClassOf(context);
    if (!windowClass.supportsTwoPane) {
      return hasSelection ? detail() : list;
    }
    final double width =
        Breakpoints.conversationListWidth(MediaQuery.sizeOf(context).width);
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: <Widget>[
        SizedBox(
          width: width,
          child: Material(color: context.surfaces.panel, child: list),
        ),
        VerticalDivider(width: 1, color: context.surfaces.border),
        Expanded(
          child: hasSelection
              ? detail()
              : (placeholder ??
                  const EmptyState(title: 'Select a conversation')),
        ),
      ],
    );
  }
}

/// A centred reading column, capped the way `.view-sabha` is
/// (`clamp(720px, 66vw, 1280px)`).
class ReadingColumn extends StatelessWidget {
  const ReadingColumn({required this.child, super.key});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final double width = MediaQuery.sizeOf(context).width;
    if (Breakpoints.classify(width) == ComradeWindowClass.compact) return child;
    return Center(
      child: ConstrainedBox(
        constraints:
            BoxConstraints(maxWidth: Breakpoints.feedColumnWidth(width)),
        child: child,
      ),
    );
  }
}
