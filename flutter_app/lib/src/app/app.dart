import 'package:flutter/material.dart';

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
  Widget build(BuildContext context) => MaterialApp(
    title: 'KeyTao',
    debugShowCheckedModeBanner: false,
    theme: keytaoTheme(Brightness.light),
    darkTheme: keytaoTheme(Brightness.dark),
    themeMode: ThemeMode.system,
    home: ListenableBuilder(
      listenable: widget.controller,
      builder: (context, _) => AnimatedSwitcher(
        duration: MediaQuery.disableAnimationsOf(context)
            ? Duration.zero
            : const Duration(milliseconds: 240),
        child: _completed
            ? AppShell(
                key: const ValueKey('shell'),
                controller: widget.controller,
              )
            : OnboardingPage(
                key: const ValueKey('onboarding'),
                controller: widget.controller,
                onFinished: () => setState(() => _completed = true),
              ),
      ),
    ),
  );
}
