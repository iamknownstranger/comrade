/// The private journal — wellbeing pillar #1. Everything written here stays on
/// this device, sealed inside the encrypted store; nothing is ever published
/// to a relay.
///
/// Port of `ui/JournalScreen.kt`. Desktop has no journal at all (three
/// `journal_*` Tauri commands registered, no caller).
///
/// Dictation is **not** ported: `OneShotRecognizer`/Vosk is an Android
/// service, the model download is an Android foreground service, and there is
/// no cross-platform on-device recogniser here. A mic button that cannot
/// listen is worse than no mic button — the same "no fake switches" rule the
/// Android settings screen already states about the mesh.
///
/// **Recording entries — voice and video alike — are shown here but not
/// recorded here**, for the same reason and by the same rule
/// (`docs/JOURNAL.md`): capture, the dedicated folders that keep a recording out
/// of the gallery, and the orphan sweep are all Android's, and a recording never
/// leaves the device that made it — so there is nothing for this frontend to
/// play even when it can see the entry. What it draws is the title and the
/// clip's length, so such an entry is not a blank card.
library;

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../data/comrade_repository.dart';
import '../data/models.dart';
import '../state/content_providers.dart';
import '../state/providers.dart';
import '../util/display_name.dart';
import '../util/journal_note.dart';
import '../util/journal_recording.dart';
import '../widgets/app_chrome.dart';
import '../widgets/glass_surface.dart';
import '../widgets/list_skeleton.dart';

/// Self-reported mood markers, low → high. Stored as the emoji itself.
const List<String> kMoods = <String>['😞', '😕', '😐', '🙂', '😄'];

class JournalScreen extends ConsumerStatefulWidget {
  const JournalScreen({super.key});

  @override
  ConsumerState<JournalScreen> createState() => _JournalScreenState();
}

class _JournalScreenState extends ConsumerState<JournalScreen> {
  final TextEditingController _draft = TextEditingController();
  String? _mood;
  bool _saving = false;
  String? _error;

  @override
  void dispose() {
    _draft.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    final String text = _draft.text.trim();
    if (text.isEmpty || _saving) return;
    setState(() {
      _saving = true;
      _error = null;
    });
    try {
      await ref.read(journalProvider.notifier).add(text: text, mood: _mood);
      if (!mounted) return;
      _draft.clear();
      setState(() {
        _mood = null;
        _saving = false;
      });
    } on ComradeException catch (e) {
      if (mounted) {
        setState(() {
          _saving = false;
          _error = e.message;
        });
      }
    }
  }

