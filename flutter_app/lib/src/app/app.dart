import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:forui/forui.dart';

import '../rust/api/types.dart';
import '../ui/color_picker.dart';
import '../ui/menus.dart';
import '../ui/onboarding.dart';
import '../ui/package_schema_selection.dart';
import '../ui/shell.dart';
import '../ui/theme.dart';
import '../ui/widgets.dart';
import 'controller.dart';
import 'options.dart';

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
  late String? _accent = _selectedAccent;
  late final AppLifecycleListener _lifecycle;

  String? get _selectedAccent => widget.controller.uiSettings?.accentColor;

  void _accentChanged() {
    final accent = _selectedAccent;
    if (accent != _accent) setState(() => _accent = accent);
  }

  @override
  void initState() {
    super.initState();
    final c = widget.controller;
    if (c.isWindows &&
        c.local?.installed == false &&
        c.activePage == AppPage.input) {
      c.selectPage(AppPage.scheme);
    }
    c.addListener(_accentChanged);
    _lifecycle = AppLifecycleListener(
      onResume: widget.controller.onResume,
      onHide: widget.controller.onPause,
      onPause: widget.controller.onPause,
      onDetach: widget.controller.onPause,
    );
  }

  @override
  void didUpdateWidget(covariant KeyTaoApp oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.controller != oldWidget.controller) {
      oldWidget.controller.removeListener(_accentChanged);
      widget.controller.addListener(_accentChanged);
      _accent = _selectedAccent;
    }
  }

  @override
  void dispose() {
    widget.controller.removeListener(_accentChanged);
    _lifecycle.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => ValueListenableBuilder<bool>(
    valueListenable: widget.controller.reduceMotionChanges,
    builder: (context, reduceMotion, child) {
      final media = MediaQuery.of(context);
      return MediaQuery(
        data: media.copyWith(
          disableAnimations: media.disableAnimations || reduceMotion,
        ),
        child: child!,
      );
    },
    child: KeyTaoTheme(
      platform: widget.controller.info.platform,
      accent: switch (_accent) {
        final value? => parseAccentColor(value),
        null => ChromeAccent.defaultColor,
      },
      home: AppMenus(
        platform: widget.controller.info.platform,
        controller: widget.controller,
        child: AppErrorPresenter(
          controller: widget.controller,
          child: _MigrationNoticePresenter(
            controller: widget.controller,
            child: Builder(
              builder: (context) => AnimatedSwitcher(
                duration: MediaQuery.disableAnimationsOf(context)
                    ? Duration.zero
                    : const Duration(milliseconds: 240),
                child: _completed || !widget.controller.hasOnboarding
                    ? AppShell(controller: widget.controller)
                    : OnboardingPage(
                        controller: widget.controller,
                        onFinished: () {
                          final c = widget.controller;
                          if (c.isMobile) c.selectPage(AppPage.scheme);
                          setState(() => _completed = true);
                        },
                      ),
              ),
            ),
          ),
        ),
      ),
    ),
  );
}

class _MigrationNoticePresenter extends StatefulWidget {
  const _MigrationNoticePresenter({
    required this.controller,
    required this.child,
  });

  final AppController controller;
  final Widget child;

  @override
  State<_MigrationNoticePresenter> createState() =>
      _MigrationNoticePresenterState();
}

class _MigrationNoticePresenterState extends State<_MigrationNoticePresenter> {
  bool _dialogOpen = false;

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_onChanged);
    _onChanged();
  }

  @override
  void didUpdateWidget(covariant _MigrationNoticePresenter oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      oldWidget.controller.removeListener(_onChanged);
      widget.controller.addListener(_onChanged);
      _onChanged();
    }
  }

  @override
  void dispose() {
    widget.controller.removeListener(_onChanged);
    super.dispose();
  }

  void _onChanged() {
    final c = widget.controller;
    if (!c.hasPendingMigrationFailureToast &&
        !c.hasPendingToast &&
        !c.hasPendingSchemaSelectionPrompt &&
        c.migrationBackupDir == null) {
      return;
    }
    WidgetsBinding.instance.addPostFrameCallback((_) async {
      if (!mounted) return;
      if (c.takeMigrationFailureToast() case final message?) {
        final toaster = FToaster.of(context);
        if (toaster.mounted) {
          toaster.show(
            duration: const Duration(seconds: 5),
            builder: (_, _) => FToast(title: Text(message)),
          );
        }
      }
      if (c.takePendingToast() case final message?) {
        final toaster = FToaster.of(context);
        if (toaster.mounted) {
          toaster.show(
            duration: const Duration(seconds: 4),
            builder: (_, _) => FToast(title: Text(message)),
          );
        }
      }
      if (c.takePendingSchemaSelectionPrompt() case final prompt?) {
        await showPackageSchemaSelection(
          context,
          c,
          prompt.packageKey,
          activateOnConfirm: prompt.activateOnConfirm,
          convert: prompt.convert,
        );
        if (!mounted) return;
      }
      final backupDir = c.migrationBackupDir;
      if (backupDir == null || _dialogOpen) return;
      _dialogOpen = true;
      try {
        await showFDialog<void>(
          context: context,
          barrierDismissible: false,
          builder: (dialogContext, _, animation) => AppDialog(
            title: '方案目录已更新',
            animation: animation,
            actions: [
              ActionButton(
                AppStrings.close,
                onPress: () async {
                  Navigator.pop(dialogContext);
                  await c.dismissMigrationBackupNotice();
                },
              ),
              ActionButton(
                '复制文件目录',
                icon: FLucideIcons.copy,
                primary: c.isMobile,
                onPress: () async {
                  await Clipboard.setData(ClipboardData(text: backupDir));
                  if (mounted) {
                    final toaster = FToaster.of(context);
                    if (toaster.mounted) {
                      toaster.show(
                        duration: const Duration(seconds: 3),
                        builder: (_, _) =>
                            const FToast(title: Text('已复制备份文件目录')),
                      );
                    }
                  }
                },
              ),
              if (!c.isMobile)
                ActionButton(
                  '打开备份文件目录',
                  icon: FLucideIcons.folderOpen,
                  primary: true,
                  onPress: () async {
                    final opened = await c.open(backupDir);
                    if (!opened) {
                      await Clipboard.setData(ClipboardData(text: backupDir));
                      if (mounted) {
                        final toaster = FToaster.of(context);
                        if (toaster.mounted) {
                          toaster.show(
                            duration: const Duration(seconds: 5),
                            builder: (_, _) =>
                                const FToast(title: Text('无法直接打开，已复制备份文件目录')),
                          );
                        }
                      }
                    }
                    if (dialogContext.mounted) Navigator.pop(dialogContext);
                    await c.dismissMigrationBackupNotice();
                  },
                ),
            ],
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              mainAxisSize: MainAxisSize.min,
              spacing: 12,
              children: [
                const Text('方案目录已升级为独立隔离结构，原有方案已清空并备份至以下目录，可前往该目录找回备份文件：'),
                DecoratedBox(
                  decoration: BoxDecoration(
                    color: AppPalette.of(dialogContext).controlSurface,
                    borderRadius: BorderRadius.circular(8),
                  ),
                  child: Padding(
                    padding: const EdgeInsets.all(10),
                    child: SelectableText(
                      backupDir,
                      style: dialogContext.theme.typography.body.xs.copyWith(
                        fontFamily: 'monospace',
                      ),
                    ),
                  ),
                ),
              ],
            ),
          ),
        );
      } finally {
        _dialogOpen = false;
      }
    });
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
