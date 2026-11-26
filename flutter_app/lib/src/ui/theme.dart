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
    ground: Color(0xFFF4F5F7),
    surface: Color(0xFFFBFBFC),
    hairline: Color(0xFFDCDEE2),
    ink: Color(0xFF17181B),
    muted: Color(0xFF5A5C63),
    mutedSecondary: Color(0xFF6A6C73),
    unselected: Color(0xFF85878E),
    success: Color(0xFF1F8A4C),
    successBackground: Color(0xFFDCEFE3),
    successForeground: Color(0xFF145C33),
    error: Color(0xFFB3412F),
    controlSurface: Color(0xFFE8E9ED),
  );

  static const dark = AppPalette._(
    ground: Color(0xFF111214),
    surface: Color(0xFF19191C),
    hairline: Color(0xFF2B2C30),
    ink: Color(0xFFF1F2F4),
    muted: Color(0xFFA2A4AA),
    mutedSecondary: Color(0xFF8C8E94),
    unselected: Color(0xFF6E7076),
    success: Color(0xFF58B67C),
    successBackground: Color(0xFF163A26),
    successForeground: Color(0xFFA4DFBA),
    error: Color(0xFFE08573),
    controlSurface: Color(0xFF232428),
  );

  static AppPalette of(BuildContext context) =>
      forAccent(Theme.of(context).brightness, ChromeAccent.of(context));

  static AppPalette forAccent(Brightness brightness, Color accent) {
    final night = brightness == Brightness.dark;
    final base = night ? dark : light;
    Color tint(Color color, double amount) =>
        Color.alphaBlend(accent.withValues(alpha: accent.a * amount), color);

    return AppPalette._(
      ground: tint(base.ground, .07),
      surface: tint(base.surface, night ? .03 : .02),
      hairline: tint(base.hairline, .04),
      ink: base.ink,
      muted: base.muted,
      mutedSecondary: base.mutedSecondary,
      unselected: base.unselected,
      success: base.success,
      successBackground: base.successBackground,
      successForeground: base.successForeground,
      error: base.error,
      controlSurface: tint(base.controlSurface, night ? .04 : .05),
    );
  }
}

class ChromeAccent extends InheritedTheme {
  const ChromeAccent({super.key, required this.color, required super.child});

  final Color color;

  static const defaultColor = Color(0xFF3B73D9);

  static Color foreground(Color accent) =>
      1.05 / (accent.computeLuminance() + .05) >= 4.5
      ? Colors.white
      : Colors.black;

  static Color of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<ChromeAccent>()?.color ??
      defaultColor;

  @override
  bool updateShouldNotify(ChromeAccent oldWidget) => color != oldWidget.color;

  @override
  Widget wrap(BuildContext context, Widget child) =>
      ChromeAccent(color: color, child: child);
}

