/// `ListSkeleton` itself (`docs/DESIGN_SYSTEM.md` §7.3): the shared row count,
/// the reduced-motion freeze, and that the two factories draw the row shapes
/// they claim to — an avatar circle for [ListSkeleton.peerRows], a card frame
/// for [ListSkeleton.cardRows].
///
/// Screen-level tests (`journal_screen_test.dart`, `thread_sheet_test.dart`,
/// …) assert that a *screen* swaps its spinner for one of these; this file
/// asserts the widget those screens swap in actually behaves the way §7.3
/// describes, so a future edit to the shared widget cannot break every
/// screen's test silently by keeping each screen's own assertion shallow.
library;

import 'package:comrade/src/theme/comrade_theme.dart';
import 'package:comrade/src/widgets/list_skeleton.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

Widget _probe(Widget child, {bool disableAnimations = false}) => MaterialApp(
      theme: ComradeTheme.dark(),
      home: MediaQuery(
        data: MediaQueryData(disableAnimations: disableAnimations),
        child: Scaffold(body: child),
      ),
    );

void main() {
  testWidgets('peerRows draws the requested number of rows, divided',
      (WidgetTester tester) async {
    await tester.pumpWidget(_probe(
      ListSkeleton.peerRows(key: const Key('probe-skeleton'), rowCount: 4),
    ));
    await tester.pump();

    expect(find.byKey(const Key('probe-skeleton')), findsOneWidget);
    // One circular block per row — the avatar every peer row has.
    expect(
      find.descendant(
        of: find.byKey(const Key('probe-skeleton')),
        matching: find.byWidgetPredicate(
          (Widget w) => w is SkeletonBlock && w.shape == BoxShape.circle,
        ),
      ),
      findsNWidgets(4),
    );
    // Rows minus one separator between them.
    expect(
      find.descendant(
        of: find.byKey(const Key('probe-skeleton')),
        matching: find.byType(Divider),
      ),
      findsNWidgets(3),
    );
  });

  testWidgets('cardRows draws card-framed rows with no divider',
      (WidgetTester tester) async {
    await tester.pumpWidget(_probe(
      ListSkeleton.cardRows(key: const Key('probe-cards'), rowCount: 3),
    ));
    await tester.pump();

    expect(find.byKey(const Key('probe-cards')), findsOneWidget);
    expect(find.byType(Divider), findsNothing);
    // Each card is its own bordered container — three rectangular blocks for
    // the body-text lines, one per card at minimum.
    expect(
      find.descendant(
        of: find.byKey(const Key('probe-cards')),
        matching: find.byWidgetPredicate(
          (Widget w) => w is SkeletonBlock && w.shape == BoxShape.rectangle,
        ),
      ),
      findsWidgets,
    );
  });

  testWidgets('reduced motion freezes the pulse instead of animating it',
      (WidgetTester tester) async {
    await tester.pumpWidget(_probe(
      ListSkeleton.peerRows(key: const Key('reduced-skeleton'), rowCount: 2),
      disableAnimations: true,
    ));
    await tester.pump();
    final Opacity before = tester.widget<Opacity>(
      find.descendant(
        of: find.byKey(const Key('reduced-skeleton')),
        matching: find.byType(Opacity),
      ),
    );

    // Advance well past the pulse's own period; under reduced motion nothing
    // should move.
    await tester.pump(const Duration(milliseconds: 1200));
    final Opacity after = tester.widget<Opacity>(
      find.descendant(
        of: find.byKey(const Key('reduced-skeleton')),
        matching: find.byType(Opacity),
      ),
    );
    expect(after.opacity, before.opacity);
    // Still a layout preview, not a blank frame.
    expect(after.opacity, greaterThan(0));
  });

  testWidgets('without reduced motion the pulse actually moves',
      (WidgetTester tester) async {
    await tester.pumpWidget(_probe(
      ListSkeleton.peerRows(key: const Key('animated-skeleton'), rowCount: 2),
    ));
    await tester.pump();
    final double first = tester
        .widget<Opacity>(
          find.descendant(
            of: find.byKey(const Key('animated-skeleton')),
            matching: find.byType(Opacity),
          ),
        )
        .opacity;

    await tester.pump(const Duration(milliseconds: 550));
    final double mid = tester
        .widget<Opacity>(
          find.descendant(
            of: find.byKey(const Key('animated-skeleton')),
            matching: find.byType(Opacity),
          ),
        )
        .opacity;

    expect(mid, isNot(equals(first)));
  });
}
