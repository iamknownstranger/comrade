/// The public feed (Sabha): broadcast short posts to open relays and watch the
/// live public stream. Unlike Chats, nothing here is private — the composer
/// says so explicitly.
///
/// Ported from `ui/FeedScreen.kt` and `desktop/ui` `#view-sabha`. Two things
/// come from desktop: the 2,000-character limit with a live counter (Android
/// had neither, so a post could be silently rejected by a relay), and the
/// centred reading column that stops a Chitthi stretching to 3,000 px on an
/// ultrawide monitor.
library;

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../data/comrade_repository.dart';
import '../data/models.dart';
import '../state/content_providers.dart';
import '../state/providers.dart';
import '../util/display_name.dart';
import '../widgets/app_chrome.dart';
import '../widgets/list_skeleton.dart';
import '../widgets/peer_avatar.dart';

const int kChitthiMaxChars = 2000;

class FeedScreen extends ConsumerStatefulWidget {
  const FeedScreen({super.key});

  @override
  ConsumerState<FeedScreen> createState() => _FeedScreenState();
}

class _FeedScreenState extends ConsumerState<FeedScreen> {
  final TextEditingController _draft = TextEditingController();
  bool _busy = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    _draft.addListener(() => setState(() {}));
  }

  @override
  void dispose() {
    _draft.dispose();
    super.dispose();
  }

  Future<void> _post() async {
    final String text = _draft.text.trim();
    if (text.isEmpty || _busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await ref.read(feedProvider.notifier).post(
            text,
            author: ref.read(profileProvider)?.npub,
          );
      if (!mounted) return;
      _draft.clear();
      setState(() => _busy = false);
    } on ComradeException catch (e) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = e.message;
        });
      }
    } catch (_) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = 'Could not post.';
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final AsyncValue<List<ChitthiInfo>> feed = ref.watch(feedProvider);
    final String? me = ref.watch(profileProvider)?.npub;

    return ReadingColumn(
      child: ListView(
        padding: const EdgeInsets.all(16),
        children: <Widget>[
          _composer(context),
          const SizedBox(height: 14),
          ...feed.when(
            // §7.3: the row shape is known before the list loads.
            loading: () => <Widget>[
              ListSkeleton.cardRows(key: const Key('feed-skeleton')),
            ],
            error: (Object e, StackTrace s) => <Widget>[
              EmptyState(
                title: 'Could not load the timeline',
                body: '$e',
                action: FilledButton(
                  onPressed: () => ref.invalidate(feedProvider),
                  child: const Text('Try again'),
                ),
              ),
            ],
            data: (List<ChitthiInfo> items) {
              if (items.isEmpty) {
                return <Widget>[
                  Padding(
                    padding: const EdgeInsets.only(top: 16),
                    child: Text(
                      'Nothing here yet. Live public posts stream in as the '
                      'relays deliver them.',
                      style: Theme.of(context).textTheme.bodySmall?.copyWith(
                            color:
                                Theme.of(context).colorScheme.onSurfaceVariant,
                          ),
                    ),
                  ),
                ];
              }
              return <Widget>[
                for (final ChitthiInfo post in items)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 10),
                    child: _ChitthiCard(post: post, isMine: post.author == me),
                  ),
              ];
            },
          ),
        ],
      ),
    );
  }

  Widget _composer(BuildContext context) {
    final int used = _draft.text.characters.length;
    final bool over = used > kChitthiMaxChars;
    return SectionCard(
      padding: const EdgeInsets.all(12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: <Widget>[
          TextField(
            key: const Key('feed-input'),
            controller: _draft,
            minLines: 3,
            maxLines: 8,
            maxLength: kChitthiMaxChars,
            buildCounter: (
              BuildContext context, {
              required int currentLength,
              required bool isFocused,
              required int? maxLength,
            }) =>
                null,
            decoration: const InputDecoration(
                hintText: 'Write a Chitthi to the world…'),
          ),
          const SizedBox(height: 8),
          Row(
            children: <Widget>[
              Expanded(
                child: Text(
                  'Public — anyone on the network can read this.',
                  style: Theme.of(context).textTheme.labelSmall?.copyWith(
                        color: Theme.of(context).colorScheme.outline,
                      ),
                ),
              ),
              Text(
                '$used / $kChitthiMaxChars',
                style: Theme.of(context).textTheme.labelSmall?.copyWith(
                      color: over
                          ? Theme.of(context).colorScheme.error
                          : Theme.of(context).colorScheme.outline,
                    ),
              ),
              const SizedBox(width: 12),
              BusyButton(
                key: const Key('feed-post'),
                label: 'Post',
                busyLabel: 'Posting…',
                busy: _busy,
                onPressed: _draft.text.trim().isEmpty || over ? null : _post,
              ),
            ],
          ),
          ErrorText(_error),
        ],
      ),
    );
  }
}

class _ChitthiCard extends StatelessWidget {
  const _ChitthiCard({required this.post, required this.isMine});

  final ChitthiInfo post;
  final bool isMine;

  @override
  Widget build(BuildContext context) => SectionCard(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: <Widget>[
            Row(
              children: <Widget>[
                PeerAvatar(
                  title: isMine ? 'You' : shortNpub(post.author),
                  seed: post.author,
                  size: 28,
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: isMine
                      ? Text(
                          'You',
                          style: Theme.of(context)
                              .textTheme
                              .labelMedium
                              ?.copyWith(
                                color: Theme.of(context).colorScheme.primary,
                              ),
                        )
                      : KeyText(
                          post.author,
                          style: Theme.of(context).textTheme.labelMedium,
                        ),
                ),
                Text(
                  relativeTime(post.createdAt),
                  style: Theme.of(context).textTheme.labelSmall?.copyWith(
                        color: Theme.of(context).colorScheme.outline,
                      ),
                ),
              ],
            ),
            const SizedBox(height: 6),
            Text(post.content, style: Theme.of(context).textTheme.bodyMedium),
            if (post.replyTo != null)
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: Text(
                  '↳ reply to ${post.replyTo!.length > 12 ? '${post.replyTo!.substring(0, 12)}…' : post.replyTo}',
                  style: Theme.of(context).textTheme.labelSmall?.copyWith(
                        color: Theme.of(context).colorScheme.outline,
                      ),
                ),
              ),
          ],
        ),
      );
}
