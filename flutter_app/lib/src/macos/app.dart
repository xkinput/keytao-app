import 'package:flutter/cupertino.dart';
import 'package:flutter/material.dart' show SelectableText, ThemeMode;
import 'package:flutter/services.dart';
import 'package:macos_ui/macos_ui.dart';

import '../app/controller.dart';

class KeyTaoMacosApp extends StatelessWidget {
  const KeyTaoMacosApp({super.key, required this.home});
  final Widget home;

  @override
  Widget build(BuildContext context) => StreamBuilder(
    stream: AccentColorListener.instance.onChanged,
    builder: (context, _) => StreamBuilder(
      stream: WindowMainStateListener.instance.onChanged,
      builder: (context, _) => MacosApp(
        title: 'KeyTao',
        debugShowCheckedModeBanner: false,
        themeMode: ThemeMode.system,
        theme: MacosThemeData.light(
          accentColor: AccentColorListener.instance.currentAccentColor,
          isMainWindow: WindowMainStateListener.instance.isMainWindow,
        ),
        darkTheme: MacosThemeData.dark(
          accentColor: AccentColorListener.instance.currentAccentColor,
          isMainWindow: WindowMainStateListener.instance.isMainWindow,
        ),
        builder: (context, child) => DefaultTextStyle(
          style: MacosTheme.of(context).typography.body,
          child: _MacosMenus(child: child!),
        ),
        home: home,
      ),
    ),
  );
}

class _MacosMenus extends StatelessWidget {
  const _MacosMenus({required this.child});
  final Widget child;

  @override
  Widget build(BuildContext context) => PlatformMenuBar(
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

class MacosWindowTitle extends StatefulWidget {
  const MacosWindowTitle({super.key, required this.title, required this.child});
  final String title;
  final Widget child;

  @override
  State<MacosWindowTitle> createState() => _MacosWindowTitleState();
}

class _MacosWindowTitleState extends State<MacosWindowTitle> {
  static const _channel = MethodChannel('ink.rea.keytao/window');

  @override
  void initState() {
    super.initState();
    _updateTitle();
  }

  @override
  void didUpdateWidget(covariant MacosWindowTitle oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.title != oldWidget.title) _updateTitle();
  }

  void _updateTitle() => _channel.invokeMethod<void>('setTitle', widget.title);

  @override
  Widget build(BuildContext context) => widget.child;
}

class MacosErrorPresenter extends StatefulWidget {
  const MacosErrorPresenter({
    super.key,
    required this.controller,
    required this.child,
  });
  final AppController controller;
  final Widget child;

  @override
  State<MacosErrorPresenter> createState() => _MacosErrorPresenterState();
}

class _MacosErrorPresenterState extends State<MacosErrorPresenter> {
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
    final error = widget.controller.error;
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
        await showMacosAlertDialog<void>(
          context: context,
          builder: (context) => MacosAlertDialog(
            appIcon: const MacosIcon(CupertinoIcons.exclamationmark_triangle, size: 48),
            title: const Text('操作失败'),
            message: ConstrainedBox(
              constraints: const BoxConstraints(maxHeight: 180),
              child: SingleChildScrollView(child: SelectableText(error)),
            ),
            primaryButton: PushButton(
              controlSize: ControlSize.regular,
              onPressed: () => Navigator.pop(context),
              child: const Text('好'),
            ),
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

class MacosStartup extends StatelessWidget {
  const MacosStartup({super.key, this.error});
  final String? error;

  @override
  Widget build(BuildContext context) => KeyTaoMacosApp(
    home: MacosWindow(
      child: MacosScaffold(
        toolBar: const ToolBar(title: Text('KeyTao')),
        children: [
          ContentArea(
            builder: (context, _) => Center(
              child: error == null
                  ? const ProgressCircle()
                  : MacosAlertDialog(
                      appIcon: const MacosIcon(
                        CupertinoIcons.exclamationmark_triangle,
                        size: 48,
                      ),
                      title: const Text('启动失败'),
                      message: SelectableText(error!),
                      primaryButton: PushButton(
                        controlSize: ControlSize.regular,
                        onPressed: () => WindowManipulator.closeWindow(),
                        child: const Text('关闭'),
                      ),
                    ),
            ),
          ),
        ],
      ),
    ),
  );
}
