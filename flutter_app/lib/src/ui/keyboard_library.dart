import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';
import 'candidate_panel_preview.dart';
import 'mobile_keyboard_preview.dart';
import 'theme.dart';
import 'widgets.dart';

class KeyboardLibrarySection extends StatefulWidget {
  const KeyboardLibrarySection({
    super.key,
    required this.controller,
    required this.previewTheme,
    required this.schemeName,
  });

  final AppController controller;
  final Map<String, dynamic> previewTheme;
  final String schemeName;

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

  Future<void> _delete(KeyboardSummaryDto keyboard) => _perform(() async {
    final confirmed = await showFDialog<bool>(
      context: context,
      builder: (context, _, animation) => AppDialog(
        title: '删除自定义键盘',
        animation: animation,
        actions: [
          ActionButton('取消', onPress: () => Navigator.pop(context, false)),
          ActionButton(
            '删除',
            primary: true,
            onPress: () => Navigator.pop(context, true),
          ),
        ],
        child: Text('确定删除「${keyboard.name}」吗？'),
      ),
    );
    if (confirmed != true) return;
    if (await _controller.deleteKeyboardProfile(keyboard.id) && mounted) {
      _notice('已删除 ${keyboard.name}');
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
    _notice([
      '$status ${result.name}',
      ...result.warnings,
    ].join('\n'));
  }

  Widget _toolbarAction(String label, IconData icon, VoidCallback? onPress) =>
      Tooltip(
        message: label,
        child: FButton.icon(
          variant: FButtonVariant.ghost,
          size: FButtonSizeVariant.sm,
          onPress: onPress,
          child: Icon(icon, size: 16),
        ),
      );

  @override
  Widget build(BuildContext context) {
    final palette = AppPalette.of(context);
    final accent = ChromeAccent.of(context);
    final keyboards = _controller.keyboards;
    final recommended = _controller.recommendedKeyboard;
    final current = _current;
    final showMnemonicInCard = (KeyboardSummaryDto kb) {
      if (!kb.hasMnemonics) return false;
      if (kb.current) {
        return _controller.androidSettings?.mnemonicHintsEnabled ?? false;
      }
      return kb.id == 'keytao.keytao26' || kb.id == 'keytao.keydo30';
    };

    return Section(
      key: const ValueKey('mobile-keyboard-library'),
      title: '键盘方案',
      trailing: Wrap(
        spacing: 2,
        children: [
          _toolbarAction(
            '粘贴导入',
            FLucideIcons.clipboardPaste,
            _enabled ? _paste : null,
          ),
          _toolbarAction(
            '复制分享串',
            FLucideIcons.copy,
            _enabled && current != null
                ? () => _export(current.id, copy: true)
                : null,
          ),
          _toolbarAction(
            '导出文件',
            FLucideIcons.download,
            _enabled && current != null ? () => _export(current.id) : null,
          ),
          _toolbarAction(
            '导入文件',
            FLucideIcons.upload,
            _enabled ? _openFile : null,
          ),
        ],
      ),
      children: [
        if (recommended != null)
          Container(
            key: const ValueKey('recommended-keyboard-banner'),
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
            decoration: BoxDecoration(
              color: accent.withValues(alpha: 0.1),
              borderRadius: BorderRadius.circular(10),
              border: Border.all(color: accent.withValues(alpha: 0.35)),
            ),
            child: Row(
              spacing: 10,
              children: [
                Icon(FLucideIcons.sparkles, size: 16, color: accent),
                Expanded(
                  child: Text(
                    '推荐当前方案使用「${recommended.name}」',
                    style: context.theme.typography.body.sm.copyWith(
                      color: palette.ink,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                ),
                if (!recommended.current)
                  ActionButton(
                    '切换',
                    primary: true,
                    onPress: _enabled ? () => _apply(recommended) : null,
                  )
                else
                  Text(
                    '已启用',
                    style: context.theme.typography.body.xs.copyWith(
                      color: accent,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
              ],
            ),
          ),
        if (keyboards.isEmpty)
          StatusMessage('正在加载键盘方案…', icon: FLucideIcons.keyboard)
        else
          GridView.builder(
            key: const ValueKey('keyboard-profile-grid'),
            shrinkWrap: true,
            physics: const NeverScrollableScrollPhysics(),
            gridDelegate: const SliverGridDelegateWithFixedCrossAxisCount(
              crossAxisCount: 2,
              crossAxisSpacing: 10,
              mainAxisSpacing: 10,
              childAspectRatio: 1.08,
            ),
            itemCount: keyboards.length,
            itemBuilder: (context, index) {
              final kb = keyboards[index];
              final isRecommended = recommended?.id == kb.id;
              return _KeyboardProfileCard(
                key: ValueKey('keyboard-card-${kb.id}'),
                keyboard: kb,
                isRecommended: isRecommended,
                previewTheme: widget.previewTheme,
                schemeName: widget.schemeName,
                showMnemonicPreview: showMnemonicInCard(kb),
                enabled: _enabled,
                onTap: () => _apply(kb),
                onDelete: kb.builtIn ? null : () => _delete(kb),
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

class _KeyboardProfileCard extends StatelessWidget {
  const _KeyboardProfileCard({
    super.key,
    required this.keyboard,
    required this.isRecommended,
    required this.previewTheme,
    required this.schemeName,
    required this.showMnemonicPreview,
    required this.enabled,
    required this.onTap,
    this.onDelete,
  });

  final KeyboardSummaryDto keyboard;
  final bool isRecommended;
  final Map<String, dynamic> previewTheme;
  final String schemeName;
  final bool showMnemonicPreview;
  final bool enabled;
  final VoidCallback onTap;
  final VoidCallback? onDelete;

  @override
  Widget build(BuildContext context) {
    final palette = AppPalette.of(context);
    final accent = ChromeAccent.of(context);
    final active = keyboard.current;
    return Material(
      color: active ? accent.withValues(alpha: 0.08) : palette.surface,
      borderRadius: BorderRadius.circular(12),
      child: InkWell(
        borderRadius: BorderRadius.circular(12),
        onTap: enabled && !active ? onTap : null,
        child: Container(
          padding: const EdgeInsets.all(8),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(12),
            border: Border.all(
              color: active ? accent : palette.hairline,
              width: active ? 1.5 : 1,
            ),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(
                child: ClipRRect(
                  borderRadius: BorderRadius.circular(8),
                  child: Center(
                    child: MobileKeyboardPreview(
                      theme: previewTheme,
                      schemeName: schemeName,
                      layoutJsonOverride: keyboard.previewLayoutJson,
                      mnemonicHintsOverride: showMnemonicPreview,
                      showToolbar: false,
                      fullHeight: true,
                    ),
                  ),
                ),
              ),
              const SizedBox(height: 6),
              Row(
                children: [
                  Expanded(
                    child: Text(
                      keyboard.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: context.theme.typography.body.sm.copyWith(
                        fontWeight: active ? FontWeight.w600 : FontWeight.w500,
                        color: active ? accent : palette.ink,
                      ),
                    ),
                  ),
                  if (onDelete != null)
                    GestureDetector(
                      onTap: enabled ? onDelete : null,
                      child: Padding(
                        padding: const EdgeInsets.only(left: 4),
                        child: Icon(
                          FLucideIcons.trash2,
                          size: 14,
                          color: palette.muted,
                        ),
                      ),
                    ),
                ],
              ),
              if (active || isRecommended || keyboard.hasMnemonics) ...[
                const SizedBox(height: 3),
                Wrap(
                  spacing: 4,
                  runSpacing: 2,
                  children: [
                    if (active) _Badge('使用中', accent),
                    if (isRecommended) _Badge('推荐', accent),
                    if (keyboard.hasMnemonics) _Badge('助记', palette.muted),
                  ],
                ),
              ],
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
