/// Port of
/// `android/app/src/test/java/mullu/comrade/ui/LinkPreviewDecisionsTest.kt`.
///
/// Pins the link-preview decision vectors — a hostile `site_name` cannot
/// relabel the domain (there is no such parameter to begin with), and a
/// bare-link message still gets a card.
library;

import 'package:comrade/src/util/link_preview.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('whether a message gets a card', () {
    test('a message with no URL gets no card', () {
      expect(hasLinkPreview('just saying hi'), isFalse);
      expect(linkPreviewFor('just saying hi'), isNull);
    });

    test('a message that is only the URL still gets a card', () {
      // The single most common way a link is shared — suppressing it here
      // would remove the preview for exactly the case it exists to serve.
      expect(hasLinkPreview('https://example.com/offer'), isTrue);
      expect(
        linkPreviewFor('https://example.com/offer')?.domain,
        'example.com',
      );
    });

    test('a message with text and a URL gets a card', () {
      expect(
        hasLinkPreview('check this out: https://example.com/offer'),
        isTrue,
      );
    });

    test('only the first URL becomes the card', () {
      const String text =
          'https://first.example/a and also https://second.example/b';
      expect(firstUrl(text), 'https://first.example/a');
      expect(linkPreviewFor(text)?.domain, 'first.example');
    });

    test('trailing sentence punctuation is not part of the URL', () {
      expect(
        firstUrl('Look: https://example.com/foo.'),
        'https://example.com/foo',
      );
      expect(
        firstUrl('Have you seen (https://example.com/foo)?'),
        'https://example.com/foo',
      );
    });
  });

  group('the domain the card shows', () {
    test(
        'domain comes from the host, not from a site_name, since '
        'linkPreviewFor accepts none', () {
      // There is no site_name parameter anywhere in this file — see the
      // header. This test pins the one thing that would matter if there
      // were: a phishing host must not be able to relabel itself.
      expect(
        displayDomain('https://paypal-secure.evil.example/login'),
        'paypal-secure.evil.example',
      );
    });

    test('the real host wins over a userinfo lookalike', () {
      // A classic phishing shape: the legitimate-looking name sits before
      // the "@" as userinfo, and the host a client actually connects to —
      // the one this card must name — is what follows it.
      expect(
        displayDomain('https://example.com@evil.example/path'),
        'evil.example',
      );
    });

    test('www prefix is stripped and case is normalised', () {
      expect(displayDomain('https://WWW.Example.com/path'), 'example.com');
    });

    test('a URL with no host gets no domain', () {
      expect(displayDomain('https:///no-host'), isNull);
    });

    test('an unparsable URL gets no domain rather than a guess', () {
      expect(displayDomain('https://[bad'), isNull);
    });
  });
}
