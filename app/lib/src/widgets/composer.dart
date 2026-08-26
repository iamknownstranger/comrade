/// The message composer — Telegram's layout, with this app's honesty rules.
///
/// ```text
///  ╭───────────────────────────────────────────────╮      ╭────╮
///  │ 🙂   Message…                             📎  │  ⧉   │ 🎤 │
///  ╰───────────────────────────────────────────────╯      ╰────╯
///     emoji        text field           attach     swap    mic → send
/// ```
///
/// * **Emoji on the left, inside the field.** Opens a dependency-free picker
///   (`emoji_picker.dart`) that inserts at the caret, not at the end.
/// * **Paper clip on the right, inside the field.** The document picker.
/// * **One round button outside, on the right.** It is *Send* the moment there
///   is text to send — which is what makes the layout work on a phone and a
///   desktop equally — and otherwise the current capture control.
/// * **A small swap control beside it**, present only when this device offers
///   more than one way to capture, showing the glyph of the mode it would move
///   *to*. Telegram hides that switch behind a press-and-hold on the mic; that
///   is undiscoverable with a mouse *and* with a finger, and worse, it shares a
///   target with a tap that starts recording — so it is a visible button here.
///
/// Everything is capability-gated from one startup probe. A platform with no
/// recorder and no camera gets a plain Send button rather than controls that
/// fail on tap; the same rule the settings screen states as "no fake switches".
/// That is also why the mic exists at all now: divergence D13 dropped voice
/// notes because press-and-hold has no meaning with a mouse — the *gesture* was
/// the problem, not the feature, so recording is tap-to-start / tap-to-send.
library;

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/semantics.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../util/composer_mode.dart';
import 'emoji_picker.dart';
import 'glass_surface.dart';
import 'media_attachment.dart';

export '../util/composer_mode.dart' show ComposerCaptureMode;

class MessageComposer extends ConsumerStatefulWidget {
  const MessageComposer({
    required this.controller,
    required this.focusNode,
    required this.onSend,
    required this.onAttachment,
    this.sending = false,
    this.attaching = false,
    this.hintText = 'Message',
    super.key,
  });

  final TextEditingController controller;
  final FocusNode focusNode;

  /// Send the text currently in [controller].
  final Future<void> Function() onSend;

  /// Send one attachment — picked, captured, or recorded.
  ///
  /// `preview` asks the caller to show the confirm-before-sending sheet. It is
  /// false for a voice note and true for everything else: the recording strip is
  /// *already* that sheet — it holds the clip with a discard button and a send
  /// button — and a second confirmation after tapping send would be one too many.
  final Future<void> Function(PickedAttachment attachment,
      {required bool preview}) onAttachment;

  /// A text send is in flight.
  final bool sending;

  /// An attachment send is in flight (upload + encrypt).
  final bool attaching;

  final String hintText;

  @override
  ConsumerState<MessageComposer> createState() => _MessageComposerState();
}

class _MessageComposerState extends ConsumerState<MessageComposer> {
  ComposerCaptureMode _mode = ComposerCaptureMode.voice;

  bool _recording = false;
  int _recordedSeconds = 0;
  Timer? _tick;

  /// True while a picker/camera/recorder round trip is open, so a second tap
  /// cannot start a second one.
  bool _busy = false;

  // Explicit `FocusNode`s so `ComradeFocusRing` (§4) can share the same node
  // the button itself focuses through, rather than wrap it in a second,
  // independent `Focus` — see that widget's own doc for why that matters.
  final FocusNode _attachFocus = FocusNode(debugLabel: 'dm-attach');
  final FocusNode _actionFocus = FocusNode(debugLabel: 'dm-action');

  @override
  void initState() {
    super.initState();
    // The round button swaps between Send and capture as the draft changes.
    widget.controller.addListener(_onDraftChanged);
  }

  @override
  void dispose() {
    widget.controller.removeListener(_onDraftChanged);
    _tick?.cancel();
    _attachFocus.dispose();
    _actionFocus.dispose();
    super.dispose();
  }

  void _onDraftChanged() {
    if (mounted) setState(() {});
  }

  bool get _hasText => widget.controller.text.trim().isNotEmpty;

  void _snack(String message) {
    if (!mounted) return;
    ScaffoldMessenger.maybeOf(context)
        ?.showSnackBar(SnackBar(content: Text(message)));
  }

  // ── Emoji ─────────────────────────────────────────────────────────────────

  Future<void> _openEmoji() async {
    // Keep the caret where it was: opening a sheet unfocuses the field, and
    // inserting into an unfocused field with a stale selection is how emoji end
    // up in the middle of a word.
    final TextSelection selection = widget.controller.selection;
    if (!selection.isValid) {
      widget.controller.selection = TextSelection.collapsed(
        offset: widget.controller.text.length,
      );
    }
    await showEmojiPicker(
      context,
      onPick: (String emoji) => insertEmoji(widget.controller, emoji),
    );
    if (mounted) widget.focusNode.requestFocus();
  }

  // ── Attach / capture ──────────────────────────────────────────────────────

