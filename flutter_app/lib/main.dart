import 'package:flutter/material.dart';

import 'src/app/app.dart';
import 'src/app/bootstrap.dart';
import 'src/app/controller.dart';
import 'src/app/theme.dart';

export 'src/app/bootstrap.dart' show hostPlatform;

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  runApp(
    const MaterialApp(
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
      MaterialApp(
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
