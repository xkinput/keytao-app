import 'dart:math' as math;

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:keytao/src/ui/theme.dart';

double contrastRatio(Color foreground, Color background) {
  final first = foreground.computeLuminance();
  final second = background.computeLuminance();
  return (math.max(first, second) + .05) / (math.min(first, second) + .05);
}

void main() {
  test('accent tint keeps app text at WCAG AA contrast', () {
    for (final brightness in Brightness.values) {
      final base = brightness == Brightness.dark
          ? AppPalette.dark
          : AppPalette.light;
      for (final accent in const [
        Color(0xFF000000),
        Color(0xFFFFFFFF),
        ChromeAccent.defaultColor,
        Color(0xFFFFC301),
        Color(0xFF0F9F8F),
      ]) {
        final palette = AppPalette.forAccent(brightness, accent);
        expect(
          contrastRatio(ChromeAccent.foreground(accent), accent),
          greaterThanOrEqualTo(4.5),
        );
        expect(palette.ink, base.ink);
        expect(palette.muted, base.muted);
        expect(palette.mutedSecondary, base.mutedSecondary);
        expect(palette.success, base.success);
        expect(palette.successForeground, base.successForeground);
        expect(palette.error, base.error);
        for (final (background, neutral) in [
          (palette.ground, base.ground),
          (palette.surface, base.surface),
          (palette.controlSurface, base.controlSurface),
          (palette.hairline, base.hairline),
        ]) {
          expect(background, isNot(neutral));
          for (final foreground in [palette.ink, palette.muted]) {
            expect(
              contrastRatio(foreground, background),
              greaterThanOrEqualTo(4.5),
              reason: '$brightness with $accent, $foreground on $background',
            );
          }
        }
      }
    }
  });
}
