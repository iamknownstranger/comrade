/// The thread sheet and the topic sheet — Telegram's forum topics and Slack's
/// threads, over a two-person chat.
///
/// Two sheets, not two routes. A thread is read *beside* the conversation it
/// came from, not instead of it: pushing a route would make going back a
/// navigation decision, and the whole point of a thread is that you dip into it
/// and return. `showModalBottomSheet` is what the message-action sheet already
/// uses here, and what Android's `ThreadSheet.kt` uses for the same reason.
///
/// ## What is decided elsewhere
///
/// Ordering, filtering, which rows are hidden and what `/assign` does are
/// `util/topic_view.dart` — pure, unit-tested, and mirrored by
/// `TopicDecisions.kt` and `topics.mjs`. This file draws them. The slug rules
/// and the reply graph are `comrade_core::topic`, over the bridge.
///
/// The one thing this file decides on its own is the *word* for a preview core
/// deliberately left unworded ([PreviewKind]) — because a sentence has to live
/// where it can be localised, and core cannot reach that.
library;

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../data/comrade_repository.dart';
import '../../data/models.dart';
import '../../state/topic_providers.dart';
import '../../theme/comrade_theme.dart';
import '../../util/topic_view.dart';
import '../../widgets/glass_surface.dart';
import '../../widgets/list_skeleton.dart';

/// Open the topic sheet.
///
/// [filingMessageId] turns every topic row into a *destination* rather than a
/// place to read — the message whose thread is about to be filed. Held as a
/// parameter rather than sheet state because "the sheet is a destination" and
/// "the sheet is a reading surface" are two modes and a row must not mean both
/// at once.
///
/// Returns the slug the thread was filed under, `''` if it was taken out of
/// every topic, or null if nothing was filed — so the caller can say so out
/// loud. Filing produces no chat bubble on either side
/// (`comrade_core::topic`'s module header has the reason), which makes that
/// sentence the only trace the action leaves.
Future<String?> showTopicSheet(
  BuildContext context, {
  required String peer,
  String? filingMessageId,
  required void Function(String rootId) onOpenThread,
}) =>
    showModalBottomSheet<String?>(
      context: context,
      showDragHandle: true,
      useSafeArea: true,
      isScrollControlled: true,
      backgroundColor: Colors.transparent,
      builder: (BuildContext sheetContext) => GlassSurface(
        tier: GlassTier.sheet,
        borderRadius: kGlassSheetRadius,
        child: _TopicSheet(
          peer: peer,
          filingMessageId: filingMessageId,
          onOpenThread: onOpenThread,
        ),
      ),
    );

/// Open one thread. [rootId] may name any message in it — core walks up to the
/// root, so a sheet opened from a reply shows the same thread as one opened
/// from the top.
Future<void> showThreadSheet(
  BuildContext context, {
  required String peer,
  required String rootId,
  required VoidCallback onFile,
}) =>
    showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      useSafeArea: true,
      isScrollControlled: true,
      backgroundColor: Colors.transparent,
      builder: (BuildContext sheetContext) => GlassSurface(
        tier: GlassTier.sheet,
        borderRadius: kGlassSheetRadius,
        child: _ThreadSheet(peer: peer, rootId: rootId, onFile: onFile),
      ),
    );

class _TopicSheet extends ConsumerStatefulWidget {
  const _TopicSheet({
    required this.peer,
    required this.filingMessageId,
    required this.onOpenThread,
  });

  final String peer;
  final String? filingMessageId;
  final void Function(String rootId) onOpenThread;

  @override
  ConsumerState<_TopicSheet> createState() => _TopicSheetState();
}

class _TopicSheetState extends ConsumerState<_TopicSheet> {
  final TextEditingController _name = TextEditingController();
  ThreadFilter _filter = const ThreadFilter.all();
  bool _showArchived = false;
  bool _naming = false;
  bool _busy = false;
  String? _error;

  bool get _filing => widget.filingMessageId != null;

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  Future<void> _file(String? topicName) async {
    final String? target = widget.filingMessageId;
    if (target == null || _busy) return;
    setState(() => _busy = true);
    try {
      final ThreadInfo filed = await ref
          .read(topicsProvider(widget.peer).notifier)
          .assignThread(messageId: target, topicName: topicName);
      if (!mounted) return;
      // `''` rather than null for an unfiling: the caller has to be able to tell
      // "taken out of that topic" from "nothing happened", and both are a
      // successful outcome the user asked for.
      Navigator.of(context).pop(filed.topicSlug ?? '');
    } on ComradeException catch (e) {
      if (!mounted) return;
      setState(() {
        _error = e.message;
        _busy = false;
      });
    }
  }

