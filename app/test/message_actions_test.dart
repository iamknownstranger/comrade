/// Port of
/// `android/app/src/test/java/mullu/comrade/ui/MessageActionsTest.kt`.
///
/// Pins the message-action decision vectors — each vector fails if the rule
/// it names is inverted, matching `chat_menu_test.dart` and
/// `chat_thread_test.dart`.
library;

import 'package:comrade/src/util/message_actions.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  MessageContext ctx({
    bool own = false,
    bool hasText = true,
    bool isMedia = false,
    int ageMs = 0,
    bool pinned = false,
    bool starred = false,
  }) =>
      MessageContext(
        own: own,
        hasText: hasText,
        isMedia: isMedia,
        ageMs: ageMs,
        pinned: pinned,
        starred: starred,
      );

  group('edit', () {
    test('offers on an own text message inside the window', () {
      final List<MessageAction> actions = messageActions(
        ctx(own: true, hasText: true, ageMs: editWindowMs - 1),
      );
      expect(actions, contains(MessageAction.edit));
    });

    test('is gone the instant the window closes', () {
      final List<MessageAction> actions = messageActions(
        ctx(own: true, hasText: true, ageMs: editWindowMs + 1),
      );
      expect(actions, isNot(contains(MessageAction.edit)));
    });

    test('never offers on someone else\'s message', () {
      final List<MessageAction> actions =
          messageActions(ctx(own: false, hasText: true, ageMs: 0));
      expect(actions, isNot(contains(MessageAction.edit)));
    });

    test('never offers on a message with no text', () {
      final List<MessageAction> actions =
          messageActions(ctx(own: true, hasText: false, ageMs: 0));
      expect(actions, isNot(contains(MessageAction.edit)));
    });
  });

  group('deleteForEveryone / deleteForMe', () {
    test('offers on an own message inside its window', () {
      final List<MessageAction> actions =
          messageActions(ctx(own: true, ageMs: deleteForEveryoneWindowMs - 1));
      expect(actions, contains(MessageAction.deleteForEveryone));
    });

    test('is gone once its window closes', () {
      final List<MessageAction> actions =
          messageActions(ctx(own: true, ageMs: deleteForEveryoneWindowMs + 1));
      expect(actions, isNot(contains(MessageAction.deleteForEveryone)));
    });

    test('never offers on someone else\'s message', () {
      final List<MessageAction> actions =
          messageActions(ctx(own: false, ageMs: 0));
      expect(actions, isNot(contains(MessageAction.deleteForEveryone)));
    });

    /// The ordering that must never invert: anything still rewritable is
    /// still retractable. A sender who can edit but not delete rewrites the
    /// message to nothing instead, so closing the delete window first buys
    /// no restraint — it only takes away the clean way to do what they will
    /// do anyway.
    test('retraction outlives rewriting', () {
      expect(deleteForEveryoneWindowMs >= editWindowMs, isTrue);
    });

    test('edit is never offered without deleteForEveryone', () {
      for (final int age in <int>[
        0,
        editWindowMs ~/ 2,
        editWindowMs - 1,
        editWindowMs,
      ]) {
        final List<MessageAction> actions =
            messageActions(ctx(own: true, hasText: true, ageMs: age));
        if (actions.contains(MessageAction.edit)) {
          expect(
            actions,
            contains(MessageAction.deleteForEveryone),
            reason: 'editable at $age ms but not retractable',
          );
        }
      }
    });

    test('deleteForMe is always there regardless of ownership or age', () {
      expect(
        messageActions(ctx(own: false, ageMs: 1 << 40))
            .contains(MessageAction.deleteForMe),
        isTrue,
      );
      expect(
        messageActions(ctx(own: true, ageMs: 1 << 40))
            .contains(MessageAction.deleteForMe),
        isTrue,
      );
    });
  });

  group('report', () {
    test('offers only on incoming messages', () {
      expect(
        messageActions(ctx(own: false)),
        contains(MessageAction.report),
      );
    });

    test('never offers on your own message', () {
      expect(
        messageActions(ctx(own: true)),
        isNot(contains(MessageAction.report)),
      );
    });
  });

  group('saveMedia / copy', () {
    test('saveMedia offers only on media messages', () {
      expect(
        messageActions(ctx(isMedia: true)),
        contains(MessageAction.saveMedia),
      );
      expect(
        messageActions(ctx(isMedia: false)),
        isNot(contains(MessageAction.saveMedia)),
      );
    });

    test('copy offers only when there is text', () {
      expect(
        messageActions(ctx(hasText: true)),
        contains(MessageAction.copy),
      );
      expect(
        messageActions(ctx(hasText: false)),
        isNot(contains(MessageAction.copy)),
      );
    });
  });

  group('pin/unpin, star/unstar', () {
    test('exactly one of pin or unpin appears', () {
      final List<MessageAction> unpinned = messageActions(ctx(pinned: false));
      expect(unpinned, contains(MessageAction.pin));
      expect(unpinned, isNot(contains(MessageAction.unpin)));

      final List<MessageAction> pinned = messageActions(ctx(pinned: true));
      expect(pinned, contains(MessageAction.unpin));
      expect(pinned, isNot(contains(MessageAction.pin)));
    });

    test('exactly one of star or unstar appears', () {
      final List<MessageAction> unstarred =
          messageActions(ctx(starred: false));
      expect(unstarred, contains(MessageAction.star));
      expect(unstarred, isNot(contains(MessageAction.unstar)));

      final List<MessageAction> starred = messageActions(ctx(starred: true));
      expect(starred, contains(MessageAction.unstar));
      expect(starred, isNot(contains(MessageAction.star)));
    });
  });

  group('destructive sorts last', () {
    test('deleteForEveryone is the only destructive entry', () {
      expect(
        MessageAction.values.where((MessageAction a) => a.destructive),
        <MessageAction>[MessageAction.deleteForEveryone],
      );
    });

    test('destructive never has anything after it', () {
      final List<MessageAction> actions = messageActions(
        ctx(own: true, hasText: true, isMedia: true, ageMs: 0),
      );
      final int idx = actions.indexOf(MessageAction.deleteForEveryone);
      expect(idx, greaterThanOrEqualTo(0),
          reason: 'expected deleteForEveryone to be offered');
      expect(idx, actions.length - 1);
    });
  });

  group('order relationship with the existing sheet', () {
    test(
        'reply, replyInThread, assignTopic, copy keep their existing '
        'relative order', () {
      final List<MessageAction> actions = messageActions(ctx(hasText: true));
      final int reply = actions.indexOf(MessageAction.reply);
      final int replyInThread = actions.indexOf(MessageAction.replyInThread);
      final int assignTopic = actions.indexOf(MessageAction.assignTopic);
      final int copy = actions.indexOf(MessageAction.copy);
      expect(reply, lessThan(replyInThread));
      expect(replyInThread, lessThan(assignTopic));
      expect(assignTopic, lessThan(copy));
    });

    test('react is always first', () {
      expect(messageActions(ctx()).first, MessageAction.react);
    });
  });

  group('selection: only what applies to every item', () {
    test('forward is fine on a mixed selection', () {
      final List<MessageContext> items = <MessageContext>[
        ctx(hasText: true, isMedia: false),
        ctx(hasText: false, isMedia: true),
      ];
      expect(selectionActions(items), contains(MessageAction.forward));
    });

    test('editing one of many selected is not offered', () {
      final List<MessageContext> items = <MessageContext>[
        ctx(own: true, hasText: true, ageMs: 0),
        ctx(own: false, hasText: true, ageMs: 0),
      ];
      expect(
        selectionActions(items),
        isNot(contains(MessageAction.edit)),
      );
    });

    test('copy applies if any selected item has text', () {
      final List<MessageContext> items = <MessageContext>[
        ctx(hasText: false, isMedia: true),
        ctx(hasText: true),
      ];
      expect(selectionActions(items), contains(MessageAction.copy));
    });

    test('copy is absent when no selected item has text', () {
      final List<MessageContext> items = <MessageContext>[
        ctx(hasText: false, isMedia: true),
        ctx(hasText: false, isMedia: true),
      ];
      expect(selectionActions(items), isNot(contains(MessageAction.copy)));
    });

    test('saveMedia applies if any selected item is media', () {
      final List<MessageContext> items = <MessageContext>[
        ctx(isMedia: false),
        ctx(isMedia: true),
      ];
      expect(selectionActions(items), contains(MessageAction.saveMedia));
    });

    test('star toggles to unstar only when all are starred', () {
      final List<MessageContext> mixed = <MessageContext>[
        ctx(starred: true),
        ctx(starred: false),
      ];
      expect(selectionActions(mixed), contains(MessageAction.star));
      expect(selectionActions(mixed), isNot(contains(MessageAction.unstar)));

      final List<MessageContext> allStarred = <MessageContext>[
        ctx(starred: true),
        ctx(starred: true),
      ];
      expect(selectionActions(allStarred), contains(MessageAction.unstar));
      expect(
        selectionActions(allStarred),
        isNot(contains(MessageAction.star)),
      );
    });

    test('report requires every item to be incoming', () {
      final List<MessageContext> mixed = <MessageContext>[
        ctx(own: false),
        ctx(own: true),
      ];
      expect(selectionActions(mixed), isNot(contains(MessageAction.report)));

      final List<MessageContext> allIncoming = <MessageContext>[
        ctx(own: false),
        ctx(own: false),
      ];
      expect(selectionActions(allIncoming), contains(MessageAction.report));
    });

    test('deleteForEveryone requires every item to qualify', () {
      final List<MessageContext> mixed = <MessageContext>[
        ctx(own: true, ageMs: 0),
        ctx(own: true, ageMs: deleteForEveryoneWindowMs + 1),
      ];
      expect(
        selectionActions(mixed),
        isNot(contains(MessageAction.deleteForEveryone)),
      );

      final List<MessageContext> allQualify = <MessageContext>[
        ctx(own: true, ageMs: 0),
        ctx(own: true, ageMs: 0),
      ];
      expect(
        selectionActions(allQualify),
        contains(MessageAction.deleteForEveryone),
      );
    });

    test('deleteForMe is always in a selection', () {
      final List<MessageContext> items = <MessageContext>[
        ctx(own: false),
        ctx(own: true),
      ];
      expect(selectionActions(items), contains(MessageAction.deleteForMe));
    });

    test(
        'selection has no reply, replyInThread, assignTopic, edit, select '
        'or messageInfo', () {
      final List<MessageContext> items = <MessageContext>[
        ctx(own: true, hasText: true, ageMs: 0),
      ];
      final List<MessageAction> actions = selectionActions(items);
      expect(actions, isNot(contains(MessageAction.reply)));
      expect(actions, isNot(contains(MessageAction.replyInThread)));
      expect(actions, isNot(contains(MessageAction.assignTopic)));
      expect(actions, isNot(contains(MessageAction.edit)));
      expect(actions, isNot(contains(MessageAction.select)));
      expect(actions, isNot(contains(MessageAction.messageInfo)));
      expect(actions, isNot(contains(MessageAction.react)));
    });
  });

  group('selection cap', () {
    test('can add to selection below the cap', () {
      expect(canAddToSelection(maxSelectionSize - 1), isTrue);
    });

    test('cannot add to selection at the cap', () {
      expect(canAddToSelection(maxSelectionSize), isFalse);
    });

    test('selectionActions is empty past the cap', () {
      final List<MessageContext> tooMany =
          List<MessageContext>.generate(maxSelectionSize + 1, (_) => ctx());
      expect(selectionActions(tooMany), isEmpty);
    });

    test('selectionActions is not empty at the cap', () {
      final List<MessageContext> atCap =
          List<MessageContext>.generate(maxSelectionSize, (_) => ctx());
      expect(selectionActions(atCap), isNotEmpty);
    });
  });
}
