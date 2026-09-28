import 'package:flutter/material.dart';
// Forui already depends on this Flutter SDK package. Its delegates localize
// material_ui; Flutter's host and SelectableText need the SDK delegates too.
// ignore: depend_on_referenced_packages
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:forui/forui.dart';

import '../rust/api/types.dart';

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

  FThemeData _platformTheme(FThemeData base) {
    if (platform != BridgePlatform.windows) return base;
    final typeface = FTypeface.inherit(
      colors: base.colors,
      touch: false,
      fontFamily: _windowsFontFamily,
      fontFamilyFallback: _windowsFontFallback,
    );
    // Rebuild component styles too: they capture the typeface at creation.
    return FThemeData(
      colors: base.colors,
      touch: false,
      typography: FTypography(display: typeface, body: typeface),
    );
  }

  @override
  Widget build(BuildContext context) {
    final touch =
        platform == BridgePlatform.android || platform == BridgePlatform.ios;
    final light = _platformTheme(
      touch ? FTheme.neutral.light.touch : FTheme.neutral.light.desktop,
    );
    final dark = _platformTheme(
      touch ? FTheme.neutral.dark.touch : FTheme.neutral.dark.desktop,
    );
    final fontFamily = platform == BridgePlatform.windows
        ? _windowsFontFamily
        : FTypeface.defaultFontFamily;
    final fontFallback = platform == BridgePlatform.windows
        ? _windowsFontFallback
        : null;
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
      theme: ThemeData(
        brightness: Brightness.light,
        fontFamily: fontFamily,
        fontFamilyFallback: fontFallback,
        colorSchemeSeed: light.colors.primary,
      ),
      darkTheme: ThemeData(
        brightness: Brightness.dark,
        fontFamily: fontFamily,
        fontFamilyFallback: fontFallback,
        colorSchemeSeed: dark.colors.primary,
      ),
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