  Future<void> _create() async {
    final String name = _name.text.trim();
    if (name.isEmpty || _busy) return;
    // Naming and filing in one gesture when there is something to file:
    // somebody who typed a name while a thread was selected meant both, and
    // making them tap the row they just created is a step that exists only
    // because the code has two calls.
    if (_filing) {
      await _file(name);
      return;
    }
    setState(() => _busy = true);
    try {
      await ref.read(topicsProvider(widget.peer).notifier).createTopic(name);
      if (!mounted) return;
      setState(() {
        _name.clear();
        _naming = false;
        _error = null;
        _busy = false;
      });
    } on ComradeException catch (e) {
      if (!mounted) return;
      setState(() {
        _error = e.message;
        _busy = false;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final AsyncValue<TopicsState> async =
        ref.watch(topicsProvider(widget.peer));
    return Padding(
      padding: EdgeInsets.only(
        bottom: MediaQuery.of(context).viewInsets.bottom,
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 20),
            child: Row(
              children: <Widget>[
                Expanded(
                  child: Text(
                    _filing ? 'File under a topic' : 'Threads and topics',
                    style: Theme.of(context).textTheme.titleMedium,
                  ),
                ),
                TextButton(
                  key: const Key('topic-new'),
                  onPressed: () => setState(() => _naming = !_naming),
                  child: const Text('New topic'),
                ),
              ],
            ),
          ),
          if (_naming)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
              child: Row(
                children: <Widget>[
                  Expanded(
                    child: TextField(
                      key: const Key('topic-name'),
                      controller: _name,
                      decoration: const InputDecoration(
                        hintText: 'Name it, like Flat deposit',
                      ),
                      onSubmitted: (_) => _create(),
                    ),
                  ),
                  TextButton(
                    key: const Key('topic-create'),
                    onPressed: _busy ? null : _create,
                    child: const Text('Create'),
                  ),
                ],
              ),
            ),
          if (_error != null)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 4),
              child: Text(
                _error!,
                style: TextStyle(color: Theme.of(context).colorScheme.error),
              ),
            ),
          Flexible(
            child: async.when(
              // §7.3: the row shape is known before the list loads.
              loading: () => Padding(
                padding: const EdgeInsets.all(ComradeSpacing.space4),
                child: ListSkeleton.peerRows(
                  key: const Key('topics-skeleton'),
                  rowCount: 3,
                ),
              ),
              error: (Object e, _) => Padding(
                padding: const EdgeInsets.all(20),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: <Widget>[
                    Text(e is ComradeException ? e.message : '$e'),
                    const SizedBox(height: ComradeSpacing.space2),
                    TextButton(
                      onPressed: () =>
                          ref.invalidate(topicsProvider(widget.peer)),
                      child: const Text('Try again'),
                    ),
                  ],
                ),
              ),
              data: (TopicsState state) => _body(context, state),
            ),
          ),
        ],
      ),
    );
  }

  Widget _body(BuildContext context, TopicsState state) {
    final List<TopicInfo> topics =
        visibleTopics(state.topics, includeClosed: _showArchived);
    final List<ThreadInfo> rows = threadsFor(
      state.threads,
      filter: _filter,
      // Under a named topic, a thread of one is there because somebody filed
      // it — hiding it would lose the filing.
      includeSingletons: _filter.kind == ThreadFilterKind.inTopic,
    );
    return ListView(
      // Keyed for the same reason as `thread-log` above.
      key: const Key('threads-list'),
      shrinkWrap: true,
      padding: const EdgeInsets.only(bottom: 24),
      children: <Widget>[
        if (!_filing)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16),
            child: Wrap(
              spacing: 8,
              children: <Widget>[
                FilterChip(
                  label: const Text('All threads'),
                  selected: _filter.kind == ThreadFilterKind.all,
                  onSelected: (_) =>
                      setState(() => _filter = const ThreadFilter.all()),
                ),
                FilterChip(
                  label: const Text('Not filed'),
                  selected: _filter.kind == ThreadFilterKind.unfiled,
                  onSelected: (_) =>
                      setState(() => _filter = const ThreadFilter.unfiled()),
                ),
                if (state.topics.any((TopicInfo t) => t.closed))
                  FilterChip(
                    label: const Text('Show archived'),
                    selected: _showArchived,
                    onSelected: (bool v) => setState(() => _showArchived = v),
                  ),
              ],
            ),
          ),
        if (topics.isEmpty)
          const Padding(
            padding: EdgeInsets.all(20),
            child:
                Text('No topics yet. Name one, then file a thread under it.'),
          ),
        for (final TopicInfo topic in topics)
          ListTile(
            key: Key('topic-${topic.slug}'),
            leading: const Icon(Icons.tag),
            title: Text(topic.name),
            subtitle: Text(
              topic.closed
                  ? 'Archived'
                  : '${topic.threadCount} · ${topic.messageCount} messages',
            ),
            selected: _filter.slug == topic.slug,
            trailing: _filing
                ? null
                : Row(
                    mainAxisSize: MainAxisSize.min,
                    children: <Widget>[
                      if (unreadThreadCount(state.threads, topic.slug) > 0)
                        _Badge(unreadThreadCount(state.threads, topic.slug)),
                      TextButton(
                        onPressed: _busy
                            ? null
                            : () => ref
                                .read(topicsProvider(widget.peer).notifier)
                                .setTopicClosed(topic.slug,
                                    closed: !topic.closed),
                        child: Text(topic.closed ? 'Bring back' : 'Archive'),
                      ),
                    ],
                  ),
            onTap: () {
              if (_filing) {
                _file(topic.name);
              } else {
                setState(() => _filter = _filter.slug == topic.slug
                    ? const ThreadFilter.all()
                    : ThreadFilter.inTopic(topic.slug));
              }
            },
          ),
        // Taking a thread out of every topic is a destination too, and only
        // exists while something is being filed.
        if (_filing)
          ListTile(
            key: const Key('topic-unfile'),
            title: const Text('Take out of this topic'),
            onTap: () => _file(null),
          ),
        if (!_filing) ...<Widget>[
          const Divider(),
          if (rows.isEmpty)
            Padding(
              padding: const EdgeInsets.all(20),
              child: Text(
                _filter.kind == ThreadFilterKind.inTopic
                    ? 'Nothing filed here yet.'
                    : 'No threads yet. Reply to a message and it becomes one.',
              ),
            ),
          for (final ThreadInfo row in rows)
            ListTile(
              key: Key('thread-${row.rootId}'),
              leading: const Icon(Icons.forum_outlined),
              title: Text(
                _previewText(row),
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontWeight: row.unread ? FontWeight.w600 : FontWeight.normal,
                ),
              ),
              subtitle: Text(
                row.topicSlug == null
                    ? _replies(row.replyCount)
                    : '${_replies(row.replyCount)} · #${row.topicSlug}',
              ),
              trailing: row.unread ? const _Dot() : null,
              onTap: () {
                Navigator.of(context).pop();
                widget.onOpenThread(row.rootId);
              },
            ),
        ],
      ],
    );
  }

  String _replies(int n) => n == 1 ? '1 reply' : '$n replies';
}

