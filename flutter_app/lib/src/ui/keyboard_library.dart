import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'candidate_panel_preview.dart';
import 'keyboard_editor_page.dart';
import 'mobile_keyboard_preview.dart';
import 'theme.dart';
import 'widgets.dart';

class KeyboardLibrarySection extends StatefulWidget {
  const KeyboardLibrarySection({
    super.key,
    required this.controller,
    required this.previewTheme,
    required this.schemeName,
    this.onMore,
  });

  final AppController controller;
  final Map<String, dynamic> previewTheme;
  final String schemeName;
  final VoidCallback? onMore;

  @override
  State<KeyboardLibrarySection> createState() => _KeyboardLibrarySectionState();
}

class _KeyboardLibrarySectionState extends State<KeyboardLibrarySection> {
  bool _busy = false;
  String? _status;
  String? _error;

  AppController get _controller => widget.controller;
  bool get _enabled => !_busy && !_controller.busy;

  KeyboardSummaryDto? get _current =>
      _controller.keyboards.where((k) => k.current).firstOrNull;

  Future<void> _perform(Future<void> Function() action) async {
    if (!_enabled) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await action();
    } catch (failure) {
      if (mounted) {
        setState(() => _error = errorText(failure));
      }
    } finally {
      if (mounted) {
        setState(() => _busy = false);
      }
    }
  }

  void _notice(String message) => setState(() {
    _status = message;
    _error = null;
  });

  Future<void> _apply(KeyboardSummaryDto keyboard) => _perform(() async {
    if (await _controller.applyKeyboardProfile(keyboard.id) && mounted) {
      _notice('已切换为 ${keyboard.name}');
    }
  });

  Future<void> _export(String id, {bool copy = false}) => _perform(() async {
    final exported = await _controller.exportKeyboardProfile(id);
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
          child: const Text('该键盘配置较大，请使用文件分享'),
        ),
      );
      if (useFile != true) return;
    } else if (copy) {
      await Clipboard.setData(ClipboardData(text: exported.shareString));
      if (mounted) _notice('键盘分享串已复制');
      return;
    }
    await _controller.shareKeyboardFile(exported);
    if (mounted) _notice('已导出 ${exported.fileName}');
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
    if (await file.length() > 384 * 1024) {
      throw StateError('键盘配置文件超过 384 KiB 上限');
    }
    await _import(await file.readAsString());
  });

  Future<void> _import(String text) async {
    final preview = await _controller.previewKeyboardImport(text);
    if (!mounted) return;
    var overwrite = false;
    if (preview.conflict && !preview.builtInConflict) {
      final choice = await showFDialog<bool>(
        context: context,
        builder: (context, _, animation) => AppDialog(
          title: '同名键盘配置已存在',
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
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            spacing: 10,
            children: [
              Text(preview.name),
              MobileKeyboardPreview(
                theme: widget.previewTheme,
                schemeName: widget.schemeName,
                layoutJsonOverride: preview.previewLayoutJson,
                mnemonicHintsOverride: preview.hasMnemonics,
                showToolbar: false,
                fullHeight: true,
              ),
            ],
          ),
        ),
      );
      if (choice == null) return;
      overwrite = choice;
    }
    final result = await _controller.importKeyboardProfile(
      text,
      overwrite: overwrite,
    );
    if (!mounted) return;
    final status = switch (result.disposition) {
      'copied' => '已另存为副本',
      'overwritten' => '已覆盖',
      _ => '已导入',
    };
    _notice(['$status ${result.name}', ...result.warnings].join('\n'));
  }

  void _openMore() {
    if (widget.onMore case final callback?) {
      callback();
      return;
    }
    showKeyboardDetailPage(
      context,
      controller: _controller,
      previewTheme: widget.previewTheme,
      schemeName: widget.schemeName,
    );
  }

  Widget _action(String label, IconData icon, VoidCallback? onPress) =>
      FTooltip(
        tipBuilder: (_, _) => Text(label),
        child: FButton.icon(
          variant: FButtonVariant.ghost,
          onPress: onPress,
          style: const FButtonStyleDelta.delta(
            contentStyle: FButtonContentStyleDelta.delta(
              constraints: BoxConstraints.tightFor(width: 40, height: 40),
              padding: EdgeInsetsGeometryDelta.value(EdgeInsets.zero),
            ),
          ),
          child: Icon(icon, size: 16, semanticLabel: label),
        ),
      );

  @override
  Widget build(BuildContext context) {
    final keyboards = _controller.keyboards;
    final recommended = _controller.recommendedKeyboard;
    final current = _current;

    return Column(
      key: const ValueKey('mobile-keyboard-library'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      spacing: 8,
      children: [
        Row(
          children: [
            const EyebrowLabel('键盘方案'),
            Expanded(
              child: Wrap(
                alignment: WrapAlignment.end,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  _action(
                    '粘贴导入',
                    FLucideIcons.clipboardPaste,
                    _enabled ? _paste : null,
                  ),
                  _action(
                    '复制分享串',
                    FLucideIcons.copy,
                    _enabled && current != null
                        ? () => _export(current.id, copy: true)
                        : null,
                  ),
                  _action(
                    '导出文件',
                    FLucideIcons.download,
                    _enabled && current != null
                        ? () => _export(current.id)
                        : null,
                  ),
                  _action(
                    '导入文件',
                    FLucideIcons.upload,
                    _enabled ? _openFile : null,
                  ),
                  FButton(
                    key: const ValueKey('keyboard-library-more-button'),
                    variant: FButtonVariant.ghost,
                    size: context.platformVariant.touch
                        ? FButtonSizeVariant.md
                        : FButtonSizeVariant.xs,
                    mainAxisSize: MainAxisSize.min,
                    onPress: _enabled ? _openMore : null,
                    child: const Text('更多键盘'),
                  ),
                ],
              ),
            ),
          ],
        ),
        if (_busy) const FProgress(),
        if (keyboards.isEmpty)
          StatusMessage('正在加载键盘方案…', icon: FLucideIcons.keyboard)
        else
          LayoutBuilder(
            key: const ValueKey('keyboard-profile-grid'),
            builder: (context, constraints) {
              const gap = 6.0;
              const minChipWidth = 112.0;
              const maxChipWidth = 160.0;
              final availableWidth = constraints.hasBoundedWidth
                  ? constraints.maxWidth
                  : maxChipWidth;
              final columns = ((availableWidth + gap) / (minChipWidth + gap))
                  .floor()
                  .clamp(1, keyboards.length);
              final width = ((availableWidth + gap) / columns - gap)
                  .floorToDouble()
                  .clamp(0.0, maxChipWidth);
              return Wrap(
                spacing: gap,
                runSpacing: gap,
                children: [
                  for (final kb in keyboards)
                    SizedBox(
                      width: width,
                      child: _CompactKeyboardCard(
                        key: ValueKey('keyboard-card-${kb.id}'),
                        keyboard: kb,
                        isRecommended: recommended?.id == kb.id,
                        enabled: _enabled,
                        onTap: () => _apply(kb),
                      ),
                    ),
                ],
              );
            },
          ),
        if (_status case final message?)
          StatusMessage(message, icon: FLucideIcons.circleCheck),
        if (_error case final message?) ErrorMessage(message),
      ],
    );
  }
}

