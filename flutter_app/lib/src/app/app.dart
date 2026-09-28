import 'package:dynamic_color/dynamic_color.dart';
import 'package:flutter/material.dart';

import '../macos/app.dart';
import '../macos/onboarding.dart';
import '../macos/shell.dart';
import '../onboarding/onboarding_page.dart';
import '../rust/api/types.dart';
import 'controller.dart';
import 'shell.dart';
import 'theme.dart';

class KeyTaoApp extends StatefulWidget {
  const KeyTaoApp({
    super.key,
    required this.controller,
    required this.initialOnboarding,
  });
  final AppController controller;
  final OnboardingDto initialOnboarding;
  @override
  State<KeyTaoApp> createState() => _KeyTaoAppState();
}

class _KeyTaoAppState extends State<KeyTaoApp> {
  late bool _completed = widget.initialOnboarding.completed;
  late final AppLifecycleListener _lifecycle;

  @override
  void initState() {
    super.initState();
    _lifecycle = AppLifecycleListener(onResume: widget.controller.onResume);
  }

  @override
  void dispose() {
    _lifecycle.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final macos = widget.controller.info.platform == BridgePlatform.macOs;
    final home = ListenableBuilder(
      listenable: widget.controller,
      builder: (context, _) => AnimatedSwitcher(
        duration: MediaQuery.disableAnimationsOf(context)
            ? Duration.zero
            : const Duration(milliseconds: 240),
        child: _completed
            ? macos
                  ? MacosShell(controller: widget.controller)
                  : AppShell(controller: widget.controller)
            : macos
            ? MacosOnboarding(
                controller: widget.controller,
                onFinished: () => setState(() => _completed = true),
              )
            : OnboardingPage(
                controller: widget.controller,
                onFinished: () => setState(() => _completed = true),
              ),
      ),
    );
    if (macos) {
      return KeyTaoMacosApp(
        home: MacosErrorPresenter(controller: widget.controller, child: home),
      );
    }
    if (widget.controller.isAndroid) {
      return DynamicColorBuilder(
        builder: (light, dark) => _materialApp(home, light: light, dark: dark),
      );
    }
    return _materialApp(home);
  }

  Widget _materialApp(Widget home, {ColorScheme? light, ColorScheme? dark}) =>
      MaterialApp(
        title: 'KeyTao',
        debugShowCheckedModeBanner: false,
        theme: keytaoTheme(Brightness.light, colorScheme: light),
        darkTheme: keytaoTheme(Brightness.dark, colorScheme: dark),
        themeMode: ThemeMode.system,
        home: home,
      );
}
