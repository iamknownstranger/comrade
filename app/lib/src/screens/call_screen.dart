/// The full-screen call overlay: ringing, connecting, active, ended.
///
/// Port of `call/CallScreen.kt`. It observes [callProvider] and renders every
/// phase; the media itself comes from whatever [CallEngine] is installed (see
/// `state/call_providers.dart` for why that is a seam and not an
/// implementation).
///
/// Details kept because they were deliberate:
///  * one composition subtree for Connecting **and** Active, so the video
///    surfaces are not torn down and recreated exactly as the first frames
///    arrive;
///  * tap the picture-in-picture tile to swap it with the full-screen view;
///  * the local track mirrors wherever it renders, the remote never does.
///
/// Details that are new, and why:
///  * **A video call opens full screen and then gets out of the way.** The
///    controls linger for [_chromeLinger] after the call connects and then fade
///    out; a tap anywhere brings them back, a second tap dismisses them again.
///    This is FaceTime's behaviour and it exists because the thing worth
///    looking at is the person, not five buttons. Audio calls keep their
///    controls permanently — there is nothing underneath to reveal.
///  * **A network-strength indicator instead of a verification code.** Four
///    Telegram-style bars (`widgets/signal_bars.dart`) replace the 4-emoji
///    short authentication string that used to live in this corner. The SAS was
///    an out-of-band man-in-the-middle check on the media path; Comrade's SDP
///    already rides the NIP-44 gift-wrapped DM channel, so the fingerprints are
///    authenticated by the peer's Nostr key before a call is ever answered, and
///    the check was near-redundant. Signal strength is the thing people
///    actually need mid-call. `comrade_core::call::derive_sas` is still there
///    and still tested — nothing surfaces it.
///  * **Mute and camera slash themselves.** `widgets/slashed_icon.dart` draws
///    the line on rather than swapping glyph, because a swapped glyph reads as
///    "the icon changed", not as "your mic is off".
///  * **"Video paused" is a state, not a frozen frame.** Whenever a side stops
///    sending — camera off, or capture suspended because that app is in the
///    background — the tile says so over the avatar.
///  * **The chat button shrinks the call rather than hiding it.** Native
///    picture-in-picture where the OS gives us a window, a draggable in-app
///    tile everywhere else.
///  * **A short bar and a ⋮ dock, in Telegram's order.** Camera, mic, output,
///    ⋮, End — and everything else behind the ⋮. See `call_controls.dart`: the
///    bar used to hold every control, which on a video call was six buttons on
///    a screen that fits five, and the one pushed off the end was End call.
library;

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../state/call_providers.dart';
import '../theme/comrade_theme.dart';
import '../util/display_name.dart';
import '../widgets/glass_surface.dart';
import '../widgets/peer_avatar.dart';
import '../widgets/signal_bars.dart';
import '../widgets/slashed_icon.dart';
import 'call_controls.dart';

/// How long the controls stay up before a video call fades them out — long
/// enough to hit "mute" without hunting, short enough that the call is mostly
/// the other person's face.
const Duration _chromeLinger = Duration(seconds: 4);

/// Fade duration for the controls. Deliberately unhurried: a fast fade reads
/// as a glitch, and this one is triggered by doing nothing at all.
const Duration _chromeFade = Duration(milliseconds: 320);

/// Diameter of a control in the bar. Five of these plus their slots fit the
/// ~336dp a 360dp-wide phone leaves after padding — see [kMaxPrimaryCallControls].
const double _barButtonSize = 56;

/// Shows the call overlay when a call is in flight, nothing when idle.
/// Callers place this last in their layout stack so it covers the app.
class CallOverlay extends ConsumerWidget {
  const CallOverlay({this.onOpenChat, super.key});

  /// Invoked by the in-call chat button, before the call shrinks into
  /// picture-in-picture. The shell navigates to the conversation with this
  /// peer; the call keeps running either way.
  final void Function(String peer, String label)? onOpenChat;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final CallSession session = ref.watch(callProvider);
    final CallUiState state = session.state;
    if (state is CallIdle) return const SizedBox.shrink();

    // In-app picture-in-picture: the call gives up the screen to a corner tile
    // so the conversation behind it is usable. Only meaningful mid-call —
    // ringing and the ended card stay full screen.
    if (session.pip == CallPipMode.inApp &&
        (state is CallConnecting || state is CallActive)) {
      return FloatingCallTile(session: session);
    }

    return Positioned.fill(
      child: Material(
        color: CallPalette.background,
        child: switch (state) {
          CallRinging() => _RingingContent(state: state),
          // One subtree for both in-call phases.
          CallConnecting() ||
          CallActive() =>
            _InCallContent(session: session, onOpenChat: onOpenChat),
          CallEnded() => _EndedContent(state: state),
          CallIdle() => const SizedBox.shrink(),
        },
      ),
    );
  }
}

class _RingingContent extends ConsumerWidget {
  const _RingingContent({required this.state});