class _CompactKeyboardCard extends StatelessWidget {
  const _CompactKeyboardCard({
    super.key,
    required this.keyboard,
    required this.isRecommended,
    required this.enabled,
    required this.onTap,
  });

  final KeyboardSummaryDto keyboard;
  final bool isRecommended;
  final bool enabled;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = AppPalette.of(context);
    final accent = ChromeAccent.of(context);
    final active = keyboard.current;
    return Semantics(
      selected: active,
      child: FTappable(
        semanticsLabel: '切换键盘 ${keyboard.name}',
        onPress: enabled && !active ? onTap : null,
        child: Container(
          constraints: const BoxConstraints(minHeight: 44),
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 6),
          decoration: BoxDecoration(
            color: active ? accent.withValues(alpha: 0.08) : palette.surface,
            border: Border.all(
              color: active ? accent : palette.hairline,
              width: active ? 1.5 : 1,
            ),
            borderRadius: BorderRadius.circular(8),
          ),
          child: Row(
            spacing: 4,
            children: [
              Expanded(
                child: Text(
                  keyboard.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: 12,
                    fontWeight: active ? FontWeight.w600 : FontWeight.w500,
                    color: active ? accent : palette.ink,
                  ),
                ),
              ),
              if (isRecommended) _Badge('推荐', accent),
              if (active)
                Icon(
                  FLucideIcons.check,
                  size: 13,
                  color: accent,
                  semanticLabel: '当前键盘',
                ),
            ],
          ),
        ),
      ),
    );
  }
}

class _Badge extends StatelessWidget {
  const _Badge(this.label, this.color);

  final String label;
  final Color color;

  @override
  Widget build(BuildContext context) => Container(
    padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 1),
    decoration: BoxDecoration(
      color: color.withValues(alpha: 0.12),
      borderRadius: BorderRadius.circular(4),
    ),
    child: Text(
      label,
      style: TextStyle(fontSize: 10, fontWeight: FontWeight.w600, color: color),
    ),
  );
}

Future<bool?> showRecommendedKeyboardDialog(
  BuildContext context,
  AppController controller,
  KeyboardSummaryDto recommended,
) {
  final s = controller.uiSettings;
  final accent =
      s?.accentColor ??
      '#${(ChromeAccent.of(context).toARGB32() & 0xFFFFFF).toRadixString(16).padLeft(6, '0')}';
  final theme = controller.cachedCandidatePanelPreview(
    colorScheme: s?.colorScheme ?? UiColorSchemeDto.auto,
    accentColor: accent,
    cornerRadius: s?.cornerRadius,
    backgroundColor: s?.backgroundColor ?? '',
    fontSize: s?.fontSize ?? 18,
    systemDark: MediaQuery.platformBrightnessOf(context) == Brightness.dark,
  );
  final schemeName =
      controller.currentSchemaName ??
      controller.installedPackageFor(controller.activeSchemeKey)?.displayName ??
      schemeDisplayName(controller.activeSchemeKey);

  return showFDialog<bool>(
    context: context,
    builder: (context, _, animation) => AppDialog(
      title: '切换推荐键盘',
      animation: animation,
      actions: [
        ActionButton('暂不切换', onPress: () => Navigator.pop(context, false)),
        ActionButton(
          '切换为 ${recommended.name}',
          primary: true,
          onPress: () => Navigator.pop(context, true),
        ),
      ],
      child: Column(
        key: const ValueKey('recommended-keyboard-dialog'),
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        spacing: 12,
        children: [
          Text(
            '当前方案推荐使用「${recommended.name}」，是否立即切换？',
            style: context.theme.typography.body.sm,
          ),
          ClipRRect(
            borderRadius: BorderRadius.circular(10),
            child: MobileKeyboardPreview(
              theme: theme,
              schemeName: schemeName,
              layoutJsonOverride: recommended.previewLayoutJson,
              mnemonicHintsOverride:
                  recommended.hasMnemonics &&
                  (recommended.id == 'keytao.keytao26' ||
                      recommended.id == 'keytao.keydo30'),
              showToolbar: false,
              fullHeight: true,
            ),
          ),
        ],
      ),
    ),
  );
}
