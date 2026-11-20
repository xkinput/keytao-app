import 'dart:async';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../rust/api/core.dart' as core;
import '../rust/api/types.dart';
import 'theme.dart';
import 'widgets.dart';

class ThemeLibrarySection extends StatefulWidget {
  const ThemeLibrarySection({super.key, required this.controller});
  final AppController controller;

  @override
  State<ThemeLibrarySection> createState() => _ThemeLibrarySectionState();
}

class _ThemeLibrarySectionState extends State<ThemeLibrarySection> {
  List<ThemeSummaryDto> _themes = [];
  ImeUiSettingsDto? _settings;
  bool _storageReady = false;
  bool _reloadPending = true;
  bool _loading = false;
  bool _working = false;
  String? _error;
  String? _message;
  Timer? _messageTimer;

  AppController get _controller => widget.controller;
  bool get _ready => !_controller.isAndroid || _controller.storageReady;
  bool get _enabled => _ready && !_controller.busy && !_loading && !_working;
  ThemeSummaryDto? get _current =>
      _themes.where((theme) => theme.current).firstOrNull;

  @override
  void initState() {
    super.initState();
    _settings = _controller.uiSettings;
    _storageReady = _ready;
    _controller.addListener(_controllerChanged);
    WidgetsBinding.instance.addPostFrameCallback((_) => _load());
  }

  @override
  void dispose() {
    _controller.removeListener(_controllerChanged);
    _messageTimer?.cancel();
    super.dispose();
  }