class KeyTaoTheme extends StatelessWidget {
  const KeyTaoTheme({
    super.key,
    required this.platform,
    required this.home,
    this.accent = ChromeAccent.defaultColor,
  });
  final BridgePlatform platform;
  final Widget home;
  final Color accent;

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
    final defaults = FTypeface.inherit(
      colors: colors,
      touch: touch,
      fontFamily: platform == BridgePlatform.windows
          ? _windowsFontFamily
          : FTypeface.defaultFontFamily,
      fontFamilyFallback: platform == BridgePlatform.windows
          ? _windowsFontFallback
          : null,
    );
    final typeface = defaults
        .scale(sizeScalar: .875)
        .copyWith(
          sm: defaults.sm.copyWith(fontSize: touch ? 14 : 13),
          xs: defaults.xs.copyWith(fontSize: 12),
        );
    final typography = FTypography(display: typeface, body: typeface);
    // Rebuild component styles too: they capture the typeface at creation.
    return FThemeData(
      colors: colors,
      touch: touch,
      typography: typography,
      style:
          FStyle.inherit(
            colors: colors,
            typography: typography,
            touch: touch,
          ).copyWith(
            iconStyle: const IconThemeDataDelta.delta(size: 20),
            pagePadding: const EdgeInsetsDelta.value(
              EdgeInsets.symmetric(horizontal: 9, vertical: 6),
            ),
          ),
    );
  }

  ThemeData _materialTheme(AppPalette palette, Brightness brightness) {
    final touch =
        platform == BridgePlatform.android || platform == BridgePlatform.ios;
    final buttonStyle = ButtonStyle(
      minimumSize: WidgetStatePropertyAll(Size(40, touch ? 44 : 36)),
      padding: const WidgetStatePropertyAll(
        EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      ),
    );
    final theme = ThemeData(
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
    final textTheme = theme.typography.englishLike.merge(theme.textTheme);
    return theme.copyWith(
      textTheme: textTheme
          .apply(fontSizeFactor: .875)
          .copyWith(
            bodyMedium: textTheme.bodyMedium!.copyWith(fontSize: 13),
            bodySmall: textTheme.bodySmall!.copyWith(fontSize: 12),
            titleMedium: textTheme.titleMedium!.copyWith(fontSize: 15),
          ),
      iconTheme: const IconThemeData(size: 20),
      visualDensity: touch ? VisualDensity.standard : VisualDensity.compact,
      materialTapTargetSize: touch
          ? MaterialTapTargetSize.padded
          : MaterialTapTargetSize.shrinkWrap,
      listTileTheme: ListTileThemeData(
        dense: true,
        minTileHeight: touch ? 44 : 36,
        minVerticalPadding: 4,
        minLeadingWidth: 24,
        horizontalTitleGap: 12,
        contentPadding: const EdgeInsets.symmetric(horizontal: 12),
        iconColor: palette.muted,
      ),
      cardTheme: const CardThemeData(margin: EdgeInsets.all(6)),
      filledButtonTheme: FilledButtonThemeData(style: buttonStyle),
      outlinedButtonTheme: OutlinedButtonThemeData(style: buttonStyle),
      textButtonTheme: TextButtonThemeData(style: buttonStyle),
      elevatedButtonTheme: ElevatedButtonThemeData(style: buttonStyle),
      iconButtonTheme: IconButtonThemeData(style: buttonStyle),
      inputDecorationTheme: InputDecorationThemeData(
        isDense: true,
        contentPadding: const EdgeInsets.symmetric(
          horizontal: 12,
          vertical: 10,
        ),
        constraints: BoxConstraints(minHeight: touch ? 44 : 36),
      ),
      chipTheme: const ChipThemeData(
        padding: EdgeInsets.symmetric(horizontal: 6, vertical: 4),
        labelPadding: EdgeInsets.symmetric(horizontal: 4),
      ),
      navigationRailTheme: const NavigationRailThemeData(
        minWidth: 64,
        minExtendedWidth: 160,
        selectedIconTheme: IconThemeData(size: 20),
        unselectedIconTheme: IconThemeData(size: 20),
      ),
      navigationBarTheme: const NavigationBarThemeData(
        height: 64,
        iconTheme: WidgetStatePropertyAll(IconThemeData(size: 20)),
        labelTextStyle: WidgetStatePropertyAll(TextStyle(fontSize: 12)),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final touch =
        platform == BridgePlatform.android || platform == BridgePlatform.ios;
    final lightPalette = AppPalette.forAccent(Brightness.light, accent);
    final darkPalette = AppPalette.forAccent(Brightness.dark, accent);
    final light = _platformTheme(
      lightPalette,
      brightness: Brightness.light,
      touch: touch,
    );
    final dark = _platformTheme(
      darkPalette,
      brightness: Brightness.dark,
      touch: touch,
    );
    return ChromeAccent(
      color: accent,
      child: MaterialApp(
        title: 'KeyTao',
        debugShowCheckedModeBanner: false,
        locale: const Locale('zh', 'CN'),
        localizationsDelegates: const [
          ...FLocalizations.localizationsDelegates,
          GlobalMaterialLocalizations.delegate,
          GlobalCupertinoLocalizations.delegate,
        ],
        supportedLocales: FLocalizations.supportedLocales,
        theme: _materialTheme(lightPalette, Brightness.light),
        darkTheme: _materialTheme(darkPalette, Brightness.dark),
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
          child: FTooltipGroup(child: FToaster(child: child!)),
        ),
        home: home,
      ),
    );
  }
}
