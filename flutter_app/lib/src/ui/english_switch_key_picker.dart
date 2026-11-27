import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:forui/forui.dart';

import 'theme.dart';
import 'widgets.dart';

Map<String, String> englishSwitchKeyPresets({
  required bool isMacOs,
  List<String> schemaKeys = const [],
}) {
  final alt = isMacOs ? 'Option' : 'Alt';
  final schemaSummary = formatSchemaEnglishSwitchKeys(
    schemaKeys,
    isMacOs: isMacOs,
  );
  return {
    'schema': schemaSummary == null ? '跟随方案' : '跟随方案（$schemaSummary）',
    'Shift': '左右 Shift',
    'Shift_L': '左 Shift',
    'Shift_R': '右 Shift',
    'Control': '左右 Control',
    'Control_L': '左 Control',
    'Control_R': '右 Control',
    'Alt': '左右 $alt',
    'Alt_L': '左 $alt',
    'Alt_R': '右 $alt',
    if (!isMacOs) 'Caps_Lock': 'Caps Lock',
  };
}

String? formatSchemaEnglishSwitchKeys(
  List<String> keys, {
  required bool isMacOs,
}) {
  if (keys.isEmpty) return null;
  final alt = isMacOs ? 'Option' : 'Alt';
  final set = keys.toSet();
  final labels = <String>[];
  final consumed = <String>{};

  void addPair(String left, String right, String bothLabel, String lLabel, String rLabel) {
    final hasL = set.contains(left);
    final hasR = set.contains(right);
    if (hasL && hasR) {
      labels.add(bothLabel);
      consumed..add(left)..add(right);
    } else if (hasL) {
      labels.add(lLabel);
      consumed.add(left);
    } else if (hasR) {
      labels.add(rLabel);
      consumed.add(right);
    }
  }

  addPair('Shift_L', 'Shift_R', '左右 Shift', '左 Shift', '右 Shift');
  addPair('Control_L', 'Control_R', '左右 Control', '左 Control', '右 Control');
  addPair('Alt_L', 'Alt_R', '左右 $alt', '左 $alt', '右 $alt');
  for (final key in keys) {
    if (consumed.contains(key)) continue;
    labels.add(englishSwitchKeyLabel(key, isMacOs: isMacOs));
    consumed.add(key);
  }
  return labels.isEmpty ? null : labels.join(' / ');
}

String englishSwitchKeyLabel(
  String value, {
  required bool isMacOs,
  List<String> schemaKeys = const [],
}) {
  final trimmed = value.trim();
  if (trimmed.isEmpty || trimmed.toLowerCase() == 'schema') {
    final schemaSummary = formatSchemaEnglishSwitchKeys(
      schemaKeys,
      isMacOs: isMacOs,
    );
    return schemaSummary == null ? '跟随方案' : '跟随方案（$schemaSummary）';
  }
  final presets = englishSwitchKeyPresets(isMacOs: isMacOs);
  if (presets[trimmed] case final label?) return label;
  if (trimmed == 'Caps_Lock') return 'Caps Lock';
  final alt = isMacOs ? 'Option' : 'Alt';
  final parts = trimmed.split('+');
  return parts
      .map((part) {
        return switch (part) {
          'Control' => 'Ctrl',
          'Alt' => alt,
          'Shift' => 'Shift',
          'space' => 'Space',
          'grave' => '`',
          'minus' => '-',
          'equal' => '=',
          'bracketleft' => '[',
          'bracketright' => ']',
          'backslash' => '\\',
          'semicolon' => ';',
          'apostrophe' => "'",
          'comma' => ',',
          'period' => '.',
          'slash' => '/',
          'BackSpace' => 'Backspace',
          'Return' => 'Enter',
          'Escape' => 'Esc',
          'Tab' => 'Tab',
          _ when part.length == 1 => part.toUpperCase(),
          _ => part,
        };
      })
      .join(' + ');
}