  void _controllerChanged() {
    final changed =
        !identical(_settings, _controller.uiSettings) ||
        _storageReady != _ready;
    _settings = _controller.uiSettings;
    _storageReady = _ready;
    if (changed) _reloadPending = true;
    if (_reloadPending && !_working && !_loading && !_controller.busy) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _load());
    }
    if (mounted) setState(() {});
  }

  Future<void> _load() async {
    if (!mounted || !_ready || _loading || _controller.busy) return;
    _reloadPending = false;
    setState(() => _loading = true);
    try {
      final themes = await core.getThemeLibrary();
      if (mounted) setState(() => _themes = themes);
    } catch (failure) {
      if (mounted) setState(() => _error = errorText(failure));
    } finally {
      if (mounted) {
        setState(() => _loading = false);
        if (_reloadPending) unawaited(_load());
      }
    }
  }

  void _notice(String message) {
    _messageTimer?.cancel();
    setState(() => _message = message);
    _messageTimer = Timer(const Duration(seconds: 8), () {
      if (mounted) setState(() => _message = null);
    });
  }

  Future<void> _perform(Future<void> Function() action) async {
    if (!_enabled) return;
    setState(() {
      _working = true;
      _error = null;
      _message = null;
    });
    try {
      await action();
    } catch (failure) {
      if (mounted) setState(() => _error = errorText(failure));
    } finally {
      if (mounted) {
        setState(() => _working = false);
        await _load();
      }
    }
  }

  Future<void> _apply(ThemeSummaryDto theme) => _perform(() async {
    await _controller.themeLibraryAction(
      () => core.applyTheme(id: theme.id),
      refresh: true,
    );
  });

  Future<void> _duplicate(ThemeSummaryDto theme) => _perform(() async {
    await _controller.themeLibraryAction(
      () => core.duplicateTheme(id: theme.id),
    );
    if (mounted) _notice('已复制 ${theme.name}');
  });

  Future<void> _delete(ThemeSummaryDto theme) => _perform(() async {
    final confirmed = await showFDialog<bool>(
      context: context,
      builder: (context, _, animation) => AppDialog(
        title: '删除主题',
        animation: animation,
        actions: [
          ActionButton('取消', onPress: () => Navigator.pop(context, false)),
          ActionButton(
            '删除',
            primary: true,
            onPress: () => Navigator.pop(context, true),
          ),
        ],
        child: Text(theme.name),
      ),
    );
    if (confirmed != true) return;
    await _controller.themeLibraryAction(
      () => core.deleteTheme(id: theme.id),
      refresh: true,
    );
    if (mounted) _notice('已删除 ${theme.name}');
  });

  Future<void> _export(String id, {bool copy = false}) => _perform(() async {
    final exported = await _controller.themeLibraryAction(
      () => core.exportTheme(id: id),
    );
    if (!mounted) return;
    if (copy && exported.useFile) {
      final useFile = await showFDialog<bool>(
        context: context,
        builder: (context, _, animation) => AppDialog(
          title: '分享串超过 4000 字符',
          animation: animation,
          actions: [
            ActionButton('取消', onPress: () => Navigator.pop(context, false)),
            ActionButton(
              '导出文件',
              primary: true,
              onPress: () => Navigator.pop(context, true),
            ),
          ],
          child: const Text('请使用文件分享'),
        ),
      );
      if (useFile != true) return;
    } else if (copy) {
      await Clipboard.setData(ClipboardData(text: exported.shareString));
      if (mounted) _notice('分享串已复制');
      return;
    }
    await _controller.shareThemeFile(exported);
  });

  Future<void> _paste() => _perform(() async {
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    final text = data?.text;
    if (text == null || text.trim().isEmpty) {
      throw StateError('剪贴板为空');
    }
    await _import(text);
  });

  Future<void> _openFile() => _perform(() async {
    final file = await openFile();
    if (file == null) return;
    if (await file.length() > 96 * 1024) {
      throw StateError('主题文件超过大小上限');
    }
    await _import(await file.readAsString());
  });

  Future<void> _import(String text) async {
    final preview = await _controller.themeLibraryAction(
      () => core.previewThemeImport(text: text),
    );
    if (!mounted) return;
    var overwrite = false;
    if (preview.conflict && preview.canOverwrite) {
      final choice = await showFDialog<bool>(
        context: context,
        builder: (context, _, animation) => AppDialog(
          title: '同名主题已存在',
          animation: animation,
          actions: [
            ActionButton('取消', onPress: () => Navigator.pop(context)),
            ActionButton('覆盖', onPress: () => Navigator.pop(context, true)),
            ActionButton(
              '另存为副本',
              primary: true,
              onPress: () => Navigator.pop(context, false),
            ),
          ],
          child: Text(preview.name),
        ),
      );
      if (choice == null) return;
      overwrite = choice;
    }
    final result = await _controller.themeLibraryAction(
      () => core.importTheme(text: text, overwrite: overwrite),
      refresh: true,
    );
    if (!mounted) return;
    final status = switch (result.disposition) {
      'alreadyInstalled' => '已安装，内容相同，已跳过',
      'copied' => '已另存为副本',
      'overwritten' => '已覆盖',
      _ => '已导入',
    };
    _notice(
      [
        '$status ${result.name}',
        if (result.renamedFrom case final original?)
          '$original → ${result.id}（主题 ID 冲突）',
        ...result.dropped.map((field) => '已丢弃 $field'),
        ...result.clamped.map((field) => '已调整 $field'),
        ...result.unknown.map((field) => '已忽略 $field'),
        ...result.warnings,
      ].join('\n'),
    );
  }

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    spacing: 4,
    children: [
      Row(
        children: [
          const Expanded(child: EyebrowLabel('主题库')),
          _action(
            '粘贴导入',
            FLucideIcons.clipboardPaste,
            _enabled ? _paste : null,
          ),
          _action(
            '复制分享串',
            FLucideIcons.copy,
            _enabled && _current != null
                ? () => _export(_current!.id, copy: true)
                : null,
          ),
          _action(
            '导出文件',
            FLucideIcons.download,
            _enabled && _current != null ? () => _export(_current!.id) : null,
          ),
          _action('导入文件', FLucideIcons.upload, _enabled ? _openFile : null),
        ],
      ),
      if (_loading || _working) const FProgress(),
      if (_themes.isNotEmpty)
        SizedBox(
          height: 64,
          child: SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: Row(
              spacing: 8,
              children: [
                for (final theme in _themes)
                  SizedBox(width: 144, child: _card(context, theme)),
              ],
            ),
          ),
        ),
      if (_message case final message?)
        Semantics(liveRegion: true, child: Text(message)),
      if (_error case final error?) ErrorMessage(error),
    ],
  );

  Widget _action(String label, IconData icon, VoidCallback? onPress) =>
      FTooltip(
        tipBuilder: (_, _) => Text(label),
        child: FButton.icon(
          variant: FButtonVariant.ghost,
          onPress: onPress,
          style: const FButtonStyleDelta.delta(
            contentStyle: FButtonContentStyleDelta.delta(
              constraints: BoxConstraints.tightFor(width: 44, height: 44),
              padding: EdgeInsetsGeometryDelta.value(EdgeInsets.zero),
            ),
          ),
          child: Icon(icon, size: 16, semanticLabel: label),
        ),
      );

  Widget _card(BuildContext context, ThemeSummaryDto theme) {
    final palette = AppPalette.of(context);
    return Semantics(
      selected: theme.current,
      child: FTappable(
        semanticsLabel: '应用主题 ${theme.name}',
        onPress: _enabled && !theme.current ? () => _apply(theme) : null,
        child: Container(
          decoration: BoxDecoration(
            color: palette.surface,
            border: Border.all(
              color: theme.current
                  ? ChromeAccent.of(context)
                  : palette.hairline,
            ),
            borderRadius: BorderRadius.circular(8),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Padding(
                padding: const EdgeInsets.only(left: 6, right: 6, top: 2),
                child: Row(
                  spacing: 6,
                  children: [
                    Expanded(child: _colors(theme.light, '浅色')),
                    Expanded(child: _colors(theme.dark, '深色')),
                  ],
                ),
              ),
              Padding(
                padding: const EdgeInsets.only(left: 6),
                child: Row(
                  spacing: 4,
                  children: [
                    Expanded(
                      child: Text(
                        theme.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                    if (theme.current)
                      Icon(
                        FLucideIcons.check,
                        size: 16,
                        color: ChromeAccent.of(context),
                        semanticLabel: '当前主题',
                      ),
                    FPopoverMenu(
                      menuAnchor: Alignment.topRight,
                      childAnchor: Alignment.bottomRight,
                      menuBuilder: (_, popover, _) => [
                        FItemGroup(
                          enabled: _enabled,
                          children: [
                            FItem(
                              title: const Text('复制'),
                              onPress: () {
                                popover.hide();
                                _duplicate(theme);
                              },
                            ),
                            FItem(
                              title: const Text('导出'),
                              onPress: () {
                                popover.hide();
                                _export(theme.id);
                              },
                            ),
                            FItem(
                              title: const Text('删除'),
                              enabled: !theme.builtIn,
                              onPress: theme.builtIn
                                  ? null
                                  : () {
                                      popover.hide();
                                      _delete(theme);
                                    },
                            ),
                          ],
                        ),
                      ],
                      builder: (_, popover, _) => _action(
                        '${theme.name} 主题操作',
                        FLucideIcons.ellipsis,
                        _enabled ? popover.toggle : null,
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _colors(ThemePreviewColorsDto colors, String label) => Semantics(
    label: '$label预览',
    child: Container(
      height: 16,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: _color(colors.background),
        borderRadius: BorderRadius.circular(4),
      ),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.center,
        spacing: 4,
        children: [
          Container(
            width: 6,
            height: 6,
            decoration: BoxDecoration(
              color: _color(colors.accent),
              shape: BoxShape.circle,
            ),
          ),
          Text(
            '字',
            style: TextStyle(color: _color(colors.foreground), fontSize: 10),
          ),
        ],
      ),
    ),
  );

  Color _color(String rgba) {
    final value = int.parse(rgba.substring(1), radix: 16);
    return Color(((value & 0xff) << 24) | (value >> 8));
  }
}