  final CallRinging state;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final String status = state.isIncoming
        ? (state.isVideo ? 'Incoming video call' : 'Incoming voice call')
        : (state.remoteRinging ? 'Ringing…' : 'Calling…');
    final CallController controller = ref.read(callProvider.notifier);
    return Column(
      children: <Widget>[
        Expanded(
          child: Padding(
            padding: const EdgeInsets.only(top: 72),
            child: _PeerHeader(
              peer: state.peerNpub,
              label: state.label,
              status: status,
            ),
          ),
        ),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 44, vertical: 44),
          child: Row(
            mainAxisAlignment: state.isIncoming
                ? MainAxisAlignment.spaceBetween
                : MainAxisAlignment.center,
            children: <Widget>[
              if (state.isIncoming) ...<Widget>[
                CallActionButton(
                  icon: Icons.call_end,
                  label: 'Decline',
                  background: CallPalette.hangup,
                  size: 68,
                  onPressed: controller.reject,
                ),
                CallActionButton(
                  icon: state.isVideo ? Icons.videocam : Icons.call,
                  label: 'Accept',
                  background: CallPalette.accept,
                  size: 68,
                  onPressed: controller.accept,
                ),
              ] else
                CallActionButton(
                  icon: Icons.call_end,
                  label: 'Cancel',
                  background: CallPalette.hangup,
                  size: 68,
                  onPressed: controller.hangup,
                ),
            ],
          ),
        ),
      ],
    );
  }
}

class _InCallContent extends ConsumerStatefulWidget {
  const _InCallContent({required this.session, this.onOpenChat});

  final CallSession session;
  final void Function(String peer, String label)? onOpenChat;

  @override
  ConsumerState<_InCallContent> createState() => _InCallContentState();
}

