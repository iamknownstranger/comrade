/// Call history: every logged voice/video call, incoming or outgoing, newest
/// first. Missed/declined/busy/failed rows get the same error tint a phone
/// dialer would use, so a missed call stands out at a glance; tapping any row
/// calls that peer back.
///
/// Port of `ui/CallHistoryScreen.kt`. Desktop has no equivalent screen at all
/// (`call_history` is a registered Tauri command with no caller) — this is one
/// of the four Android-only surfaces the unified app has to carry.
library;

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../data/models.dart';
import '../../state/content_providers.dart';
import '../../state/providers.dart';
import '../../util/display_name.dart';
import '../../widgets/app_chrome.dart';
import '../../widgets/list_skeleton.dart';
import '../../widgets/peer_avatar.dart';

/// Outcomes a row renders with the "problem" (error) tint.
const Set<String> kProblemOutcomes = <String>{
  'missed',
  'declined',
  'busy',
  'failed'
};

String callOutcomeLabel(CallRecordInfo record) => switch (record.outcome) {
      'connected' => formatCallDuration(record.durationSecs),
      'missed' => 'Missed',
      'declined' => 'Declined',
      'busy' => 'Busy',
      'cancelled' => 'Cancelled',
      'failed' => 'Failed',
      _ => record.outcome,
    };

class CallHistoryScreen extends ConsumerWidget {
  const CallHistoryScreen({required this.onCallBack, super.key});

  final void Function(String peer, String peerLabel, bool video) onCallBack;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final AsyncValue<List<CallRecordInfo>> history =
        ref.watch(callHistoryProvider);
    final Map<String, ContactInfo> contacts = ref.watch(contactsByNpubProvider);

    return history.when(
      // §7.3: a call row is an avatar, a name and a time — a shape known
      // before the list arrives, so the loading state shows it rather than
      // hiding it behind a spinner.
      loading: () => ListSkeleton.peerRows(
        key: const Key('call-history-skeleton'),
      ),
      error: (Object e, StackTrace s) => EmptyState(
        title: 'Could not load call history',
        body: '$e',
        action: FilledButton(
          onPressed: () => ref.invalidate(callHistoryProvider),
          child: const Text('Try again'),
        ),
      ),
      data: (List<CallRecordInfo> list) {
        if (list.isEmpty) return const EmptyState(title: 'No calls yet');
        return ReadingColumn(
          child: ListView.separated(
            itemCount: list.length,
            separatorBuilder: (_, __) => const Divider(indent: 76, height: 1),
            itemBuilder: (BuildContext context, int index) {
              final CallRecordInfo record = list[index];
              final ContactInfo? contact = contacts[record.peer];
              final String title =
                  peerTitle(record.peer, contact?.alias, contact?.name);
              return _CallHistoryRow(
                record: record,
                title: title,
                onTap: () => onCallBack(record.peer, title, record.isVideo),
              );
            },
          ),
        );
      },
    );
  }
}

class _CallHistoryRow extends StatelessWidget {
  const _CallHistoryRow({
    required this.record,
    required this.title,
    required this.onTap,
  });

  final CallRecordInfo record;
  final String title;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final ColorScheme colors = Theme.of(context).colorScheme;
    final bool problem = kProblemOutcomes.contains(record.outcome);
    final Color tint = problem ? colors.error : colors.onSurfaceVariant;
    final String direction = record.incoming ? 'Incoming' : 'Outgoing';
    return InkWell(
      onTap: onTap,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 11),
        child: Row(
          children: <Widget>[
            PeerAvatar(title: title, seed: record.peer),
            const SizedBox(width: 14),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: <Widget>[
                  Text(
                    title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: Theme.of(context).textTheme.titleMedium,
                  ),
                  Text(
                    '$direction · ${callOutcomeLabel(record)} · '
                    '${relativeTime(record.startedAt)}',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: Theme.of(context)
                        .textTheme
                        .bodyMedium
                        ?.copyWith(color: colors.onSurfaceVariant),
                  ),
                ],
              ),
            ),
            Icon(
              record.isVideo ? Icons.videocam : Icons.call,
              color: tint,
              semanticLabel: record.isVideo ? 'Video call' : 'Voice call',
            ),
          ],
        ),
      ),
    );
  }
}
