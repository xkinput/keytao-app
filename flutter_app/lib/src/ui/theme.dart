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

  @override
  Widget build(BuildContext context) {
    final touch =
        platform == BridgePlatform.android || platform == BridgePlatform.ios;
    final light = touch
        ? FTheme.neutral.light.touch
        : FTheme.neutral.light.desktop;
    final dark = touch
        ? FTheme.neutral.dark.touch
        : FTheme.neutral.dark.desktop;
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
        fontFamily: FTypeface.defaultFontFamily,
        colorSchemeSeed: light.colors.primary,
      ),
      darkTheme: ThemeData(
        brightness: Brightness.dark,
        fontFamily: FTypeface.defaultFontFamily,
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