class _InCallContentState extends ConsumerState<_InCallContent>
    with WidgetsBindingObserver {
  /// Tap the picture-in-picture tile to swap it with the full-screen view.
  /// Only changes which track renders where — the tracks are untouched.
  bool _swapped = false;

  /// Whether the controls and the name pill are on screen. Always true for an
  /// audio call; self-hiding for video (see [_armAutoHide]).
  bool _chromeVisible = true;

  /// Whether the ⋮ dock — screen share, camera flip, chat — is open.
  bool _dockOpen = false;

  Timer? _hideTimer;
  bool _immersive = false;

  bool get _isVideo => widget.session.state.video;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _applyImmersive();
    _armAutoHide();
  }

  @override
  void didUpdateWidget(_InCallContent oldWidget) {
    super.didUpdateWidget(oldWidget);
    // Connecting → Active on a video call is the moment the picture appears:
    // start the fade countdown from there, not from when the screen was built.
    final bool becameActive = oldWidget.session.state is CallConnecting &&
        widget.session.state is CallActive;
    if (becameActive) {
      _chromeVisible = true;
      _armAutoHide();
    }
    if (oldWidget.session.state.video != _isVideo) _applyImmersive();
  }

  @override
  void dispose() {
    _hideTimer?.cancel();
    WidgetsBinding.instance.removeObserver(this);
    if (_immersive) {
      unawaited(
        SystemChrome.setEnabledSystemUIMode(SystemUiMode.edgeToEdge),
      );
    }
    super.dispose();
  }

  /// A video call takes the whole screen, status bar and all — and gives it
  /// back the moment the call is over.
  void _applyImmersive() {
    final bool wanted = _isVideo;
    if (wanted == _immersive) return;
    _immersive = wanted;
    unawaited(
      SystemChrome.setEnabledSystemUIMode(
        wanted ? SystemUiMode.immersiveSticky : SystemUiMode.edgeToEdge,
      ),
    );
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    // Backgrounding with no picture-in-picture window means nothing is showing
    // the camera — so stop capturing it. The controller knows about the PiP
    // exception; this only reports the lifecycle.
    unawaited(ref.read(callProvider.notifier).onAppLifecycleChanged(state));
  }

  void _armAutoHide() {
    _hideTimer?.cancel();
    if (!_isVideo) return;
    // An open dock is someone mid-decision — fading the controls out from
    // under them would be the rudest possible moment for it.
    if (_dockOpen) return;
    _hideTimer = Timer(_chromeLinger, () {
      if (mounted) setState(() => _chromeVisible = false);
    });
  }

  /// Tap anywhere on the picture: dismiss the dock if it is open, otherwise
  /// reveal the controls or dismiss them early.
  void _toggleChrome() {
    if (_dockOpen) {
      _closeDock();
      return;
    }
    if (!_isVideo) return;
    _hideTimer?.cancel();
    setState(() => _chromeVisible = !_chromeVisible);
    if (_chromeVisible) _armAutoHide();
  }

  /// Any deliberate interaction keeps the controls up rather than having them
  /// vanish mid-gesture.
  void _keepChromeUp() {
    if (_isVideo && _chromeVisible) _armAutoHide();
  }

  void _toggleDock() {
    _hideTimer?.cancel();
    setState(() => _dockOpen = !_dockOpen);
    if (!_dockOpen) _armAutoHide();
  }

  void _closeDock() {
    if (!_dockOpen) return;
    setState(() => _dockOpen = false);
    _armAutoHide();
  }

  Future<void> _openChat() async {
    final CallUiState state = widget.session.state;
    final String? peer = state.peer;
    if (peer != null && peer.isNotEmpty) {
      widget.onOpenChat?.call(peer, state.peerLabel);
    }
    await ref.read(callProvider.notifier).openChat();
  }

  @override
  Widget build(BuildContext context) {
    final CallSession session = widget.session;
    final CallUiState state = session.state;
    final bool connecting = state is CallConnecting;
    final CallEngine engine = ref.watch(callEngineProvider);

    // The OS is drawing us into a thumbnail: the picture is all that fits, and
    // the controls belong to the PiP window's own action bar.
    if (session.pip == CallPipMode.native) {
      return _NativePipContent(session: session, engine: engine);
    }

    // Quality is only meaningful once the media path is up.
    final CallQuality quality =
        connecting ? CallQuality.unknown : session.quality;

    return Stack(
      fit: StackFit.expand,
      children: <Widget>[
        // A voice call that starts sharing a screen grows a video stage it did
        // not have — which is why this asks the session, not the call kind.
        if (session.showsVideoStage)
          _videoStage(context, session, engine, connecting, quality)
        else
          Center(
            child: _PeerHeader(
              peer: state.peer ?? '',
              label: state.peerLabel,
              status: connecting
                  ? 'Connecting…'
                  : _CallTimer.labelFor(state as CallActive),
              extra: <Widget>[
                if (!connecting) ConnectionStrengthIndicator(quality: quality),
              ],
              liveTimerFor: connecting ? null : state as CallActive,
            ),
          ),
        _chrome(
          child: Align(
            alignment: Alignment.bottomCenter,
            child: Stack(
              alignment: Alignment.bottomCenter,
              children: <Widget>[
                // The scrim that keeps white icons legible over a bright
                // frame. Deliberately does not take pointer input: a
                // `BoxDecoration` hit-tests its whole rectangle, so without
                // this the dark band silently swallowed every tap that missed
                // a button — including the one meant to dismiss the dock.
                if (session.showsVideoStage)
                  const Positioned.fill(
                    child: IgnorePointer(
                      child: DecoratedBox(
                        decoration: BoxDecoration(
                          gradient: LinearGradient(
                            begin: Alignment.topCenter,
                            end: Alignment.bottomCenter,
                            colors: <Color>[
                              Colors.transparent,
                              Color(0xB3000000)
                            ],
                          ),
                        ),
                      ),
                    ),
                  ),
                // The controls' own band *does* absorb, so the gaps between
                // buttons are not a way to accidentally dismiss the chrome
                // you are currently using.
                GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onTap: () {
                    _closeDock();
                    _keepChromeUp();
                  },
                  child: Padding(
                    padding: const EdgeInsets.fromLTRB(12, 28, 12, 36),
                    child: _controlsArea(session),
                  ),
                ),
              ],
            ),
          ),
        ),
      ],
    );
  }

  /// The bar, with the ⋮ dock stacked above it when open.
  Widget _controlsArea(CallSession session) {
    final CallControlLayout layout = layoutCallControls(
      video: session.state.video,
      cameraOn: session.cameraOn,
    );
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.end,
      children: <Widget>[
        if (_dockOpen)
          Padding(
            padding: const EdgeInsets.only(right: 8, bottom: 14),
            child: _CallOptionsDock(
              items: layout.dock,
              session: session,
              onScreenShare: () {
                _closeDock();
                unawaited(
                  ref.read(callProvider.notifier).toggleScreenShare(),
                );
              },
              onSwitchCamera: () {
                _closeDock();
                unawaited(ref.read(callProvider.notifier).switchCamera());
              },
              onChat: () {
                _closeDock();
                unawaited(_openChat());
              },
            ),
          ),
        _controls(session, layout.primary),
      ],
    );
  }

  /// The bar itself: [CallControl]s laid out one per equal-width slot.
  ///
  /// Equal slots rather than a centred row with fixed gaps, so the bar is
  /// exactly as wide as the screen and a button physically cannot be pushed off
  /// the end of it — which is the failure this whole arrangement replaces.
  Widget _controls(CallSession session, List<CallControl> primary) => Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          for (final CallControl control in primary)
            Expanded(
              child: Align(
                alignment: Alignment.topCenter,
                child: _controlButton(control, session),
              ),
            ),
        ],
      );

  Widget _controlButton(CallControl control, CallSession session) {
    final CallController controller = ref.read(callProvider.notifier);
    switch (control) {
      case CallControl.camera:
        return CallActionButton(
          key: const Key('call-camera'),
          icon: Icons.videocam,
          slashed: !session.cameraOn,
          label: session.cameraOn ? 'Camera' : 'Camera off',
          size: _barButtonSize,
          background: session.cameraOn
              ? CallPalette.controlIdle
              : CallPalette.controlActive,
          tint: session.cameraOn ? Colors.white : CallPalette.background,
          onPressed: () {
            _keepChromeUp();
            unawaited(controller.toggleCamera());
          },
        );
      case CallControl.mic:
        // The mic slashes itself rather than swapping glyph — a struck-through
        // mic is "your mic is off"; a different picture is "the icon changed".
        return CallActionButton(
          key: const Key('call-mute'),
          icon: Icons.mic,
          slashed: session.muted,
          label: session.muted ? 'Unmute' : 'Mute',
          size: _barButtonSize,
          background: session.muted
              ? CallPalette.controlActive
              : CallPalette.controlIdle,
          tint: session.muted ? CallPalette.background : Colors.white,
          onPressed: () {
            _keepChromeUp();
            unawaited(controller.toggleMute());
          },
        );
      case CallControl.speaker:
        return AudioRouteButton(
          session: session,
          onInteract: _keepChromeUp,
          size: _barButtonSize,
        );
      case CallControl.more:
        // Lit while something behind it is running, so "you are sharing your
        // screen" is legible with the dock shut.
        return CallActionButton(
          key: const Key('call-more'),
          icon: Icons.more_vert,
          label: 'More',
          size: _barButtonSize,
          background: session.screenSharing
              ? CallPalette.controlActive
              : CallPalette.controlIdle,
          tint: session.screenSharing ? CallPalette.background : Colors.white,
          onPressed: _toggleDock,
        );
      case CallControl.hangup:
        return CallActionButton(
          key: const Key('call-hangup'),
          icon: Icons.call_end,
          label: 'End',
          size: _barButtonSize,
          background: CallPalette.hangup,
          onPressed: controller.hangup,
        );
      // Dock-only controls; `layoutCallControls` never puts them in the bar.
      case CallControl.screenShare:
      case CallControl.switchCamera:
      case CallControl.chat:
        return const SizedBox.shrink();
    }
  }

  /// Wraps the self-hiding parts of the UI. Fades rather than unmounts, and
  /// stops taking taps while invisible so a hidden button can't be pressed.
  Widget _chrome({required Widget child}) => AnimatedOpacity(
        opacity: _chromeVisible ? 1 : 0,
        duration: _chromeFade,
        curve: Curves.easeOut,
        child: IgnorePointer(ignoring: !_chromeVisible, child: child),
      );

  Widget _videoStage(
    BuildContext context,
    CallSession session,
    CallEngine engine,
    bool connecting,
    CallQuality quality,
  ) {
    final CallUiState state = session.state;
    final Widget? mainView =
        engine.videoView(local: _swapped, mirror: _swapped);
    final bool pipIsLocal = !_swapped;
    final Widget? pipView =
        engine.videoView(local: pipIsLocal, mirror: pipIsLocal);

    // Whichever track each surface is showing, "paused" is that side having
    // stopped sending — not a renderer that has no frames yet.
    final bool mainPaused =
        _swapped ? session.localVideoPaused : session.remoteVideoPaused;
    final bool tilePaused =
        pipIsLocal ? session.localVideoPaused : session.remoteVideoPaused;

    return Stack(
      fit: StackFit.expand,
      children: <Widget>[
        _VideoSurface(
          view: mainView,
          peer: state.peer ?? '',
          label: state.peerLabel,
          paused: mainPaused,
          large: true,
        ),
        // The tap target for revealing/dismissing the controls. Below every
        // control in the stack, so buttons keep their own taps.
        Positioned.fill(
          child: GestureDetector(
            key: const Key('call-stage'),
            behavior: HitTestBehavior.opaque,
            onTap: _toggleChrome,
          ),
        ),
        // Self-view. Deliberately *not* part of the fading chrome — FaceTime
        // keeps it up, and it is the only feedback that your own camera works.
        Positioned(
          top: 16,
          right: 16,
          child: GestureDetector(
            onTap: () {
              _keepChromeUp();
              setState(() => _swapped = !_swapped);
            },
            child: Semantics(
              button: true,
              label: 'Swap video',
              child: Container(
                width: 110,
                height: 156,
                clipBehavior: Clip.antiAlias,
                decoration: BoxDecoration(
                  color: CallPalette.pipBackground,
                  borderRadius: BorderRadius.circular(14),
                ),
                child: Stack(
                  fit: StackFit.expand,
                  children: <Widget>[
                    _VideoSurface(
                      view: pipView,
                      peer: pipIsLocal ? '' : (state.peer ?? ''),
                      label: pipIsLocal ? 'You' : state.peerLabel,
                      paused: tilePaused,
                      large: false,
                    ),
                    if (pipIsLocal && !session.localVideoPaused)
                      Align(
                        alignment: Alignment.bottomCenter,
                        child: Padding(
                          padding: const EdgeInsets.only(bottom: 6),
                          child: CallActionButton(
                            icon: Icons.flip_camera_ios,
                            label: null,
                            background: const Color(0x66000000),
                            size: 34,
                            onPressed: () {
                              _keepChromeUp();
                              unawaited(ref
                                  .read(callProvider.notifier)
                                  .switchCamera());
                            },
                          ),
                        ),
                      ),
                  ],
                ),
              ),
            ),
          ),
        ),
        // Name + duration/status pill and the signal bars, kept clear of the
        // self-view tile. Fades with the controls.
        Positioned(
          left: 16,
          top: 20,
          right: 142,
          child: _chrome(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: <Widget>[
                Container(
                  decoration: BoxDecoration(
                    color: const Color(0x66000000),
                    borderRadius: BorderRadius.circular(14),
                  ),
                  padding:
                      const EdgeInsets.symmetric(horizontal: 14, vertical: 8),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: <Widget>[
                      Text(
                        state.peerLabel,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style:
                            const TextStyle(color: Colors.white, fontSize: 15),
                      ),
                      if (connecting)
                        const Text(
                          'Connecting…',
                          style: TextStyle(
                            color: CallPalette.secondaryText,
                            fontSize: 13,
                            fontFamily: 'monospace',
                          ),
                        )
                      else
                        _CallTimer(active: state as CallActive),
                    ],
                  ),
                ),
                if (!connecting) ...<Widget>[
                  const SizedBox(height: 6),
                  ConnectionStrengthIndicator(quality: quality),
                ],
              ],
            ),
          ),
        ),
      ],
    );
  }
}