  /// Run one picker/camera round trip, with the spinner up and a second tap
  /// locked out, then hand what it produced to the caller.
  ///
  /// `_busy` is dropped *before* [MessageComposer.onAttachment] deliberately: the
  /// caller now shows a confirm-before-sending sheet, and a paper clip spinning
  /// throughout would claim an upload is in flight while the sender is still
  /// writing the caption. The sheet is modal, so nothing needs guarding while it
  /// is up, and the real upload has its own flag in
  /// [MessageComposer.attaching].
  Future<void> _guarded(Future<PickedAttachment?> Function() source) async {
    if (_busy || widget.attaching) return;
    setState(() => _busy = true);
    PickedAttachment? picked;
    try {
      picked = await source();
    } finally {
      if (mounted) setState(() => _busy = false);
    }
    if (picked != null) await widget.onAttachment(picked, preview: true);
  }

  Future<void> _attach() => _guarded(() async {
        final AttachmentPicker picker = ref.read(attachmentPickerProvider);
        final PickedAttachment? picked = await picker.pick();
        if (picked == null && mounted) {
          // Cancelled and unsupported look identical from here; the picker's own
          // platform error would have thrown. Say the neutral thing.
          _maybeSayNoPicker();
        }
        return picked;
      });

  void _maybeSayNoPicker() {
    if (ref.read(attachmentPickerProvider) is NoAttachmentPicker) {
      _snack('No file picker is wired up on this platform yet.');
    }
  }

  Future<void> _capturePhoto() =>
      _guarded(() => ref.read(mediaCaptureProvider).capturePhoto());

  Future<void> _captureVideo() =>
      _guarded(() => ref.read(mediaCaptureProvider).captureVideo());

  // ── Voice notes ───────────────────────────────────────────────────────────

  Future<void> _startRecording() async {
    if (_busy || _recording) return;
    final VoiceNoteRecorder recorder = ref.read(voiceNoteRecorderProvider);
    final bool started = await recorder.start();
    if (!started) {
      _snack('Could not start recording — the microphone is unavailable.');
      return;
    }
    if (!mounted) return;
    setState(() {
      _recording = true;
      _recordedSeconds = 0;
    });
    _tick = Timer.periodic(const Duration(seconds: 1), (_) {
      if (mounted) setState(() => _recordedSeconds++);
    });
  }

  Future<void> _finishRecording({required bool send}) async {
    _tick?.cancel();
    _tick = null;
    final VoiceNoteRecorder recorder = ref.read(voiceNoteRecorderProvider);
    if (mounted) setState(() => _recording = false);
    if (!send) {
      await recorder.cancel();
      return;
    }
    final PickedAttachment? note = await recorder.stop();
    if (note == null) {
      // Too short to be a note — an accidental tap, not an error.
      _snack('Too short — hold on a moment longer.');
      return;
    }
    await widget.onAttachment(note, preview: false);
  }

  // ── The round button and its mode ─────────────────────────────────────────

  /// The modes this device can actually offer, in cycle order.
  ///
  /// `read`, not `watch`, because this is also called from the swap handler
  /// (where `watch` is illegal) and because the answer cannot change while the
  /// app runs — it comes from one probe at startup.
  List<ComposerCaptureMode> _availableModes() {
    final MediaCaptureDelegate capture = ref.read(mediaCaptureProvider);
    return availableCaptureModes(
      canRecordAudio: ref.read(voiceNoteRecorderProvider).available,
      canTakePhoto: capture.canCapturePhoto,
      canRecordVideo: capture.canCaptureVideo,
    );
  }

  /// The current mode, corrected to something this device has. A stale `_mode`
  /// (a camera that vanished, or a default that never applied) must never leave
  /// the button doing nothing.
  ComposerCaptureMode? _effectiveMode(List<ComposerCaptureMode> available) =>
      effectiveCaptureMode(_mode, available);

  /// Move to the next available mode.
  ///
  /// A visible control rather than a long-press: holding is undiscoverable with
  /// a mouse *and* with a finger, and the one thing worse than a hidden gesture
  /// is a hidden gesture that shares a target with a tap that records.
  void _cycleMode() {
    final ComposerCaptureMode? next = nextCaptureMode(_mode, _availableModes());
    if (next == null) return;
    setState(() => _mode = next);
  }

  static IconData _iconFor(ComposerCaptureMode mode) => switch (mode) {
        ComposerCaptureMode.voice => Icons.mic_none,
        ComposerCaptureMode.photo => Icons.photo_camera_outlined,
        ComposerCaptureMode.video => Icons.videocam_outlined,
      };

  static String _labelFor(ComposerCaptureMode mode) => switch (mode) {
        ComposerCaptureMode.voice => 'Record a voice note',
        ComposerCaptureMode.photo => 'Take a photo',
        ComposerCaptureMode.video => 'Record a video',
      };