String? rimeKeyTokenForLogicalKey(LogicalKeyboardKey key) {
  if (key == LogicalKeyboardKey.space) return 'space';
  if (key == LogicalKeyboardKey.backquote) return 'grave';
  if (key == LogicalKeyboardKey.minus) return 'minus';
  if (key == LogicalKeyboardKey.equal) return 'equal';
  if (key == LogicalKeyboardKey.bracketLeft) return 'bracketleft';
  if (key == LogicalKeyboardKey.bracketRight) return 'bracketright';
  if (key == LogicalKeyboardKey.backslash) return 'backslash';
  if (key == LogicalKeyboardKey.semicolon) return 'semicolon';
  if (key == LogicalKeyboardKey.quoteSingle) return 'apostrophe';
  if (key == LogicalKeyboardKey.comma) return 'comma';
  if (key == LogicalKeyboardKey.period) return 'period';
  if (key == LogicalKeyboardKey.slash) return 'slash';
  if (key == LogicalKeyboardKey.tab) return 'Tab';
  if (key == LogicalKeyboardKey.enter) return 'Return';
  if (key == LogicalKeyboardKey.backspace) return 'BackSpace';
  if (key == LogicalKeyboardKey.escape) return 'Escape';
  if (key == LogicalKeyboardKey.delete) return 'Delete';
  if (key == LogicalKeyboardKey.insert) return 'Insert';
  if (key == LogicalKeyboardKey.home) return 'Home';
  if (key == LogicalKeyboardKey.end) return 'End';
  if (key == LogicalKeyboardKey.pageUp) return 'Page_Up';
  if (key == LogicalKeyboardKey.pageDown) return 'Page_Down';
  final fKeys = <LogicalKeyboardKey, String>{
    LogicalKeyboardKey.f1: 'F1',
    LogicalKeyboardKey.f2: 'F2',
    LogicalKeyboardKey.f3: 'F3',
    LogicalKeyboardKey.f4: 'F4',
    LogicalKeyboardKey.f5: 'F5',
    LogicalKeyboardKey.f6: 'F6',
    LogicalKeyboardKey.f7: 'F7',
    LogicalKeyboardKey.f8: 'F8',
    LogicalKeyboardKey.f9: 'F9',
    LogicalKeyboardKey.f10: 'F10',
    LogicalKeyboardKey.f11: 'F11',
    LogicalKeyboardKey.f12: 'F12',
  };
  if (fKeys[key] case final fKey?) return fKey;
  final label = key.keyLabel;
  if (label.length == 1) {
    final code = label.codeUnitAt(0);
    if (code >= 0x41 && code <= 0x5a) return label.toLowerCase();
    if (code >= 0x61 && code <= 0x7a) return label;
    if (code >= 0x30 && code <= 0x39) return label;
  }
  return null;
}

String? modifierPresetForLogicalKey(
  LogicalKeyboardKey key, {
  required bool isMacOs,
}) {
  if (key == LogicalKeyboardKey.shiftLeft) return 'Shift_L';
  if (key == LogicalKeyboardKey.shiftRight) return 'Shift_R';
  if (key == LogicalKeyboardKey.controlLeft) return 'Control_L';
  if (key == LogicalKeyboardKey.controlRight) return 'Control_R';
  if (key == LogicalKeyboardKey.altLeft) return 'Alt_L';
  if (key == LogicalKeyboardKey.altRight) return 'Alt_R';
  if (!isMacOs && key == LogicalKeyboardKey.capsLock) return 'Caps_Lock';
  return null;
}

bool _isMetaKey(LogicalKeyboardKey key) =>
    key == LogicalKeyboardKey.meta ||
    key == LogicalKeyboardKey.metaLeft ||
    key == LogicalKeyboardKey.metaRight ||
    key == LogicalKeyboardKey.superKey;

Future<String?> showEnglishSwitchKeyDialog(
  BuildContext context, {
  required String currentKey,
  required bool isMacOs,
  List<String> schemaKeys = const [],
}) {
  FocusManager.instance.primaryFocus?.unfocus();
  return showFDialog<String>(
    context: context,
    builder: (context, _, animation) => _EnglishSwitchKeyRecorderDialog(
      currentKey: currentKey,
      isMacOs: isMacOs,
      schemaKeys: schemaKeys,
      animation: animation,
    ),
  );
}

class _EnglishSwitchKeyRecorderDialog extends StatefulWidget {
  const _EnglishSwitchKeyRecorderDialog({
    required this.currentKey,
    required this.isMacOs,
    required this.schemaKeys,
    required this.animation,
  });

  final String currentKey;
  final bool isMacOs;
  final List<String> schemaKeys;
  final Animation<double> animation;

  @override
  State<_EnglishSwitchKeyRecorderDialog> createState() =>
      _EnglishSwitchKeyRecorderDialogState();
}