/// What the OS picture-in-picture window shows: the picture, and nothing else.
///
/// A PiP window is a thumbnail with its own system-drawn action bar, so app
/// controls in it would be unreadable and unhittable. Tapping it asks the OS to
/// restore the app, which is the platform gesture — we do not intercept it.
class _NativePipContent extends StatelessWidget {
  const _NativePipContent({required this.session, required this.engine});

  final CallSession session;
  final CallEngine engine;

  @override
  Widget build(BuildContext context) {
    final CallUiState state = session.state;
    return _VideoSurface(
      view: state.video ? engine.videoView(local: false, mirror: false) : null,
      peer: state.peer ?? '',
      label: state.peerLabel,
      paused: state.video && session.remoteVideoPaused,
      large: false,
    );
  }
}

/// One video surface: the live picture, with the "no picture" state drawn *over*
/// it rather than in place of it.
///
/// The distinction is the whole point of this widget. Swapping the surface out
/// for a placeholder — which is what this screen used to do — unmounts the
/// renderer, and unmounting a renderer detaches its sink from the track. The
/// frames the peer sends when they un-pause then have nowhere to land until a
/// brand-new renderer has been built and re-attached, so a pause on one side
/// could leave the other side's picture not coming back. Keeping the surface
/// mounted and covering it means resuming is instant, because nothing was ever
/// torn down: the cover simply lifts.
///
/// [view] is null only when the engine genuinely has no renderer for that track
/// (an audio call, or before the media path exists), and then the cover is all
/// there is.
class _VideoSurface extends StatelessWidget {
  const _VideoSurface({
    required this.view,
    required this.peer,
    required this.label,
    required this.paused,
    required this.large,
  });