  Future<void> _confirmDelete(JournalEntryInfo entry) async {
    final bool? yes = await showGlassDialog<bool>(
      context,
      builder: (BuildContext context) => AlertDialog(
        title: const Text('Delete this entry?'),
        content: const Text(
          'It will be removed from this device. There is no other copy.',
        ),
        actions: <Widget>[
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('Delete'),
          ),
        ],
      ),
    );
    if (yes ?? false) await ref.read(journalProvider.notifier).delete(entry.id);
  }

  /// Hand one entry to one person.
  ///
  /// A dialog of Comrade's own contacts rather than the platform share sheet,
  /// and that is the point: the system sheet would offer every app on the
  /// device a plaintext copy of the most private thing this app holds. The note
  /// travels as an encrypted DM and nothing else ever sees it.
  Future<void> _share(JournalEntryInfo entry) async {
    final List<ContactInfo> contacts =
        await ref.read(comradeRepositoryProvider).contacts();
    if (!mounted) return;
    final List<ShareTarget> targets = shareTargets(
      contacts
          .map((ContactInfo c) => (
                npub: c.npub,
                alias: c.alias,
                name: c.name,
                comrade: c.comrade,
              ))
          .toList(),
    );
    final ShareTarget? pick = await showGlassDialog<ShareTarget>(
      context,
      builder: (BuildContext context) => AlertDialog(
        title: const Text('Send this note to…'),
        content: SizedBox(
          width: double.maxFinite,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: <Widget>[
              Text(
                'A copy goes into your chat with them, encrypted like any '
                'other message. The entry stays in your journal, and nothing '
                'else from it is sent.',
                style: Theme.of(context).textTheme.bodySmall?.copyWith(
                      color: Theme.of(context).colorScheme.onSurfaceVariant,
                    ),
              ),
              const SizedBox(height: 10),
              if (targets.isEmpty)
                const Text(
                  "You haven't saved anyone yet. Add a contact from Chats "
                  'first — a note is only ever sent to someone you chose.',
                )
              else
                Flexible(
                  child: ListView(
                    key: const Key('journal-share-targets'),
                    shrinkWrap: true,
                    children: <Widget>[
                      for (final ShareTarget t in targets)
                        ListTile(
                          title: Text(t.label),
                          onTap: () => Navigator.of(context).pop(t),
                        ),
                    ],
                  ),
                ),
            ],
          ),
        ),
        actions: <Widget>[
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('Cancel'),
          ),
        ],
      ),
    );
    if (pick == null || !mounted) return;
    try {
      await ref
          .read(comradeRepositoryProvider)
          .shareJournalEntry(peer: pick.npub, entryId: entry.id);
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Sent to ${pick.label}.')),
      );
    } on ComradeException catch (e) {
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Could not send: ${e.message}')),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final AsyncValue<List<JournalEntryInfo>> entries =
        ref.watch(journalProvider);
    final int nowSecs = DateTime.now().millisecondsSinceEpoch ~/ 1000;

    return ReadingColumn(
      child: ListView(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
        children: <Widget>[
          _composer(context),
          const SizedBox(height: 10),
          ...entries.when(
            // §7.3: the row shape is known before the list loads.
            loading: () => <Widget>[
              ListSkeleton.cardRows(key: const Key('journal-skeleton')),
            ],
            error: (Object e, StackTrace s) => <Widget>[
              EmptyState(
                title: 'Could not load the journal',
                body: '$e',
                action: FilledButton(
                  onPressed: () => ref.invalidate(journalProvider),
                  child: const Text('Try again'),
                ),
              ),
            ],
            data: (List<JournalEntryInfo> list) {
              if (list.isEmpty) {
                return <Widget>[
                  Padding(
                    padding: const EdgeInsets.only(top: 24),
                    child: Text(
                      'Nothing yet. A line a day is plenty — write whatever is '
                      'on your mind.',
                      textAlign: TextAlign.center,
                      style: Theme.of(context).textTheme.bodyMedium?.copyWith(
                            color:
                                Theme.of(context).colorScheme.onSurfaceVariant,
                          ),
                    ),
                  ),
                ];
              }
              return <Widget>[
                for (final JournalDay day
                    in groupJournalByDay(list, nowSecs)) ...<Widget>[
                  Padding(
                    padding: const EdgeInsets.only(top: 6, bottom: 6),
                    child: Text(
                      day.label,
                      style: Theme.of(context).textTheme.titleSmall?.copyWith(
                            color: Theme.of(context).colorScheme.primary,
                          ),
                    ),
                  ),
                  for (final JournalEntryInfo entry in day.entries)
                    Padding(
                      padding: const EdgeInsets.only(bottom: 10),
                      child: _JournalEntryCard(
                        entry: entry,
                        onShare: () => _share(entry),
                        onDelete: () => _confirmDelete(entry),
                      ),
                    ),
                ],
              ];
            },
          ),
        ],
      ),
    );
  }

  Widget _composer(BuildContext context) => SectionCard(
        elevated: true,
        padding: const EdgeInsets.all(14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: <Widget>[
            TextField(
              key: const Key('journal-input'),
              controller: _draft,
              minLines: 3,
              maxLines: 8,
              decoration:
                  const InputDecoration(hintText: "What's on your mind?"),
            ),
            const SizedBox(height: 10),
            Wrap(
              spacing: 6,
              children: <Widget>[
                for (final String m in kMoods)
                  FilterChip(
                    label: Text(m),
                    selected: _mood == m,
                    onSelected: (_) =>
                        setState(() => _mood = _mood == m ? null : m),
                  ),
              ],
            ),
            const SizedBox(height: 10),
            Row(
              children: <Widget>[
                const Spacer(),
                BusyButton(
                  key: const Key('journal-save'),
                  label: 'Save',
                  busyLabel: 'Saving…',
                  busy: _saving,
                  onPressed: _save,
                ),
              ],
            ),
            const SizedBox(height: 8),
            Text(
              'Only on this device, sealed by your passcode. Never posted, '
              'never uploaded — a note reaches someone only if you send it to '
              'them yourself.',
              style: Theme.of(context).textTheme.bodySmall?.copyWith(
                    color: Theme.of(context).colorScheme.onSurfaceVariant,
                  ),
            ),
            ErrorText(_error),
          ],
        ),
      );
}