  @override
  Widget build(BuildContext context) {
    if (_recording) return _recordingStrip(context);

    final ColorScheme colors = Theme.of(context).colorScheme;
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 8, 12, 8),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.end,
        children: <Widget>[
          Expanded(
            child: TextField(
              key: const Key('dm-input'),
              controller: widget.controller,
              focusNode: widget.focusNode,
              minLines: 1,
              maxLines: 5,
              textInputAction: TextInputAction.send,
              onSubmitted: (_) => widget.onSend(),
              decoration: InputDecoration(
                hintText: widget.hintText,
                isDense: true,
                filled: true,
                fillColor: colors.surfaceContainerHighest,
                contentPadding:
                    const EdgeInsets.symmetric(horizontal: 4, vertical: 10),
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(26),
                  borderSide: BorderSide.none,
                ),
                // Emoji on the left, paper clip on the right — both *inside*
                // the field, so the composer reads as one pill.
                prefixIcon: IconButton(
                  key: const Key('dm-emoji'),
                  tooltip: 'Emoji',
                  onPressed: _openEmoji,
                  icon: const Icon(Icons.emoji_emotions_outlined),
                ),
                suffixIcon: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: <Widget>[
                    ComradeFocusRing(
                      focusNode: _attachFocus,
                      child: IconButton(
                        key: const Key('dm-attach'),
                        focusNode: _attachFocus,
                        tooltip: 'Attach a file (max 10 MB, encrypted)',
                        onPressed: (widget.attaching || _busy) ? null : _attach,
                        icon: (widget.attaching || _busy)
                            ? const SizedBox(
                                width: 18,
                                height: 18,
                                child:
                                    CircularProgressIndicator(strokeWidth: 2),
                              )
                            : const Icon(Icons.attach_file),
                      ),
                    ),
                    _actionControl(context),
                  ],
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }

  /// The one action control, inside the field.
  ///
  /// Send while there is text; otherwise the current capture mode, which a
  /// right-swipe on this same icon cycles. Mirrors `ComposerActionIcon` in
  /// `ChatsScreen.kt` — one control, same gesture, same order.
  ///
  /// Swiping is neither discoverable on its own nor available to a screen
  /// reader, so the cycle is also a semantics action and the tooltip names it.
  Widget _actionControl(BuildContext context) {
    final List<ComposerCaptureMode> available = _availableModes();
    final ComposerCaptureMode? mode = _effectiveMode(available);
    final ComposerCaptureMode? swapTo = nextCaptureMode(mode, available);
    final bool canSwipe = !_hasText && swapTo != null && !_recording;

    final IconData icon = _hasText || mode == null
        ? Icons.send
        : (_recording ? Icons.stop : _iconFor(mode));
    final String label = _hasText || mode == null
        ? 'Send'
        : (_recording ? 'Stop and send' : _labelFor(mode));
    final bool enabled = _hasText ? !widget.sending : (mode != null && !_busy);

    final Widget button = ComradeFocusRing(
      focusNode: _actionFocus,
      borderRadius: const BorderRadius.all(Radius.circular(24)),
      child: IconButton(
        key: const Key('dm-action'),
        focusNode: _actionFocus,
        tooltip: canSwipe ? '$label — swipe right to switch mode' : label,
        onPressed: enabled
            ? () {
                if (_hasText) {
                  widget.onSend();
                  return;
                }
                switch (mode!) {
                  case ComposerCaptureMode.voice:
                    _recording
                        ? _finishRecording(send: true)
                        : _startRecording();
                  case ComposerCaptureMode.photo:
                    _capturePhoto();
                  case ComposerCaptureMode.video:
                    _captureVideo();
                }
              }
            : null,
        icon: (_busy && !_recording)
            ? const SizedBox(
                width: 18,
                height: 18,
                child: CircularProgressIndicator(strokeWidth: 2),
              )
            : Icon(icon),
      ),
    );

    if (!canSwipe) return button;
    return Semantics(
      customSemanticsActions: <CustomSemanticsAction, VoidCallback>{
        CustomSemanticsAction(
            label: 'Switch to ${_labelFor(swapTo).toLowerCase()}'): _cycleMode,
      },
      child: GestureDetector(
        // Rightwards only, and far enough to be deliberate rather than a slip
        // while reaching for the icon.
        onHorizontalDragEnd: (DragEndDetails d) {
          if (d.primaryVelocity != null && d.primaryVelocity! > 0) _cycleMode();
        },
        child: button,
      ),
    );
  }

  Widget _recordingStrip(BuildContext context) {
    final ColorScheme colors = Theme.of(context).colorScheme;
    final String elapsed =
        '${(_recordedSeconds ~/ 60)}:${(_recordedSeconds % 60).toString().padLeft(2, '0')}';
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 8, 12, 8),
      child: Row(
        children: <Widget>[
          IconButton(
            key: const Key('dm-record-cancel'),
            tooltip: 'Discard',
            onPressed: () => _finishRecording(send: false),
            icon: Icon(Icons.delete_outline, color: colors.error),
          ),
          Icon(Icons.fiber_manual_record, size: 14, color: colors.error),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              'Recording  $elapsed',
              key: const Key('dm-record-elapsed'),
              style: Theme.of(context).textTheme.bodyMedium,
            ),
          ),
          IconButton.filled(
            key: const Key('dm-record-send'),
            tooltip: 'Send voice note',
            onPressed: () => _finishRecording(send: true),
            icon: const Icon(Icons.send),
          ),
        ],
      ),
    );
  }
}