  final Widget? view;
  final String peer;
  final String label;
  final bool paused;
  final bool large;

  @override
  Widget build(BuildContext context) => Stack(
        fit: StackFit.expand,
        children: <Widget>[
          if (view != null) view!,
          // Opaque, so a frozen last frame is never left showing underneath.
          if (view == null || paused)
            _VideoPlaceholder(
              peer: peer,
              label: label,
              paused: paused,
              large: large,
            ),
        ],
      );
}

/// The call shrunk into a corner of this app — what the in-call chat button
/// produces. Draggable, tappable to restore, and it keeps the two controls
/// worth having at that size.
///
/// This, not an OS picture-in-picture window, is what "chat during a call"
/// needs: only one window can show the call *and* the conversation at once, and
/// it has to be ours. Native PiP is for leaving the app entirely.
class FloatingCallTile extends ConsumerStatefulWidget {
  const FloatingCallTile({required this.session, super.key});

  final CallSession session;

  @override
  ConsumerState<FloatingCallTile> createState() => _FloatingCallTileState();
}

class _FloatingCallTileState extends ConsumerState<FloatingCallTile> {
  /// The tile's top-left in logical pixels, or null until the first layout
  /// places it at its default corner.
  ///
  /// **Top**-right is that default, and the corner is a deliberate choice: the
  /// bottom right of a conversation is the send button, and a call tile parked
  /// on top of it would defeat the one thing this mode exists for. From there
  /// it goes wherever it is dragged.
  Offset? _position;

  /// True while a finger is down, so the snap animation doesn't fight the drag.
  bool _dragging = false;

  static const double _width = 132;
  static const double _height = 186;
  static const double _margin = 12;

  /// Where the tile sits before anyone moves it.
  Offset _defaultPosition(Size screen) =>
      Offset(screen.width - _width - _margin, _margin);

