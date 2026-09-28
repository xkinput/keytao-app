import 'dart:io';

import 'package:flutter/material.dart';
import 'package:macos_ui/macos_ui.dart';

import 'src/app/app.dart';
import 'src/app/bootstrap.dart';
import 'src/app/controller.dart';
import 'src/app/theme.dart';
import 'src/macos/app.dart';

export 'src/app/bootstrap.dart' show hostPlatform;

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  if (Platform.isMacOS) {
    await const MacosWindowUtilsConfig().apply();
  }
  runApp(
    Platform.isMacOS
        ? const MacosStartup()
        : const MaterialApp(
            home: Scaffold(body: Center(child: CircularProgressIndicator())),
          ),
  );
  try {
    final session = await bootstrap();
    runApp(
      KeyTaoApp(
        controller: session.controller,
        initialOnboarding: session.onboarding,
      ),
    );
  } catch (error) {
    runApp(
      Platform.isMacOS
          ? MacosStartup(error: errorText(error))
          : MaterialApp(
              theme: keytaoTheme(Brightness.light),
              darkTheme: keytaoTheme(Brightness.dark),
              home: Scaffold(
                appBar: AppBar(title: const Text('KeyTao')),
                body: Center(
                  child: Padding(
                    padding: const EdgeInsets.all(24),
                    child: SelectableText('启动失败：${errorText(error)}'),
                  ),
                ),
              ),
            ),
    );
  }
}
