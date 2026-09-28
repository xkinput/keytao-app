import 'package:flutter/widgets.dart';

import '../rust/api/types.dart';
import '../ui/menus.dart';
import '../ui/onboarding.dart';
import '../ui/shell.dart';
import '../ui/theme.dart';
import 'controller.dart';

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
  Widget build(BuildContext context) => KeyTaoTheme(
    platform: widget.controller.info.platform,
    home: AppMenus(
      platform: widget.controller.info.platform,
      controller: widget.controller,
      child: AppErrorPresenter(
        controller: widget.controller,
        child: ListenableBuilder(
          listenable: widget.controller,
          builder: (context, _) => AnimatedSwitcher(
            duration: MediaQuery.disableAnimationsOf(context)
                ? Duration.zero
                : const Duration(milliseconds: 240),
            child: _completed
                ? AppShell(controller: widget.controller)
                : OnboardingPage(
                    controller: widget.controller,
                    onFinished: () => setState(() => _completed = true),
                  ),
          ),
        ),
      ),
    ),
  );
}
