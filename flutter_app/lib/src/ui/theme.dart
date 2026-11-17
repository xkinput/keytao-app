import 'package:flutter/material.dart';
// Forui already depends on this Flutter SDK package. Its delegates localize
// material_ui; Flutter's host and SelectableText need the SDK delegates too.
// ignore: depend_on_referenced_packages
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:forui/forui.dart';

import '../rust/api/types.dart';

@immutable
class AppPalette {
  const AppPalette._({
    required this.ground,
    required this.surface,
    required this.hairline,
    required this.ink,
    required this.muted,
    required this.mutedSecondary,
    required this.unselected,
    required this.success,
    required this.successBackground,
    required this.successForeground,
    required this.error,
    required this.controlSurface,
  });

  final Color ground;
  final Color surface;
  final Color hairline;
  final Color ink;
  final Color muted;
  final Color mutedSecondary;
  final Color unselected;
  final Color success;
  final Color successBackground;
  final Color successForeground;
  final Color error;
  final Color controlSurface;

  static const light = AppPalette._(
    ground: Color(0xFFF3F2EE),
    surface: Color(0xFFFAF9F6),
    hairline: Color(0xFFDEDCD5),
    ink: Color(0xFF17171A),
    muted: Color(0xFF5F5D58),
    mutedSecondary: Color(0xFF6E6C66),
    unselected: Color(0xFF8A8780),
    success: Color(0xFF1F8A4C),
    successBackground: Color(0xFFDCEFE3),
    successForeground: Color(0xFF145C33),
    error: Color(0xFFB3412F),
    controlSurface: Color(0xFFE7E5DF),
  );

  static const dark = AppPalette._(
    ground: Color(0xFF121214),
    surface: Color(0xFF1A1A1D),
    hairline: Color(0xFF2C2C30),
    ink: Color(0xFFF2F1EE),
    muted: Color(0xFFA3A19B),
    mutedSecondary: Color(0xFF8D8B85),
    unselected: Color(0xFF6F6D68),
    success: Color(0xFF58B67C),
    successBackground: Color(0xFF163A26),
    successForeground: Color(0xFFA4DFBA),
    error: Color(0xFFE08573),
    controlSurface: Color(0xFF242427),
  );

  static AppPalette of(BuildContext context) =>
      Theme.of(context).brightness == Brightness.dark ? dark : light;
}

class ChromeAccent extends InheritedTheme {
  const ChromeAccent({super.key, required this.color, required super.child});

  final Color color;

  static Color of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<ChromeAccent>()?.color ??
      const Color(0xFF3B73D9);

  @override
  bool updateShouldNotify(ChromeAccent oldWidget) => color != oldWidget.color;

  @override
  Widget wrap(BuildContext context, Widget child) =>
      ChromeAccent(color: color, child: child);
}

class KeyTaoTheme extends StatelessWidget {
  const KeyTaoTheme({super.key, required this.platform, required this.home});
  final BridgePlatform platform;
  final Widget home;

  static const _windowsFontFamily = 'Microsoft YaHei UI';
  static const _windowsFontFallback = [
    'Microsoft YaHei',
    'Segoe UI',
    'Segoe UI Emoji',
    'Segoe UI Symbol',
  ];

  FThemeData _platformTheme(
    AppPalette palette, {
    required Brightness brightness,
    required bool touch,
  }) {
    final colors =
        (brightness == Brightness.dark
                ? FColors.neutralDark
                : FColors.neutralLight)
            .copyWith(
              background: palette.ground,
              foreground: palette.ink,
              primary: palette.ink,
              primaryForeground: palette.ground,
              secondary: palette.controlSurface,
              secondaryForeground: palette.ink,
              muted: palette.controlSurface,
              mutedForeground: palette.muted,
              destructive: palette.error,
              error: palette.error,
              card: palette.surface,
              border: palette.hairline,
            );
    final typeface = FTypeface.inherit(
      colors: colors,
      touch: touch,
      fontFamily: platform == BridgePlatform.windows
          ? _windowsFontFamily
          : FTypeface.defaultFontFamily,
      fontFamilyFallback: platform == BridgePlatform.windows
          ? _windowsFontFallback
          : null,
    );
    // Rebuild component styles too: they capture the typeface at creation.
    return FThemeData(
      colors: colors,
      touch: touch,
      typography: FTypography(display: typeface, body: typeface),
    );
  }

  ThemeData _materialTheme(AppPalette palette, Brightness brightness) =>
      ThemeData(
        brightness: brightness,
        fontFamily: platform == BridgePlatform.windows
            ? _windowsFontFamily
            : FTypeface.defaultFontFamily,
        fontFamilyFallback: platform == BridgePlatform.windows
            ? _windowsFontFallback
            : null,
        scaffoldBackgroundColor: palette.ground,
        canvasColor: palette.ground,
        dividerColor: palette.hairline,
        disabledColor: palette.unselected,
        colorScheme:
            ColorScheme.fromSeed(
              seedColor: palette.ink,
              brightness: brightness,
            ).copyWith(
              primary: palette.ink,
              onPrimary: palette.ground,
              secondary: palette.controlSurface,
              onSecondary: palette.ink,
              surface: palette.surface,
              onSurface: palette.ink,
              onSurfaceVariant: palette.muted,
              surfaceContainerLowest: palette.ground,
              surfaceContainerLow: palette.surface,
              surfaceContainer: palette.surface,
              surfaceContainerHigh: palette.controlSurface,
              surfaceContainerHighest: palette.controlSurface,
              outline: palette.hairline,
              outlineVariant: palette.hairline,
              error: palette.error,
            ),
      );

  @override
  Widget build(BuildContext context) {
    final touch =
        platform == BridgePlatform.android || platform == BridgePlatform.ios;
    final light = _platformTheme(
      AppPalette.light,
      brightness: Brightness.light,
      touch: touch,
    );
    final dark = _platformTheme(
      AppPalette.dark,
      brightness: Brightness.dark,
      touch: touch,
    );
    return MaterialApp(
      title: 'KeyTao',
      debugShowCheckedModeBanner: false,
      locale: const Locale('zh', 'CN'),
      localizationsDelegates: const [
        ...FLocalizations.localizationsDelegates,
        GlobalMaterialLocalizations.delegate,
        GlobalCupertinoLocalizations.delegate,
      ],
      supportedLocales: FLocalizations.supportedLocales,
      theme: _materialTheme(AppPalette.light, Brightness.light),
      darkTheme: _materialTheme(AppPalette.dark, Brightness.dark),
      themeMode: ThemeMode.system,
      builder: (context, child) => FTheme(
        data: Theme.of(context).brightness == Brightness.dark ? dark : light,
        platform: switch (platform) {
          BridgePlatform.macOs => FPlatformVariant.macOS,
          BridgePlatform.android => FPlatformVariant.android,
          BridgePlatform.ios => FPlatformVariant.iOS,
          BridgePlatform.windows => FPlatformVariant.windows,
          _ => FPlatformVariant.linux,
        },
        child: FTooltipGroup(child: child!),
      ),
      home: home,
    );
  }
}
