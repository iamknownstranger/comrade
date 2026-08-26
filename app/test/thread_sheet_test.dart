/// The thread sheet and the topic sheet, driven through the real widgets
/// against the in-memory fake.
///
/// The invariants worth pinning here are the ones a pure test cannot reach:
/// that opening from a *reply* lands on the thread rather than starting a second
/// one, that filing says so out loud (nothing else does — filing produces no
/// chat bubble on either side), and that naming a topic twice is one topic.
library;

import 'dart:async';

import 'package:comrade/src/data/fake_comrade_repository.dart';
import 'package:comrade/src/data/models.dart';
import 'package:comrade/src/screens/chats/conversation_screen.dart';
import 'package:comrade/src/screens/chats/thread_sheet.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'helpers.dart';

/// Never resolves `topics()`, so the sheet's own loading branch can be
/// pinned rather than raced past.
class _StuckTopicsRepo extends FakeComradeRepository {
  _StuckTopicsRepo() : super(latency: Duration.zero, seed: false);

  @override
  Future<List<TopicInfo>> topics(String peer) =>
      Completer<List<TopicInfo>>().future;
}

/// A conversation of `root` plus two replies into it, and one unrelated
/// message — the smallest history in which "thread" means anything. Mirrors
/// `threaded_chat` in `comrade_ui`'s runtime tests.
Future<({String root, String reply})> threadedChat(
  FakeComradeRepository repo,
  String peer,
) async {
  final MessageInfo root = await repo.sendDm(
    peer: peer,
    content: "the deposit still hasn't come back",
  );
  final MessageInfo reply = await repo.sendDm(
    peer: peer,
    content: "i'll chase them monday",
    replyTo: root.id,
  );
  // A reply to the *reply*, so the walk up the chain has something to do.
  await repo.sendDm(peer: peer, content: 'thanks', replyTo: reply.id);
  await repo.sendDm(peer: peer, content: 'unrelated: dinner?');
  return (root: root.id, reply: reply.id);
}