  /// Keep the whole tile on screen, whatever the drag or a rotation did. Also
  /// what stops a tile dragged to the edge of a phone from becoming
  /// unreachable when the window resizes (or a PiP window restores).
  Offset _clamp(Offset p, Size screen) => Offset(
        p.dx.clamp(_margin,
            (screen.width - _width - _margin).clamp(_margin, double.infinity)),
        p.dy.clamp(
            _margin,
            (screen.height - _height - _margin)
                .clamp(_margin, double.infinity)),
      );

  /// Telegram's release behaviour: the tile flies to whichever side edge it is
  /// nearer, keeping its height. Left free-floating it would sit over the
  /// middle of the thread, which is the one place it must not be.
  Offset _snapToEdge(Offset p, Size screen) {
    final double centre = p.dx + _width / 2;
    final double right =
        (screen.width - _width - _margin).clamp(_margin, double.infinity);
    return Offset(centre < screen.width / 2 ? _margin : right, p.dy);
  }

  @override
  Widget build(BuildContext context) {
    final CallSession session = widget.session;
    final CallUiState state = session.state;
    final CallEngine engine = ref.watch(callEngineProvider);
    final CallController controller = ref.read(callProvider.notifier);
    final Widget? remote =
        state.video ? engine.videoView(local: false, mirror: false) : null;
    final Size screen = MediaQuery.sizeOf(context);
    final Offset position =
        _clamp(_position ?? _defaultPosition(screen), screen);

    return AnimatedPositioned(
      // Instant while the finger is down; a short glide when it lets go.
      duration: _dragging ? Duration.zero : const Duration(milliseconds: 180),
      curve: Curves.easeOut,
      left: position.dx,
      top: position.dy,
      child: GestureDetector(
        key: const Key('call-floating-tile'),
        onTap: controller.restoreFromPip,
        onPanStart: (_) => setState(() {
          _dragging = true;
          _position = position;
        }),
        onPanUpdate: (DragUpdateDetails d) => setState(
          () => _position = _clamp((_position ?? position) + d.delta, screen),
        ),
        onPanEnd: (_) => setState(() {
          _dragging = false;
          _position = _snapToEdge(_position ?? position, screen);
        }),
        child: Material(
          elevation: 8,
          color: CallPalette.pipBackground,
          borderRadius: BorderRadius.circular(14),
          clipBehavior: Clip.antiAlias,
          child: SizedBox(
            width: _width,
            height: _height,
            child: Stack(
              fit: StackFit.expand,
              children: <Widget>[
                _VideoSurface(
                  view: remote,
                  peer: state.peer ?? '',
                  label: state.peerLabel,
                  paused: state.video && session.remoteVideoPaused,
                  large: false,
                ),
                Align(
                  alignment: Alignment.topLeft,
                  child: Padding(
                    padding: const EdgeInsets.all(6),
                    child: ConnectionStrengthIndicator(
                      quality: session.quality,
                      compact: true,
                    ),
                  ),
                ),
                Align(
                  alignment: Alignment.bottomCenter,
                  child: Padding(
                    padding: const EdgeInsets.only(bottom: 6),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: <Widget>[
                        CallActionButton(
                          key: const Key('call-tile-mute'),
                          icon: Icons.mic,
                          slashed: session.muted,
                          label: null,
                          size: 34,
                          background: session.muted
                              ? CallPalette.controlActive
                              : const Color(0x66000000),
                          tint: session.muted
                              ? CallPalette.background
                              : Colors.white,
                          onPressed: () => unawaited(controller.toggleMute()),
                        ),
                        const SizedBox(width: 8),
                        CallActionButton(
                          key: const Key('call-tile-hangup'),
                          icon: Icons.call_end,
                          label: null,
                          size: 34,
                          background: CallPalette.hangup,
                          onPressed: controller.hangup,
                        ),
                      ],
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// What a video surface shows when there is no picture: whose call it is, and
/// — when the other side deliberately stopped sending — that it is paused
/// rather than broken.
class _VideoPlaceholder extends StatelessWidget {
  const _VideoPlaceholder({
    required this.peer,
    required this.label,
    required this.paused,
    required this.large,
  });

  final String peer;
  final String label;
  final bool paused;
  final bool large;

  @override
  Widget build(BuildContext context) => ColoredBox(
        color: CallPalette.pipBackground,
        child: Center(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: <Widget>[
              if (large)
                PeerAvatar(title: label, seed: peer, size: 96)
              else
                Icon(
                  paused ? Icons.videocam_off : Icons.videocam,
                  size: 26,
                  color: const Color(0x99FFFFFF),
                ),
              if (paused) ...<Widget>[
                SizedBox(height: large ? 18 : 6),
                Text(
                  'Video paused',
                  key: const Key('video-paused'),
                  textAlign: TextAlign.center,
                  style: TextStyle(
                    color: Colors.white,
                    fontSize: large ? 16 : 11,
                  ),
                ),
              ] else if (large) ...<Widget>[
                const SizedBox(height: 18),
                Text(
                  label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(color: Colors.white, fontSize: 20),
                ),
              ],
            ],
          ),
        ),
      );
}

class _EndedContent extends ConsumerWidget {
  const _EndedContent({required this.state});

  final CallEnded state;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final String label = switch (state.outcome) {
      'missed' => 'No answer',
      'failed' => "Couldn't connect",
      'declined' => 'Call declined',
      'busy' => 'Busy',
      'cancelled' => 'Call cancelled',
      _ => 'Call ended',
    };
    return GestureDetector(
      onTap: ref.read(callProvider.notifier).dismissEnded,
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: <Widget>[
          PeerAvatar(title: state.label, seed: state.peerNpub, size: 96),
          const SizedBox(height: 20),
          Text(
            state.label,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(color: Colors.white, fontSize: 22),
          ),
          const SizedBox(height: 8),
          Text(
            label,
            style:
                const TextStyle(color: CallPalette.secondaryText, fontSize: 15),
          ),
        ],
      ),
    );
  }
}

class _PeerHeader extends StatelessWidget {
  const _PeerHeader({
    required this.peer,
    required this.label,
    this.status,
    this.extra = const <Widget>[],
    this.liveTimerFor,
  });

  final String peer;
  final String label;
  final String? status;
  final List<Widget> extra;
  final CallActive? liveTimerFor;

  @override
  Widget build(BuildContext context) => Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: <Widget>[
          PeerAvatar(title: label, seed: peer, size: 112),
          const SizedBox(height: 24),
          Text(
            label,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            textAlign: TextAlign.center,
            style: const TextStyle(color: Colors.white, fontSize: 26),
          ),
          if (liveTimerFor != null) ...<Widget>[
            const SizedBox(height: 10),
            _CallTimer(active: liveTimerFor!, fontSize: 16),
          ] else if (status != null) ...<Widget>[
            const SizedBox(height: 10),
            Text(
              status!,
              textAlign: TextAlign.center,
              style: const TextStyle(
                  color: CallPalette.secondaryText, fontSize: 16),
            ),
          ],
          for (final Widget widget in extra) ...<Widget>[
            const SizedBox(height: 10),
            widget,
          ],
        ],
      );
}

/// Live `m:ss` (or `h:mm:ss`) duration, ticking twice a second like the
/// Compose original.
class _CallTimer extends StatefulWidget {
  const _CallTimer({required this.active, this.fontSize = 13});

  final CallActive active;
  final double fontSize;

  static String labelFor(CallActive active) => formatCallClock(
        (DateTime.now().millisecondsSinceEpoch - active.connectedAtMs) ~/ 1000,
      );

  @override
  State<_CallTimer> createState() => _CallTimerState();
}

class _CallTimerState extends State<_CallTimer> {
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    _timer = Timer.periodic(
      const Duration(milliseconds: 500),
      (_) => setState(() {}),
    );
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Text(
        _CallTimer.labelFor(widget.active),
        style: TextStyle(
          color: CallPalette.secondaryText,
          fontSize: widget.fontSize,
          fontFamily: 'monospace',
        ),
      );
}

/// The panel the ⋮ opens: everything the bar deliberately does not hold.
///
/// A panel anchored to the bar rather than a modal sheet, because the call is
/// an overlay in a [Stack] and not a route — a modal route would push itself in
/// front of the picture, and dismissing it would be a second thing that can go
/// wrong while a call is running.
class _CallOptionsDock extends StatelessWidget {
  const _CallOptionsDock({
    required this.items,
    required this.session,
    required this.onScreenShare,
    required this.onSwitchCamera,
    required this.onChat,
  });

  final List<CallControl> items;
  final CallSession session;
  final VoidCallback onScreenShare;
  final VoidCallback onSwitchCamera;
  final VoidCallback onChat;

  // Glass tier chrome (§2, "the call control dock"). The tint/border/
  // highlight/shadow are overridden from `CallPalette` rather than the
  // ambient `ComradeSurfaces` ramp: a call stays dark regardless of the
  // app's light/dark setting (see `CallPalette`'s own doc), so this dock
  // must not pick up a light-theme fill the way an unparameterised
  // `GlassSurface` would.
  @override
  Widget build(BuildContext context) => GlassSurface(
        key: const Key('call-dock'),
        tier: GlassTier.chrome,
        borderRadius: BorderRadius.circular(16),
        tint: CallPalette.pipBackground,
        border: CallPalette.controlIdle,
        highlight: Colors.white,
        shadowColor: CallPalette.background,
        child: Material(
          // The fill is `GlassSurface`'s; this `Material` exists only so
          // `_DockItem`'s `InkWell` has one to splash against.
          type: MaterialType.transparency,
          child: ConstrainedBox(
            constraints: const BoxConstraints(minWidth: 216),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: <Widget>[
                for (final CallControl item in items) _item(item),
              ],
            ),
          ),
        ),
      );

  Widget _item(CallControl item) => switch (item) {
        // Screen share is offered on **voice calls too** — that is the point of
        // it, and it is why the stage can appear on a call that started without
        // video.
        CallControl.screenShare => _DockItem(
            key: const Key('call-screen-share'),
            icon: Icons.screen_share_outlined,
            label: session.screenSharing ? 'Stop sharing' : 'Share screen',
            active: session.screenSharing,
            onPressed: onScreenShare,
          ),
        CallControl.switchCamera => _DockItem(
            key: const Key('call-switch-camera'),
            icon: Icons.flip_camera_ios,
            label: 'Switch camera',
            onPressed: onSwitchCamera,
          ),
        CallControl.chat => _DockItem(
            key: const Key('call-chat'),
            icon: Icons.chat_bubble_outline,
            label: 'Chat',
            onPressed: onChat,
          ),
        // The bar's own controls never appear here.
        _ => const SizedBox.shrink(),
      };
}

class _DockItem extends StatefulWidget {
  const _DockItem({
    required this.icon,
    required this.label,
    required this.onPressed,
    this.active = false,
    super.key,
  });

  final IconData icon;
  final String label;
  final VoidCallback onPressed;

  /// Lit while the thing this row toggles is running.
  final bool active;

  @override
  State<_DockItem> createState() => _DockItemState();
}

class _DockItemState extends State<_DockItem> {
  // Owned here (not passed in) so `ComradeFocusRing` (§4) can share the
  // exact node `InkWell` focuses through — see that widget's own doc.
  final FocusNode _focusNode = FocusNode(debugLabel: 'call-dock-item');

  @override
  void dispose() {
    _focusNode.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final Color tint = widget.active ? CallPalette.controlActive : Colors.white;
    return ComradeFocusRing(
      focusNode: _focusNode,
      child: InkWell(
        focusNode: _focusNode,
        onTap: widget.onPressed,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
          child: Row(
            children: <Widget>[
              Icon(widget.icon, size: 22, color: tint),
              const SizedBox(width: 14),
              Expanded(
                child: Text(
                  widget.label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(color: tint, fontSize: 15),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// The audio-output control: a button showing the current route that opens a
/// menu of every currently-present route.
class AudioRouteButton extends ConsumerWidget {
  const AudioRouteButton({
    required this.session,
    this.onInteract,
    this.size = 60,
    super.key,
  });

  final CallSession session;

  /// Lets the call screen keep its self-hiding controls up while this menu is
  /// being used.
  final VoidCallback? onInteract;

  final double size;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final bool active = session.audioRoute != AudioRoute.earpiece;
    return PopupMenuButton<AudioRoute>(
      tooltip: 'Speaker: ${session.audioRoute.label}',
      onOpened: onInteract,
      onSelected: (AudioRoute route) {
        onInteract?.call();
        unawaited(ref.read(callProvider.notifier).setAudioRoute(route));
      },
      itemBuilder: (BuildContext context) => <PopupMenuEntry<AudioRoute>>[
        for (final AudioRoute route in session.availableRoutes)
          PopupMenuItem<AudioRoute>(value: route, child: Text(route.label)),
      ],
      child: CallActionButton(
        key: const Key('call-speaker'),
        icon: Icons.volume_up,
        label: session.audioRoute.label,
        size: size,
        background:
            active ? CallPalette.controlActive : CallPalette.controlIdle,
        tint: active ? CallPalette.background : Colors.white,
        onPressed: null,
      ),
    );
  }
}

/// A round call control with an optional label beneath — the uniform shape
/// every control on this screen uses.
class CallActionButton extends StatelessWidget {
  const CallActionButton({
    required this.icon,
    required this.label,
    required this.background,
    required this.onPressed,
    this.tint = Colors.white,
    this.size = 60,
    this.slashed = false,
    super.key,
  });

  final IconData icon;
  final String? label;
  final Color background;
  final Color tint;
  final double size;

  /// Draws (and animates) a slash across [icon] — the explicit "this is off"
  /// every other video app uses. See `widgets/slashed_icon.dart`.
  final bool slashed;

  /// `null` when the surrounding widget owns the gesture (see
  /// [AudioRouteButton], whose popup wraps this).
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) => Column(
        mainAxisSize: MainAxisSize.min,
        children: <Widget>[
          Semantics(
            button: true,
            label: label,
            child: InkWell(
              onTap: onPressed,
              customBorder: const CircleBorder(),
              child: Container(
                width: size,
                height: size,
                decoration:
                    BoxDecoration(shape: BoxShape.circle, color: background),
                alignment: Alignment.center,
                child: SlashedIcon(
                  icon: icon,
                  slashed: slashed,
                  color: tint,
                  cutoutColor: background,
                  size: size * 0.44,
                ),
              ),
            ),
          ),
          if (label != null) ...<Widget>[
            const SizedBox(height: 6),
            // Ellipsised, not clipped: the bar gives each control an equal
            // slice of the screen, and "Wired headset" is wider than a fifth
            // of a phone.
            Text(
              label!,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              textAlign: TextAlign.center,
              style: const TextStyle(color: Color(0xB3FFFFFF), fontSize: 12),
            ),
          ],
        ],
      );
}
