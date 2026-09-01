import 'package:comrade/src/util/display_text.dart';
import 'package:comrade/src/util/profile_view.dart';
import 'package:flutter_test/flutter_test.dart';

/// The Dart third of the mirrored profile-page suite. The same cases run in
/// `desktop/ui/profile_view.test.mjs` and
/// `android/.../ui/ProfileViewTest.kt`; a change to one of the three rules has
/// to change all three suites.
///
/// The hostile characters are built from code points rather than pasted, so they
/// are visible in a diff and cannot be silently stripped by an editor.
void main() {
  const npub = 'npub1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq';

  /// U+202E RIGHT-TO-LEFT OVERRIDE — the one that makes gnp.exe read as .png.
  final rlo = String.fromCharCode(0x202E);

  List<ProfileRowKind> kinds(List<ProfileRow> rows) =>
      rows.map((r) => r.kind).toList();

  group('the info block', () {
    test('the key row is present for every profile, however full or empty', () {
      // The trust invariant, mechanically enforced: a self-declared handle must
      // never be shown with the key unreachable.
      final full = infoRows(const ProfileFields(
        npub: npub,
        name: 'bob',
        about: 'hi',
        nip05: 'bob@example.com',
        lud16: 'bob@wallet.example',
      ));
      expect(kinds(full), [
        ProfileRowKind.bio,
        ProfileRowKind.handle,
        ProfileRowKind.nip05,
        ProfileRowKind.lud16,
        ProfileRowKind.key,
      ]);

      expect(kinds(infoRows(const ProfileFields(npub: npub))),
          [ProfileRowKind.key]);
      expect(
          kinds(infoRows(const ProfileFields(npub: ''))), [ProfileRowKind.key]);
    });

    test('the key row is last, because it is the least scannable line', () {
      final rows =
          infoRows(const ProfileFields(npub: npub, name: 'bob', about: 'hi'));
      expect(rows.last.kind, ProfileRowKind.key);
    });

    test('a blank field produces no row rather than an empty one', () {
      // "We know they have no bio" and "we have not fetched a bio" are different
      // facts, and a blank row states the first when only the second is true.
      final rows =
          infoRows(const ProfileFields(npub: npub, about: '   ', name: ''));
      expect(kinds(rows), [ProfileRowKind.key]);
    });

    test(
        'your own empty bio still gets a row, because that is how you fill it in',
        () {
      final mine = infoRows(const ProfileFields(npub: npub), isSelf: true);
      expect(kinds(mine), [ProfileRowKind.bio, ProfileRowKind.key]);
      expect(mine.first.value, '');
      expect(kinds(infoRows(const ProfileFields(npub: npub))),
          [ProfileRowKind.key]);
    });

    test('a bio cannot smuggle control characters onto the page', () {
      final rows = infoRows(ProfileFields(npub: npub, about: 'Trust me$rlo'));
      expect(rows.first.value, 'Trust me');
    });

    test('a bio that is forty lines long is collapsed to one', () {
      final rows = infoRows(ProfileFields(npub: npub, about: 'line\n' * 40));
      expect(rows.first.value, ('line ' * 40).trim());
    });

    test('the key is never truncated, because a shortened key is not a key',
        () {
      expect(infoRows(const ProfileFields(npub: npub)).last.value, npub);
    });

    test('the bio is the one row that is not offered as copyable', () {
      final rows = infoRows(const ProfileFields(
        npub: npub,
        about: 'hi',
        name: 'bob',
        nip05: 'b@e.com',
        lud16: 'b@w.example',
      ));
      final copyable = {for (final r in rows) r.kind: r.copyable};
      expect(copyable, {
        ProfileRowKind.bio: false,
        ProfileRowKind.handle: true,
        ProfileRowKind.nip05: true,
        ProfileRowKind.lud16: true,
        ProfileRowKind.key: true,
      });
    });
  });

  group('the handle', () {
    test('renders with exactly one @, however many the peer published', () {
      expect(handleOf('bob'), '@bob');
      expect(handleOf('@bob'), '@bob');
      expect(handleOf('@@@bob'), '@bob');
      expect(handleOf('  @bob  '), '@bob');
    });

    test('a handle of nothing but @ is no handle at all', () {
      expect(handleOf('@'), '');
      expect(handleOf('@@@'), '');
      expect(handleOf(null), '');
      expect(handleOf(''), '');
    });
  });

  group('shared-media buckets', () {
    test('photos and videos share the grid, voice notes get their own list',
        () {
      expect(mediaTabFor('image/png'), MediaTab.media);
      expect(mediaTabFor('video/mp4'), MediaTab.media);
      expect(mediaTabFor('audio/ogg'), MediaTab.voice);
      expect(mediaTabFor('application/pdf'), MediaTab.files);
    });

    test(
        'no mime type is ever dropped, because a dropped attachment is unreachable',
        () {
      for (final mime in [
        '',
        'garbage',
        '/',
        'image',
        'IMAGE/PNG',
        '  image/png  '
      ]) {
        expect(MediaTab.values, contains(mediaTabFor(mime)));
      }
      expect(mediaTabFor('IMAGE/PNG'), MediaTab.media);
      expect(mediaTabFor('  image/png  '), MediaTab.media);
    });

    test('a profile opens on a tab that has something in it', () {
      // Opening on an empty Media tab makes a profile with plenty of files read
      // as "nothing shared", which is the failure this exists to prevent.
      expect(initialMediaTab(['application/pdf']), MediaTab.files);
      expect(initialMediaTab(['audio/ogg']), MediaTab.voice);
      expect(initialMediaTab(['image/png', 'audio/ogg']), MediaTab.media);
      expect(initialMediaTab([]), MediaTab.media);
    });
  });

  group('the collapsing header', () {
    test('the avatar shrinks from expanded to collapsed across the fraction',
        () {
      expect(collapsedAvatarSize(0, 96, 36), closeTo(96, 0.001));
      expect(collapsedAvatarSize(1, 96, 36), closeTo(36, 0.001));
      expect(collapsedAvatarSize(0.5, 96, 36), closeTo(66, 0.001));
    });

    test(
        'an overscroll fraction outside the range cannot invert or oversize it',
        () {
      // All three platforms report a fraction slightly outside 0..1 at some
      // point during an overscroll.
      expect(collapsedAvatarSize(-0.5, 96, 36), closeTo(96, 0.001));
      expect(collapsedAvatarSize(1.5, 96, 36), closeTo(36, 0.001));
      expect(collapsedAvatarSize(double.nan, 96, 36), closeTo(96, 0.001));
    });

    test('the avatar never grows while the header collapses', () {
      var previous = double.infinity;
      for (var i = 0; i <= 100; i++) {
        final size = collapsedAvatarSize(i / 100, 96, 36);
        expect(size, lessThanOrEqualTo(previous),
            reason:
                'size grew at fraction ${i / 100}, which reads as a stutter');
        previous = size;
      }
    });
  });

  group('the action row', () {
    test('a stranger is never offered a call', () {
      // Placing one makes this device gather ICE for whoever is on the other
      // end — the same bar the accepted-conversation gate holds a signal to.
      final actions = actionRow();
      expect(actions, isNot(contains(ProfileAction.call)));
      expect(actions, [
        ProfileAction.message,
        ProfileAction.addContact,
        ProfileAction.block
      ]);
    });

    test('a contact gets a call, a mute and the comrade toggle', () {
      expect(actionRow(isContact: true), [
        ProfileAction.message,
        ProfileAction.call,
        ProfileAction.mute,
        ProfileAction.addComrade,
        ProfileAction.block,
      ]);
      expect(actionRow(isContact: true, isMuted: true, isComrade: true), [
        ProfileAction.message,
        ProfileAction.call,
        ProfileAction.unmute,
        ProfileAction.removeComrade,
        ProfileAction.block,
      ]);
    });

    test('a blocked peer is offered nothing, because there is no unblock', () {
      // `STATE_BLOCKED` is written in the runtime and read only by the incoming
      // gate; there is no unblock command and no getter to drive one. A button
      // here would be the fake switch the settings screen's own rule forbids.
      // When an unblock command exists, this is the test that says so by failing.
      expect(actionRow(isBlocked: true), isEmpty);
      expect(actionRow(isBlocked: true, isContact: true, isComrade: true),
          isEmpty);
    });

    test('your own profile offers neither a message nor a block', () {
      final mine = actionRow(isSelf: true);
      expect(mine, [ProfileAction.edit, ProfileAction.copyKey]);
      expect(mine, isNot(contains(ProfileAction.message)));
      expect(mine, isNot(contains(ProfileAction.block)));
    });

    test('self outranks every other flag, so no block leaks onto your own page',
        () {
      expect(
        actionRow(
          isSelf: true,
          isContact: true,
          isComrade: true,
          isMuted: true,
          isBlocked: true,
        ),
        [ProfileAction.edit, ProfileAction.copyKey],
      );
    });
  });

  group('publishing your own picture', () {
    test(
        'an avatar has to be one of the three formats the core will fetch back',
        () {
      expect(avatarRejection('image/png', 1024), isNull);
      expect(avatarRejection('image/jpeg', 1024), isNull);
      expect(avatarRejection('image/webp', 1024), isNull);
      expect(avatarRejection('image/svg+xml', 1024), AvatarRefusal.wrongType);
      expect(avatarRejection('image/gif', 1024), AvatarRefusal.wrongType);
      expect(avatarRejection('application/zip', 1024), AvatarRefusal.wrongType);
    });

    test('SVG is refused, because it is a document that can fetch and script',
        () {
      expect(avatarMimeAllowlist, isNot(contains('image/svg+xml')));
    });

    test('an empty image is refused before its type is even considered', () {
      expect(avatarRejection('image/png', 0), AvatarRefusal.empty);
      expect(avatarRejection('image/png', -1), AvatarRefusal.empty);
    });

    test('an oversize image is refused and one exactly at the cap is not', () {
      expect(avatarRejection('image/png', maxAvatarBytes + 1),
          AvatarRefusal.tooLarge);
      expect(avatarRejection('image/png', maxAvatarBytes), isNull);
    });

    test('the type is read case- and whitespace-insensitively', () {
      expect(avatarRejection('  IMAGE/PNG  ', 1024), isNull);
    });
  });

  group('whether to fetch a peer picture at all', () {
    test("a stranger's picture URL is never fetched", () {
      // Opening a stranger's profile must not make this device call out to a
      // host they chose.
      expect(mayFetchAvatar('https://x.example/a.png'), isFalse);
    });

    test('turning remote pictures off stops every fetch, contact or not', () {
      expect(
        mayFetchAvatar('https://x.example/a.png',
            remoteAvatarsEnabled: false, isContact: true),
        isFalse,
      );
      expect(
        mayFetchAvatar('https://x.example/a.png',
            remoteAvatarsEnabled: false, isSelf: true),
        isFalse,
      );
    });

    test("a blocked peer's picture is not fetched even if still a contact", () {
      expect(
        mayFetchAvatar('https://x.example/a.png',
            isContact: true, isBlocked: true),
        isFalse,
      );
    });

    test('a contact with a picture is fetched, and so is your own', () {
      expect(
          mayFetchAvatar('https://x.example/a.png', isContact: true), isTrue);
      expect(mayFetchAvatar('https://x.example/a.png', isSelf: true), isTrue);
    });

    test('no URL means nothing to fetch', () {
      expect(mayFetchAvatar('', isContact: true), isFalse);
      expect(mayFetchAvatar('   ', isContact: true), isFalse);
      expect(mayFetchAvatar(null, isContact: true), isFalse);
    });
  });

  group('the shared text sanitiser', () {
    test(
        'a stripped control becomes a space, so two words never silently merge',
        () {
      // The failure this pins: \n is itself a control, so deleting outright
      // turns a two-line name into one word nobody is called.
      expect(sanitizeDisplayText('Bob\nSmith', 0), 'Bob Smith');
      expect(sanitizeDisplayText('holiday${rlo}gnp.exe', 0), 'holiday gnp.exe');
    });

    test('an ideographic space is whitespace, the way the other two ports have it',
        () {
      // Dart's `\s` already matches it, as JavaScript's does; Kotlin's did not
      // until 2026-09-01, and the mirrored suites had no case that said so.
      final ideographic = String.fromCharCode(0x3000);
      expect(sanitizeDisplayText('a${ideographic}b', 0), 'a b');
      for (final space in [
        0x00A0, 0x1680, 0x2000, 0x200A, 0x2028, 0x2029, 0x202F, 0x205F, 0xFEFF,
      ]) {
        expect(sanitizeDisplayText('a${String.fromCharCode(space)}b', 0), 'a b');
      }
    });

    test('text longer than the bound is truncated with an ellipsis inside it',
        () {
      final out = sanitizeDisplayText('abcdefghij', 5);
      expect(out, 'abcd…');
      expect(out.length, 5);
    });

    test('a bound of zero means no bound rather than an empty string', () {
      expect(sanitizeDisplayText('abcdefghij', 0), 'abcdefghij');
      expect(sanitizeDisplayText('abcde', 5), 'abcde');
    });

    test('nothing usable becomes the empty string, never invented wording', () {
      for (final input in [
        null,
        '',
        '   ',
        ' $rlo',
        '\n\n',
        String.fromCharCode(0x200E),
      ]) {
        expect(sanitizeDisplayText(input, 32), '');
      }
    });
  });
}