void main() {
  group('the threads bar', () {
    testWidgets('is absent until the conversation has a thread or a topic',
        (WidgetTester tester) async {
      setWindowSize(tester, const Size(420, 900));
      final FakeComradeRepository repo =
          FakeComradeRepository(latency: Duration.zero, seed: false);
      await repo.unlockVault(path: 't', passphrase: 'p');
      await repo.sendDm(peer: 'npub1nobody', content: 'hello');

      await tester.pumpWidget(
        harness(const ConversationScreen(peer: 'npub1nobody'), repo: repo),
      );
      await tester.pumpAndSettle();

      // One message nobody replied to is a thread of one, which the sheet hides
      // — so there is nothing to offer a way in to, and no chrome is spent.
      expect(find.byKey(const Key('threads-bar')), findsNothing);
    });

    testWidgets('appears once a message has a reply',
        (WidgetTester tester) async {
      setWindowSize(tester, const Size(420, 900));
      final FakeComradeRepository repo = await unlockedFake(seed: false);
      await threadedChat(repo, FakePeers.alice);

      await tester.pumpWidget(
        harness(const ConversationScreen(peer: FakePeers.alice), repo: repo),
      );
      await tester.pumpAndSettle();

      expect(find.byKey(const Key('threads-bar')), findsOneWidget);
    });
  });

  group('opening a thread', () {
    testWidgets('from a reply shows the whole thread, not a new one',
        (WidgetTester tester) async {
      setWindowSize(tester, const Size(420, 900));
      final FakeComradeRepository repo = await unlockedFake(seed: false);
      final ({String root, String reply}) ids =
          await threadedChat(repo, FakePeers.alice);

      await tester.pumpWidget(
        harness(const ConversationScreen(peer: FakePeers.alice), repo: repo),
      );
      await tester.pumpAndSettle();

      // Long-press the *middle* message and open its thread.
      await tester.longPress(find.text("i'll chase them monday").first);
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('action-open-thread')));
      await tester.pumpAndSettle();

      expect(find.byKey(const Key('thread-input')), findsOneWidget);
      // Scoped to the sheet: a modal bottom sheet leaves the conversation in
      // the tree behind it, so an unscoped finder would match the bubble there
      // and prove nothing about the thread.
      Finder inThread(String text) => find.descendant(
            of: find.byKey(const Key('thread-log')),
            matching: find.text(text),
          );
      // The root and both replies, reached from the middle of the chain.
      expect(inThread("the deposit still hasn't come back"), findsOneWidget);
      expect(inThread('thanks'), findsOneWidget);
      expect(
        inThread('unrelated: dinner?'),
        findsNothing,
        reason: 'a thread is not the whole conversation',
      );
      expect(find.text('2 replies'), findsOneWidget);

      // And core resolved the root rather than the tapped id.
      final ThreadDetail detail =
          await repo.thread(peer: FakePeers.alice, rootId: ids.reply);
      expect(detail.rootId, ids.root);
    });

    testWidgets('a reply typed in the sheet is addressed to the root',
        (WidgetTester tester) async {
      setWindowSize(tester, const Size(420, 900));
      final FakeComradeRepository repo = await unlockedFake(seed: false);
      final ({String root, String reply}) ids =
          await threadedChat(repo, FakePeers.alice);

      await tester.pumpWidget(
        harness(const ConversationScreen(peer: FakePeers.alice), repo: repo),
      );
      await tester.pumpAndSettle();
      await tester.longPress(find.text('thanks').first);
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('action-open-thread')));
      await tester.pumpAndSettle();

      await tester.enterText(
        find.byKey(const Key('thread-input')),
        'any news?',
      );
      await tester.tap(find.byKey(const Key('thread-send')));
      await tester.pumpAndSettle();

      // The flatness is the feature: opened from the deepest reply, the new
      // message still hangs off the root.
      final List<MessageInfo> msgs = await repo.messages(FakePeers.alice);
      final MessageInfo sent =
          msgs.firstWhere((MessageInfo m) => m.content == 'any news?');
      expect(sent.replyTo, ids.root);
      expect(find.text('3 replies'), findsOneWidget);
    });
  });

  group('filing a thread', () {
    testWidgets('names the topic and says so, because nothing else does',
        (WidgetTester tester) async {
      setWindowSize(tester, const Size(420, 900));
      final FakeComradeRepository repo = await unlockedFake(seed: false);
      final ({String root, String reply}) ids =
          await threadedChat(repo, FakePeers.alice);

      await tester.pumpWidget(
        harness(const ConversationScreen(peer: FakePeers.alice), repo: repo),
      );
      await tester.pumpAndSettle();
      await tester.longPress(find.text("i'll chase them monday").first);
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('action-assign-topic')));
      await tester.pumpAndSettle();

      // Name it and file it in one gesture — somebody who typed a name while a
      // thread was selected meant both.
      await tester.tap(find.byKey(const Key('topic-new')));
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const Key('topic-name')),
        'Flat deposit',
      );
      await tester.tap(find.byKey(const Key('topic-create')));
      await tester.pumpAndSettle();

      // Filing emits no bubble on either side, so the snackbar is the only
      // trace the action leaves — that is why it is asserted.
      expect(find.text('Filed under #flat-deposit.'), findsOneWidget);
      expect(
        (await repo.threads(FakePeers.alice, topicSlug: 'flat-deposit'))
            .single
            .rootId,
        ids.root,
        reason: 'filing from a reply files the thread, not the reply',
      );
    });

    testWidgets('taking a thread out of a topic is also reported',
        (WidgetTester tester) async {
      setWindowSize(tester, const Size(420, 900));
      final FakeComradeRepository repo = await unlockedFake(seed: false);
      final ({String root, String reply}) ids =
          await threadedChat(repo, FakePeers.alice);
      await repo.assignThread(
        peer: FakePeers.alice,
        messageId: ids.root,
        topicName: 'Flat deposit',
      );

      await tester.pumpWidget(
        harness(const ConversationScreen(peer: FakePeers.alice), repo: repo),
      );
      await tester.pumpAndSettle();
      await tester.longPress(find.text('thanks').first);
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('action-assign-topic')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('topic-unfile')));
      await tester.pumpAndSettle();

      expect(find.text('Taken out of that topic.'), findsOneWidget);
      // The topic survives the unfiling — nothing here deletes one.
      expect((await repo.topics(FakePeers.alice)).single.slug, 'flat-deposit');
    });

    testWidgets('naming the same word twice is one topic',
        (WidgetTester tester) async {
      // Both people typing `/assign #deposit` before either envelope lands is
      // the case the slug-as-id keying exists for.
      setWindowSize(tester, const Size(420, 900));
      final FakeComradeRepository repo = await unlockedFake(seed: false);
      await threadedChat(repo, FakePeers.alice);
      await repo.createTopic(peer: FakePeers.alice, name: 'Deposit');
      await repo.createTopic(peer: FakePeers.alice, name: 'deposit');

      await tester.pumpWidget(
        harness(const ConversationScreen(peer: FakePeers.alice), repo: repo),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('threads-bar')));
      await tester.pumpAndSettle();

      expect(find.byKey(const Key('topic-deposit')), findsOneWidget);
      expect(
        find.text('Deposit'),
        findsOneWidget,
        reason: 'the first spelling is the one that is kept',
      );
    });
  });

  group('the topic sheet', () {
    testWidgets('lists threads and opens one', (WidgetTester tester) async {
      setWindowSize(tester, const Size(420, 900));
      final FakeComradeRepository repo = await unlockedFake(seed: false);
      await threadedChat(repo, FakePeers.alice);

      await tester.pumpWidget(
        harness(const ConversationScreen(peer: FakePeers.alice), repo: repo),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('threads-bar')));
      await tester.pumpAndSettle();

      // One thread row, with replies. The unrelated message is a thread of one
      // and is hidden — listing it would make the sheet a worse copy of the
      // chat. Scoped to the sheet's own list, because the conversation is still
      // in the tree behind it.
      final Finder rows = find.descendant(
        of: find.byKey(const Key('threads-list')),
        matching: find.byWidgetPredicate(
          (Widget w) => w is ListTile && w.key.toString().contains('thread-'),
        ),
      );
      expect(rows, findsOneWidget);
      expect(find.text('2 replies'), findsOneWidget);

      await tester.tap(rows);
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('thread-input')), findsOneWidget);
    });

    testWidgets('a name that cannot be a key is refused, not mangled',
        (WidgetTester tester) async {
      // AUDIT TOPIC-1: a Devanagari topic name is a thing users will type, and it
      // has to fail visibly rather than become `#--`.
      setWindowSize(tester, const Size(420, 900));
      final FakeComradeRepository repo = await unlockedFake(seed: false);
      await threadedChat(repo, FakePeers.alice);

      await tester.pumpWidget(
        harness(const ConversationScreen(peer: FakePeers.alice), repo: repo),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('threads-bar')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('topic-new')));
      await tester.pumpAndSettle();
      await tester.enterText(find.byKey(const Key('topic-name')), 'जमा');
      await tester.tap(find.byKey(const Key('topic-create')));
      await tester.pumpAndSettle();

      expect(find.textContaining('two or more letters'), findsOneWidget);
      expect(await repo.topics(FakePeers.alice), isEmpty);
    });

    testWidgets('the row shape shows while topics load, not a bare spinner',
        (WidgetTester tester) async {
      // `showTopicSheet` directly, rather than through `ConversationScreen`'s
      // threads-bar: that bar only renders once `topicsProvider` already has
      // data, which a repo whose `topics()` never resolves would never reach.
      setWindowSize(tester, const Size(420, 900));
      final _StuckTopicsRepo repo = _StuckTopicsRepo();
      await repo.unlockVault(path: 't', passphrase: 'p');

      await tester.pumpWidget(harness(
        Builder(
          builder: (BuildContext context) => ElevatedButton(
            onPressed: () => showTopicSheet(
              context,
              peer: FakePeers.alice,
              onOpenThread: (String _) {},
            ),
            child: const Text('open'),
          ),
        ),
        repo: repo,
      ));
      await tester.tap(find.text('open'));
      // Explicit pumps rather than `pumpAndSettle`, and not because of the
      // stuck repo: the skeleton's pulse is an *infinite* repeating
      // animation, so settling never returns while one is on screen (§7.3).
      // The two pumps start the sheet's entrance transition and then run it
      // past its end, which is all this needs before asserting.
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 400));

      expect(find.byKey(const Key('topics-skeleton')), findsOneWidget);
      expect(find.byType(CircularProgressIndicator), findsNothing);
    });
  });
}