/// The one line a recording entry draws in place of a player.
///
/// Says what the entry is and how long the recording runs, and says plainly
/// that it is on the phone that made it. Offering a play control that cannot
/// play would be a fake switch; saying nothing at all would make a wordless
/// voice or video entry look like an empty card.
class _RecordingLine extends StatelessWidget {
  const _RecordingLine({required this.recording});

  final JournalRecordingInfo recording;

  @override
  Widget build(BuildContext context) {
    final bool spoken = recording.mime.startsWith('audio/');
    final String kind = spoken ? 'Voice entry' : 'Video entry';
    final String length = formatClipLength(recording.durationMs);
    final String size = formatClipSize(recording.sizeBytes);
    final String detail = <String>[length, size]
        .where((String part) => part.isNotEmpty)
        .join(' · ');
    return Padding(
      key: const Key('journal-recording-line'),
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Row(
        children: <Widget>[
          Icon(
            spoken ? Icons.graphic_eq : Icons.videocam_outlined,
            size: 18,
            color: Theme.of(context).colorScheme.outline,
          ),
          const SizedBox(width: 6),
          Expanded(
            child: Text(
              detail.isEmpty
                  ? '$kind — on the phone that recorded it'
                  : '$kind · $detail — on the phone that recorded it',
              style: Theme.of(context).textTheme.labelSmall?.copyWith(
                    color: Theme.of(context).colorScheme.onSurfaceVariant,
                  ),
            ),
          ),
        ],
      ),
    );
  }
}

class _JournalEntryCard extends StatelessWidget {
  const _JournalEntryCard({
    required this.entry,
    required this.onShare,
    required this.onDelete,
  });

  final JournalEntryInfo entry;
  final VoidCallback onShare;
  final VoidCallback onDelete;

  @override
  Widget build(BuildContext context) => SectionCard(
        padding: const EdgeInsets.fromLTRB(14, 10, 4, 10),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: <Widget>[
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: <Widget>[
                  Row(
                    children: <Widget>[
                      if (entry.mood != null) ...<Widget>[
                        Text(
                          entry.mood!,
                          style: Theme.of(context).textTheme.titleMedium,
                        ),
                        const SizedBox(width: 6),
                      ],
                      Text(
                        relativeTime(entry.createdAt),
                        style: Theme.of(context).textTheme.labelSmall?.copyWith(
                              color: Theme.of(context).colorScheme.outline,
                            ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 4),
                  if (entry.title != null)
                    Text(
                      entry.title!,
                      key: const Key('journal-entry-title'),
                      style: Theme.of(context)
                          .textTheme
                          .titleSmall
                          ?.copyWith(fontWeight: FontWeight.w600),
                    ),
                  if (entry.recording != null)
                    _RecordingLine(recording: entry.recording!),
                  // A video entry often has no words at all, and an empty Text
                  // still takes a line's worth of space.
                  if (entry.text.isNotEmpty)
                    Text(entry.text,
                        style: Theme.of(context).textTheme.bodyLarge),
                ],
              ),
            ),
            // Sharing sends the words, never a recording — the core refuses an
            // entry with none, so a control that cannot work is not offered.
            if (entry.text.trim().isNotEmpty)
              IconButton(
                key: const Key('journal-share'),
                onPressed: onShare,
                tooltip: 'Share this note',
                icon: Icon(
                  Icons.share_outlined,
                  color: Theme.of(context).colorScheme.outline,
                ),
              ),
            IconButton(
              onPressed: onDelete,
              tooltip: 'Delete entry',
              icon: Icon(
                Icons.delete_outline,
                color: Theme.of(context).colorScheme.outline,
              ),
            ),
          ],
        ),
      );
}
