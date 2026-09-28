import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'widgets.dart';

class AppMenus extends StatelessWidget {
  const AppMenus({
    super.key,
    required this.child,
    required this.platform,
    this.controller,
  });
  final Widget child;
  final BridgePlatform platform;
  final AppController? controller;
  @override
  Widget build(BuildContext context) {
    if (platform != BridgePlatform.macOs) {
      if (platform == BridgePlatform.android ||
          platform == BridgePlatform.ios) {
        return child;
      }
      return CallbackShortcuts(
        bindings: {
          for (final page in AppPage.values)
            CharacterActivator('${page.shortcutDigit}', control: true): () =>
                controller?.selectPage(page),
        },
        child: Focus(autofocus: true, child: child),
      );
    }
    return PlatformMenuBar(
      menus: [
        PlatformMenu(
          label: 'KeyTao',
          menus: [
            for (final type in const [
              PlatformProvidedMenuItemType.about,
              PlatformProvidedMenuItemType.hide,
              PlatformProvidedMenuItemType.quit,
            ])
              if (PlatformProvidedMenuItem.hasMenu(type))
                PlatformProvidedMenuItem(type: type),
          ],
        ),
        const PlatformMenu(
          label: '编辑',
          menus: [
            PlatformMenuItem(
              label: '拷贝',
              shortcut: SingleActivator(LogicalKeyboardKey.keyC, meta: true),
              onSelectedIntent: CopySelectionTextIntent.copy,
            ),
            PlatformMenuItem(
              label: '粘贴',
              shortcut: SingleActivator(LogicalKeyboardKey.keyV, meta: true),
              onSelectedIntent: PasteTextIntent(SelectionChangedCause.toolbar),
            ),
            PlatformMenuItem(
              label: '全选',
              shortcut: SingleActivator(LogicalKeyboardKey.keyA, meta: true),
              onSelectedIntent: SelectAllTextIntent(
                SelectionChangedCause.toolbar,
              ),
            ),
          ],
        ),
        PlatformMenu(
          label: '显示',
          menus: [
            for (final page in AppPage.values)
              PlatformMenuItem(
                label: page.title,
                shortcut: CharacterActivator(
                  '${page.shortcutDigit}',
                  meta: true,
                ),
                onSelected: controller == null
                    ? null
                    : () => controller!.selectPage(page),
              ),
          ],
        ),
        PlatformMenu(
          label: '窗口',
          menus: [
            for (final type in const [
              PlatformProvidedMenuItemType.minimizeWindow,
              PlatformProvidedMenuItemType.zoomWindow,
              PlatformProvidedMenuItemType.arrangeWindowsInFront,
            ])
              if (PlatformProvidedMenuItem.hasMenu(type))
                PlatformProvidedMenuItem(type: type),
          ],
        ),
      ],
      child: child,
    );
  }
}

class AppErrorPresenter extends StatefulWidget {
  const AppErrorPresenter({
    super.key,
    required this.controller,
    required this.child,
  });
  final AppController controller;
  final Widget child;
  @override
  State<AppErrorPresenter> createState() => _AppErrorPresenterState();
}

class _AppErrorPresenterState extends State<AppErrorPresenter> {
  String? _observed;
  String? _pending;
  bool _showing = false;
  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_errorChanged);
    _errorChanged();
  }

  void _errorChanged() {
    if (widget.controller.info.platform != BridgePlatform.macOs) return;
    final error = widget.controller.unhandledError;
    if (_observed == error) return;
    _observed = error;
    _pending = error;
    if (error != null) _present();
  }

  void _present() {
    if (_showing) return;
    _showing = true;
    WidgetsBinding.instance.addPostFrameCallback((_) async {
      if (!mounted) return;
      final error = _pending;
      _pending = null;
      if (error != null) {
        await showFDialog<void>(
          context: context,
          builder: (context, _, animation) => AppDialog(
            title: '操作失败',
            animation: animation,
            actions: [
              ActionButton(
                '好',
                primary: true,
                onPress: () => Navigator.pop(context),
              ),
            ],
            child: SelectableText(error),
          ),
        );
      }
      _showing = false;
      if (mounted && _pending != null) _present();
    });
  }

  @override
  void dispose() {
    widget.controller.removeListener(_errorChanged);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