/// The one place this file chooses a word, and it chooses from [PreviewKind] so
/// the *branch* is what the unit test pins and the sentence is what a
/// translator would change.
String _previewText(ThreadInfo row) {
  switch (previewKind(row)) {
    case PreviewKind.text:
      return row.preview;
    case PreviewKind.captionedMedia:
      return '📎 ${row.preview}';
    case PreviewKind.bareMedia:
      return '📎 Attachment';
    case PreviewKind.rootMissing:
      return "The first message isn't on this device";
    case PreviewKind.empty:
      return '(no text)';
  }
}

class _ThreadSheet extends ConsumerStatefulWidget {
  const _ThreadSheet({
    required this.peer,
    required this.rootId,
    required this.onFile,
  });

  final String peer;
  final String rootId;
  final VoidCallback onFile;

  @override
  ConsumerState<_ThreadSheet> createState() => _ThreadSheetState();
}

class _ThreadSheetState extends ConsumerState<_ThreadSheet> {
  final TextEditingController _draft = TextEditingController();
  final ScrollController _scroll = ScrollController();
  bool _sending = false;
  String? _error;

  @override
  void dispose() {
    _draft.dispose();
    _scroll.dispose();
    super.dispose();
  }

  ThreadKey get _key => (peer: widget.peer, rootId: widget.rootId);

