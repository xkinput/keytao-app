import 'dart:io';

import 'package:flutter/widgets.dart';

import 'src/app/app.dart';
import 'src/app/bootstrap.dart';
import 'src/app/controller.dart';
import 'src/ui/startup.dart';

export 'src/app/bootstrap.dart' show hostPlatform;

Future<void> main(List<String> args) async {
  WidgetsFlutterBinding.ensureInitialized();
  if (await runDesktopCommand(args) case final status?) {
    await Future.wait([stdout.flush(), stderr.flush()]);
    exit(status);
  }
  runApp(AppStartup(platform: hostPlatform()));
  try {
    final session = await bootstrap(args: args);
    runApp(
      KeyTaoApp(
        controller: session.controller,
        initialOnboarding: session.onboarding,
      ),
    );
  } catch (error) {
    runApp(AppStartup(platform: hostPlatform(), error: errorText(error)));
  }
}
