/// The chat list, with the message-requests bucket banner on top.
///
/// Port of `ChatsScreen.kt`'s `ChatsScreen` + `RequestsBanner` + `EmptyChats`.
library;

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../data/models.dart';
import '../../state/chat_providers.dart';
import '../../util/display_name.dart';
import '../../widgets/app_chrome.dart';
import '../../widgets/list_skeleton.dart';
import '../../widgets/peer_avatar.dart';

class ChatsListScreen extends ConsumerWidget {
  const ChatsListScreen({
    required this.onOpen,
    required this.onNewChat,
    required this.onOpenRequests,
    this.selectedPeer,
    super.key,
  });

  final void Function(ChatTarget target) onOpen;
  final VoidCallback onNewChat;
  final VoidCallback onOpenRequests;

  /// Highlighted row in the two-pane desktop layout; `null` on a phone, where
  /// the list is never visible beside the thread.
  final String? selectedPeer;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final AsyncValue<List<ConversationInfo>> conversations =
        ref.watch(conversationsProvider);
    final int requestCount = ref.watch(messageRequestCountProvider);

    return conversations.when(
      // §7.3: the row shape is known before the list loads, so the loading
      // state shows it rather than hiding it behind a spinner.
      loading: () => ListSkeleton.peerRows(key: const Key('chats-skeleton')),
      error: (Object e, StackTrace s) => EmptyState(
        title: 'Could not load your chats',
        body: '$e',
        action: FilledButton(
          onPressed: () => ref.read(conversationsProvider.notifier).refresh(),
          child: const Text('Try again'),
        ),
      ),
      data: (List<ConversationInfo> list) {
        if (list.isEmpty) {
          return Column(
            children: <Widget>[
              _RequestsBanner(count: requestCount, onTap: onOpenRequests),
              Expanded(
                child: EmptyState(
                  title: 'No chats yet',
                  body: 'Find someone by username, or share your key so they '
                      'can find you.',
                  action: FilledButton(
                    onPressed: onNewChat,
                    child: const Text('Start a chat'),
                  ),
                ),
              ),
            ],
          );
        }
        return ListView.separated(
          itemCount: list.length + 1,
          separatorBuilder: (BuildContext c, int i) => i == 0
              ? const SizedBox.shrink()
              : const Divider(indent: 76, height: 1),
          itemBuilder: (BuildContext context, int index) {
            if (index == 0) {
              return _RequestsBanner(
                  count: requestCount, onTap: onOpenRequests);
            }
            final ConversationInfo convo = list[index - 1];
            final String title =
                peerTitle(convo.peer, convo.alias, convo.peerName);
            return _ConversationRow(
              convo: convo,
              title: title,
              selected: convo.peer == selectedPeer,
              onTap: () => onOpen(
                ChatTarget(
                  peer: convo.peer,
                  alias: convo.alias,
                  username: convo.peerName,
                ),
              ),
            );
          },
        );
      },
    );
  }
}

/// Tappable banner atop the chat list linking to the message-requests inbox.
///
/// Android put this at the top of the list; desktop put a collapsible
/// `#requests-section` above the conversation list with an inline count badge
/// and Accept/Block buttons on every row. The banner wins for the unified app:
/// inline accept/block in a sidebar list means one mis-tap silently accepts a
/// stranger, and the dedicated inbox is where the decision belongs.
class _RequestsBanner extends StatelessWidget {
  const _RequestsBanner({required this.count, required this.onTap});

  final int count;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    if (count <= 0) return const SizedBox.shrink();
    final ColorScheme colors = Theme.of(context).colorScheme;
    return Material(
      color: colors.secondaryContainer,
      child: InkWell(
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
          child: Row(
            children: <Widget>[
              Text('✉', style: Theme.of(context).textTheme.titleMedium),
              const SizedBox(width: 12),
              Expanded(
                child: Text(
                  'Message requests ($count)',
                  style: Theme.of(context).textTheme.titleSmall,
                ),
              ),
              Text('›', style: Theme.of(context).textTheme.titleMedium),
            ],
          ),
        ),
      ),
    );
  }
}

class _ConversationRow extends StatelessWidget {
  const _ConversationRow({
    required this.convo,
    required this.title,
    required this.selected,
    required this.onTap,
  });

  final ConversationInfo convo;
  final String title;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final ColorScheme colors = Theme.of(context).colorScheme;
    return Material(
      color: selected ? colors.primaryContainer.withValues(alpha: 0.35) : null,
      child: InkWell(
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 11),
          child: Row(
            children: <Widget>[
              PeerAvatar(title: title, seed: convo.peer),
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
                      '${convo.lastOutgoing ? 'You: ' : ''}${convo.lastMessage}',
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
              const SizedBox(width: 8),
              Text(
                relativeTime(convo.lastAt),
                style: Theme.of(context)
                    .textTheme
                    .labelSmall
                    ?.copyWith(color: colors.outline),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
