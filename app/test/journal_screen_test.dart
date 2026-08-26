/// How a journal recording — voice or video — draws in a frontend that cannot
/// play one.
///
/// Recording, the dedicated folders that keep a clip out of the gallery, and
/// the orphan sweep are Android's (`docs/JOURNAL.md`). What has to be right
/// here is that an entry made over there is still *legible* over here: a title,
/// a clip line naming which kind it is, and — because sharing sends words and a
/// recording has none — no share control offering a send the core would refuse.
library;

import 'dart:async';

import 'package:comrade/src/data/comrade_repository.dart';
import 'package:comrade/src/data/fake_comrade_repository.dart';
import 'package:comrade/src/data/models.dart';
import 'package:comrade/src/screens/journal_screen.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'helpers.dart';

/// Never resolves — holds the journal list on `loading` for the length of a
/// test, so the skeleton frame can be asserted rather than raced past.
class _StuckJournalRepo extends FakeComradeRepository {
  _StuckJournalRepo() : super(latency: Duration.zero, seed: true);

  @override
  Future<List<JournalEntryInfo>> journal() =>
      Completer<List<JournalEntryInfo>>().future;
}

/// Fails the journal fetch outright, so the error branch and its retry can be
/// exercised without depending on the seeded fixtures.
class _FailingJournalRepo extends FakeComradeRepository {
  _FailingJournalRepo() : super(latency: Duration.zero, seed: true);

  @override
  Future<List<JournalEntryInfo>> journal() =>
      Future<List<JournalEntryInfo>>.error(
        const ComradeException('could not reach the store'),
      );
}

void main() {
  late FakeComradeRepository repo;

  Future<void> openJournal(WidgetTester tester) async {
    // Tall enough for every seeded entry to be built: the list is lazy, and
    // the video entry is the oldest, so a default-sized window would leave it
    // unbuilt and every assertion below would pass or fail for the wrong
    // reason.
    setWindowSize(tester, const Size(800, 2400));
    repo = await unlockedFake();
    await tester.pumpWidget(harness(const JournalScreen(), repo: repo));
    await tester.pumpAndSettle();
  }

  testWidgets('a video entry shows its title and how long it runs',
      (WidgetTester tester) async {
    await openJournal(tester);

    expect(find.text('The walk after the argument'), findsOneWidget);
    expect(find.textContaining('Video entry'), findsOneWidget);
    // Length and size, formatted by the same rules Android uses.
    expect(
      find.textContaining('0:47'),
      findsOneWidget,
      reason: 'the clip length should be on the card',
    );
    expect(find.textContaining('12.3 MB'), findsOneWidget);
    // And it says where the footage actually is, rather than offering a play
    // control this frontend cannot honour.
    expect(
      find.textContaining('on the phone that recorded it'),
      findsNWidgets(2),
      reason: 'both the video and the voice entry say where the file is',
    );
    expect(find.byKey(const Key('journal-recording-line')), findsNWidgets(2));
  });

  testWidgets('a voice entry is named as one, not as a video',
      (WidgetTester tester) async {
    await openJournal(tester);

    expect(find.text('Said it out loud at last'), findsOneWidget);
    // The kind comes off the mime, and getting it wrong is how a voice entry
    // ends up offering a video player it has no picture for.
    expect(find.textContaining('Voice entry'), findsOneWidget);
    expect(find.textContaining('1:12'), findsOneWidget);
    expect(find.textContaining('340 KB'), findsOneWidget);
  });

  testWidgets('an entry with no words offers no way to share it',
      (WidgetTester tester) async {
    await openJournal(tester);

    // Two seeded entries have text; the two recording entries have none. A
    // share control on either would open a picker for a send the core refuses.
    expect(find.byKey(const Key('journal-share')), findsNWidgets(2));
  });

  testWidgets('a typed entry is unaffected — no heading, still shareable',
      (WidgetTester tester) async {
    await openJournal(tester);

    expect(
        find.text('Slept badly, but the morning walk helped.'), findsOneWidget);
    // Titles are opt-in: the entries that never had one draw no heading at
    // all rather than a placeholder.
    expect(find.byKey(const Key('journal-entry-title')), findsNWidgets(2));
  });

  testWidgets('the row shape shows while the journal loads, not a bare spinner',
      (WidgetTester tester) async {
    final _StuckJournalRepo stuck = _StuckJournalRepo();
    await stuck.unlockVault(path: 'test-vault', passphrase: 'passphrase');
    await tester.pumpWidget(harness(const JournalScreen(), repo: stuck));

    // Deliberately no `pumpAndSettle` — `journal()` never resolves, so the
    // loading branch is the only frame there is to look at (§7.3).
    expect(find.byKey(const Key('journal-skeleton')), findsOneWidget);
    expect(find.byType(CircularProgressIndicator), findsNothing);
  });

  testWidgets('a failed journal load says so and offers a retry',
      (WidgetTester tester) async {
    final _FailingJournalRepo failing = _FailingJournalRepo();
    await failing.unlockVault(path: 'test-vault', passphrase: 'passphrase');
    await tester.pumpWidget(harness(const JournalScreen(), repo: failing));
    await tester.pumpAndSettle();

    expect(find.text('Could not load the journal'), findsOneWidget);
    expect(find.textContaining('could not reach the store'), findsOneWidget);
    expect(find.widgetWithText(FilledButton, 'Try again'), findsOneWidget);
  });
}
