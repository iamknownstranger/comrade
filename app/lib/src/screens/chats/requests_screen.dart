/// The message-requests inbox: strangers' first DMs, gated out of the chat
/// list.
///
/// Accepting shares your @handle with them and moves the thread into Chats;
/// blocking drops their future messages. Port of `ChatsScreen.kt`'s
/// `RequestsScreen`.
///
/// Note what is deliberately *not* shown: a request row renders the raw key,
/// never a published @handle. A stranger's self-declared name is exactly the
/// thing an impersonation attempt would set, and this screen is where the
/// trust decision happens.
library;

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../data/models.dart';
import '../../state/chat_providers.dart';
import '../../util/display_name.dart';
import '../../widgets/app_chrome.dart';
import '../../widgets/list_skeleton.dart';
import '../../widgets/peer_avatar.dart';

class RequestsScreen extends ConsumerWidget {
  const RequestsScreen({required this.onOpen, super.key});

  final void Function(ChatTarget target) onOpen;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final AsyncValue<List<MessageRequestInfo>> requests =
        ref.watch(messageRequestsProvider);

    return requests.when(
      // §7.3: the row shape is known before the list loads.
      loading: () => ListSkeleton.peerRows(key: const Key('requests-skeleton')),
      error: (Object e, StackTrace s) => EmptyState(
        title: 'Could not load requests',
        body: '$e',
        action: FilledButton(
          onPressed: () => ref.invalidate(messageRequestsProvider),
          child: const Text('Try again'),
        ),
      ),
      data: (List<MessageRequestInfo> list) {
        if (list.isEmpty) {
          return const EmptyState(title: 'No message requests.');
        }
        return ReadingColumn(
          child: ListView.separated(
            itemCount: list.length,
            separatorBuilder: (_, __) => const Divider(height: 1),
            itemBuilder: (BuildContext context, int index) {
              final MessageRequestInfo req = list[index];
              return Padding(
                padding:
                    const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: <Widget>[
                    Row(
                      children: <Widget>[
                        PeerAvatar(title: shortNpub(req.peer), seed: req.peer),
                        const SizedBox(width: 14),
                        Expanded(
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            mainAxisSize: MainAxisSize.min,
                            children: <Widget>[
                              KeyText(
                                req.peer,
                                style: Theme.of(context).textTheme.titleSmall,
                              ),
                              Text(
                                req.lastMessage,
                                maxLines: 2,
                                overflow: TextOverflow.ellipsis,
                                style: Theme.of(context)
                                    .textTheme
                                    .bodyMedium
                                    ?.copyWith(
                                      color: Theme.of(context)
                                          .colorScheme
                                          .onSurfaceVariant,
                                    ),
                              ),
                            ],
                          ),
                        ),
                      ],
                    ),
                    const SizedBox(height: 8),
                    Row(
                      children: <Widget>[
                        OutlinedButton(
                          onPressed: () => ref
                              .read(messageRequestsProvider.notifier)
                              .block(req.peer),
                          child: const Text('Block'),
                        ),
                        const SizedBox(width: 8),
                        FilledButton(
                          onPressed: () async {
                            await ref
                                .read(messageRequestsProvider.notifier)
                                .accept(req.peer);
                            onOpen(ChatTarget(peer: req.peer));
                          },
                          child: const Text('Accept'),
                        ),
                      ],
                    ),
                  ],
                ),
              );
            },
          ),
        );
      },
    );
  }
}
