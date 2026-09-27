import 'package:flutter/material.dart';

ThemeData keytaoTheme(Brightness brightness) {
  final dark = brightness == Brightness.dark;
  // KeytaoTheme.kt fallback and its shared default-theme.yaml dark palette.
  final colors =
      ColorScheme.fromSeed(
        seedColor: const Color(0xFF3B73D9),
        brightness: brightness,
      ).copyWith(
        primary: Color(dark ? 0xFFB6E3FF : 0xFF3B73D9),
        onPrimary: Color(dark ? 0xFF14233B : 0xFFFFFFFF),
        surface: Color(dark ? 0xFF171A20 : 0xFFF8FAFF),
        onSurface: Color(dark ? 0xFFEEF3F7 : 0xFF1F2933),
        surfaceContainerLow: Color(dark ? 0xFF222A33 : 0xFFFFFFFF),
        primaryContainer: Color(dark ? 0xFF2D4B63 : 0xFFE6F0FF),
        onPrimaryContainer: Color(dark ? 0xFFFFFFFF : 0xFF14233B),
        outlineVariant: Color(dark ? 0xFF3E4652 : 0xFFD8E2F1),
      );
  return ThemeData(
    useMaterial3: true,
    colorScheme: colors,
    scaffoldBackgroundColor: colors.surface,
    appBarTheme: AppBarTheme(
      backgroundColor: colors.surface,
      scrolledUnderElevation: 0,
    ),
    cardTheme: CardThemeData(
      color: colors.surfaceContainerLow,
      elevation: 0,
      margin: EdgeInsets.zero,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(24)),
    ),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      fillColor: colors.surface,
      border: OutlineInputBorder(borderRadius: BorderRadius.circular(16)),
      contentPadding: const EdgeInsets.all(16),
    ),
    dividerTheme: DividerThemeData(color: colors.outlineVariant, space: 32),
  );
}