  Future<void> _send() async {
    final String text = _draft.text.trim();
    if (text.isEmpty || _sending) return;
    setState(() => _sending = true);
    try {
      await ref.read(threadProvider(_key).notifier).reply(text);
      if (!mounted) return;
      setState(() {
        _draft.clear();
        _error = null;
        _sending = false;
      });
    } on ComradeException catch (e) {
      if (!mounted) return;
      setState(() {
        _error = e.message;
        _sending = false;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final AsyncValue<ThreadDetail> async = ref.watch(threadProvider(_key));
    final List<ThreadEntry> entries = async.value == null
        ? const <ThreadEntry>[]
        : ThreadController.entries(async.value!);
    // Newest last, and a thread is short — so land at the bottom, with none of
    // the conversation's "are they scrolled up" bookkeeping. A sheet you opened
    // deliberately is a sheet you are reading.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (_scroll.hasClients) {
        _scroll.jumpTo(_scroll.position.maxScrollExtent);
      }
    });
    return Padding(
      padding: EdgeInsets.only(
        bottom: MediaQuery.of(context).viewInsets.bottom,
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 20),
            child: Row(
              children: <Widget>[
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: <Widget>[
                      Text(
                        'Thread',
                        style: Theme.of(context).textTheme.titleMedium,
                      ),
                      Text(
                        entries.length <= 1
                            ? '0 replies'
                            : '${entries.length - 1} replies',
                        style: Theme.of(context).textTheme.bodySmall,
                      ),
                    ],
                  ),
                ),
                // The topic this thread is in, if any — a chip rather than a
                // line, so it reads as a label on the thread rather than as
                // something somebody said.
                if (async.value?.topicSlug != null)
                  Padding(
                    padding: const EdgeInsets.only(right: 8),
                    child: Chip(label: Text('#${async.value!.topicSlug}')),
                  ),
                TextButton.icon(
                  key: const Key('thread-file'),
                  onPressed: () {
                    Navigator.of(context).pop();
                    widget.onFile();
                  },
                  icon: const Icon(Icons.tag, size: 18),
                  label: const Text('File under a topic…'),
                ),
              ],
            ),
          ),
          const Divider(),
          Flexible(
            child: async.when(
              loading: () => const Padding(
                padding: EdgeInsets.all(24),
                child: Center(child: CircularProgressIndicator()),
              ),
              error: (Object e, _) => Padding(
                padding: const EdgeInsets.all(20),
                child: Text(e is ComradeException ? e.message : '$e'),
              ),
              data: (ThreadDetail _) => ListView.builder(
                // Keyed so a test can scope a finder to the thread rather than
                // to the conversation still sitting behind the sheet — a modal
                // bottom sheet does not remove it from the tree, so an
                // unscoped `find.text` matches both.
                key: const Key('thread-log'),
                controller: _scroll,
                shrinkWrap: true,
                padding: const EdgeInsets.symmetric(
                  horizontal: 16,
                  vertical: 8,
                ),
                itemCount: entries.length,
                itemBuilder: (BuildContext context, int i) =>
                    _ThreadBubble(entries[i]),
              ),
            ),
          ),
          if (_error != null)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 20),
              child: Text(
                _error!,
                style: TextStyle(color: Theme.of(context).colorScheme.error),
              ),
            ),
          Padding(
            padding: const EdgeInsets.all(12),
            child: Row(
              children: <Widget>[
                Expanded(
                  child: TextField(
                    key: const Key('thread-input'),
                    controller: _draft,
                    minLines: 1,
                    maxLines: 4,
                    decoration: const InputDecoration(
                      hintText: 'Reply in this thread',
                    ),
                    onSubmitted: (_) => _send(),
                  ),
                ),
                IconButton.filled(
                  key: const Key('thread-send'),
                  onPressed: _sending ? null : _send,
                  icon: const Icon(Icons.send),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _ThreadBubble extends StatelessWidget {
  const _ThreadBubble(this.entry);

  final ThreadEntry entry;

  @override
  Widget build(BuildContext context) {
    final String text = entry.message?.content ??
        (entry.media!.caption.trim().isEmpty
            ? '📎 Attachment'
            : '📎 ${entry.media!.caption}');
    final ColorScheme scheme = Theme.of(context).colorScheme;
    return Align(
      alignment: entry.outgoing ? Alignment.centerRight : Alignment.centerLeft,
      child: Container(
        margin: const EdgeInsets.symmetric(vertical: 3),
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          color: entry.outgoing
              ? scheme.primaryContainer
              : scheme.surfaceContainerHighest,
        ),
        child: Text(text),
      ),
    );
  }
}

class _Badge extends StatelessWidget {
  const _Badge(this.count);

  final int count;

  @override
  Widget build(BuildContext context) => Container(
        padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 2),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(50),
          color: Theme.of(context).colorScheme.primary,
        ),
        child: Text(
          '$count',
          style: TextStyle(
            fontSize: 11,
            color: Theme.of(context).colorScheme.onPrimary,
          ),
        ),
      );
}

class _Dot extends StatelessWidget {
  const _Dot();

  @override
  Widget build(BuildContext context) => Container(
        width: 8,
        height: 8,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: Theme.of(context).colorScheme.primary,
        ),
      );
}