class _EnglishSwitchKeyRecorderDialogState
    extends State<_EnglishSwitchKeyRecorderDialog> {
  final FocusNode _focusNode = FocusNode();
  String? _captured;
  String? _pendingModifier;
  String? _hint;

  @override
  void dispose() {
    _focusNode.dispose();
    super.dispose();
  }

  KeyEventResult _handleKey(FocusNode _, KeyEvent event) {
    final key = event.logicalKey;
    final hardware = HardwareKeyboard.instance;
    if (_isMetaKey(key) || hardware.isMetaPressed) {
      if (event is KeyDownEvent) {
        setState(() {
          _pendingModifier = null;
          _hint = '不支持 Command / Win 系统保留键';
        });
      }
      return KeyEventResult.handled;
    }

    if (widget.isMacOs && key == LogicalKeyboardKey.capsLock) {
      if (event is KeyDownEvent) {
        setState(() {
          _pendingModifier = null;
          _hint = 'macOS 下 Caps Lock 由系统输入法切换管理，请选择其它按键';
        });
      }
      return KeyEventResult.handled;
    }

    final modifierPreset = modifierPresetForLogicalKey(
      key,
      isMacOs: widget.isMacOs,
    );
    if (modifierPreset != null) {
      if (event is KeyDownEvent) {
        final otherPressed =
            (modifierPreset.startsWith('Shift') &&
                (hardware.isControlPressed || hardware.isAltPressed)) ||
            (modifierPreset.startsWith('Control') &&
                (hardware.isShiftPressed || hardware.isAltPressed)) ||
            (modifierPreset.startsWith('Alt') &&
                (hardware.isShiftPressed || hardware.isControlPressed));
        setState(() {
          _pendingModifier = otherPressed ? null : modifierPreset;
          _hint = null;
        });
      } else if (event is KeyUpEvent && _pendingModifier == modifierPreset) {
        setState(() {
          _captured = modifierPreset;
          _pendingModifier = null;
          _hint = null;
        });
      }
      return KeyEventResult.handled;
    }

    if (event is! KeyDownEvent) return KeyEventResult.handled;

    _pendingModifier = null;
    final token = rimeKeyTokenForLogicalKey(key);
    if (token == null) {
      setState(() => _hint = '暂不支持该按键，请改用修饰键或 Ctrl / Alt 组合键');
      return KeyEventResult.handled;
    }
    final ctrl = hardware.isControlPressed;
    final alt = hardware.isAltPressed;
    final shift = hardware.isShiftPressed;
    final isFunctionKey = RegExp(r'^F([1-9]|1[0-2])$').hasMatch(token);
    final isShiftSafe = shift && !ctrl && !alt && (token == 'space' || isFunctionKey);
    if (!ctrl && !alt && !isShiftSafe && !isFunctionKey) {
      setState(() {
        _hint = '请轻按单个修饰键，或使用 Ctrl / Alt 组合键、Shift+空格、F1–F12';
      });
      return KeyEventResult.handled;
    }
    final spec = [
      if (ctrl) 'Control',
      if (alt) 'Alt',
      if (shift) 'Shift',
      token,
    ].join('+');
    setState(() {
      _captured = spec;
      _hint = null;
    });
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) {
    final palette = AppPalette.of(context);
    final displayKey = _captured ?? widget.currentKey;
    final alt = widget.isMacOs ? 'Option' : 'Alt';
    return Focus(
      focusNode: _focusNode,
      autofocus: true,
      onKeyEvent: _handleKey,
      child: AppDialog(
        title: '录制切换中英键',
        animation: widget.animation,
        actions: [
          if (displayKey != 'schema')
            ActionButton(
              '恢复跟随方案',
              onPress: () => Navigator.pop(context, 'schema'),
            ),
          ActionButton('取消', onPress: () => Navigator.pop(context)),
          ActionButton(
            '确定',
            primary: true,
            onPress: _captured == null
                ? null
                : () => Navigator.pop(context, _captured),
          ),
        ],
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          spacing: 12,
          children: [
            Text(
              '直接按下并松开单个修饰键（左/右 Shift、Control、$alt${widget.isMacOs ? '' : '、Caps Lock'}），或按下组合键（如 Ctrl+Space、$alt+Space、F2）。',
              style: context.theme.typography.body.sm.copyWith(
                color: palette.muted,
              ),
            ),
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 20),
              decoration: BoxDecoration(
                color: palette.surface,
                border: Border.all(
                  color: _captured != null
                      ? ChromeAccent.of(context)
                      : palette.hairline,
                ),
                borderRadius: BorderRadius.circular(12),
              ),
              alignment: Alignment.center,
              child: Text(
                _pendingModifier != null
                    ? '松开以确认 ${englishSwitchKeyLabel(_pendingModifier!, isMacOs: widget.isMacOs)}'
                    : _captured != null
                    ? englishSwitchKeyLabel(_captured!, isMacOs: widget.isMacOs)
                    : '等待按键…（当前：${englishSwitchKeyLabel(widget.currentKey, isMacOs: widget.isMacOs, schemaKeys: widget.schemaKeys)}）',
                style: context.theme.typography.body.md.copyWith(
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
            if (_hint case final hint?)
              Text(
                hint,
                style: context.theme.typography.body.xs.copyWith(
                  color: palette.error,
                ),
              ),
          ],
        ),
      ),
    );
  }
}
