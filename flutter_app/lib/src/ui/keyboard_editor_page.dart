import 'dart:convert';
import 'dart:math' as math;

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../rust/api/types.dart';
import 'candidate_panel_preview.dart';
import 'mobile_keyboard_preview.dart';
import 'theme.dart';
import 'widgets.dart';

Future<void> showKeyboardDetailPage(
  BuildContext context, {
  required AppController controller,
  required Map<String, dynamic> previewTheme,
  required String schemeName,
  String? initialKeyboardId,
}) => Navigator.of(context).push<void>(
  MaterialPageRoute(
    builder: (_) => KeyboardDetailPage(
      controller: controller,
      previewTheme: previewTheme,
      schemeName: schemeName,
      initialKeyboardId: initialKeyboardId,
    ),
  ),
);

enum _EditorMode { editKeys, testKeyboard, layoutSettings }

enum _KeyInspectorTab { basic, action, swipe, longPress, extra }

const _commandTypes = <(String, String)>[
  ('', '无 / 跟随默认'),
  ('input', '常规输入 (input)'),
  ('directInput', '直出上屏 (directInput)'),
  ('rimeInput', '发送编码 (rimeInput)'),
  ('backspace', '退格删除 (backspace)'),
  ('backspaceGesture', '退格手势 (backspaceGesture)'),
  ('enter', '回车确认 (enter)'),
  ('space', '空格上屏 (space)'),
  ('shift', '大小写 Shift (shift)'),
  ('mode', '中英切换 (mode)'),
  ('keyboardMode', '切换键盘层 (keyboardMode)'),
  ('edit', '高级编辑命令 (edit)'),
  ('panel', '打开功能面板 (panel)'),
  ('floating', '悬浮键盘 (floating)'),
  ('nextInputMethod', '切换输入法 (nextInputMethod)'),
  ('keyboardPicker', '键盘选择器 (keyboardPicker)'),
  ('previousCandidatePage', '候选上一页 (previousCandidatePage)'),
  ('nextCandidatePage', '候选下一页 (nextCandidatePage)'),
  ('reset', '清空编码 (reset)'),
  ('rimeMenu', '方案选项面板 (rimeMenu)'),
  ('rimeOption', '切换方案选项 (rimeOption)'),
  ('rimeSchema', '切换输入方案 (rimeSchema)'),
  ('openPage', '打开应用页面 (openPage)'),
  ('setting', '快捷设置 (setting)'),
];

const _editVerbs = <(String, String)>[
  ('cursorLeft', '光标左移 (cursorLeft)'),
  ('cursorRight', '光标右移 (cursorRight)'),
  ('cursorUp', '光标上移 (cursorUp)'),
  ('cursorDown', '光标下移 (cursorDown)'),
  ('lineStart', '移至行首 (lineStart)'),
  ('lineEnd', '移至行尾 (lineEnd)'),
  ('selectAll', '全选 (selectAll)'),
  ('selectLeft', '向左选择 (selectLeft)'),
  ('selectRight', '向右选择 (selectRight)'),
  ('toggleSelection', '切换选区模式 (toggleSelection)'),
  ('copy', '复制 (copy)'),
  ('cut', '剪切 (cut)'),
  ('paste', '粘贴 (paste)'),
  ('pasteText', '粘贴固定文本 (pasteText)'),
  ('undo', '撤销 (undo)'),
  ('redo', '重做 (redo)'),
  ('forwardDelete', '向后删除 (forwardDelete)'),
  ('clearAll', '清空内容 (clearAll)'),
  ('tab', '制表符 Tab (tab)'),
  ('repeatCommit', '重复上次上屏 (repeatCommit)'),
];

const _panelTargets = <(String, String)>[
  ('clipboard', '剪贴板历史 (clipboard)'),
  ('phrases', '常用短语 (phrases)'),
  ('rime', '方案与选项 (rime)'),
  ('selection', '光标记选面板 (selection)'),
  ('home', '返回主键盘 (home)'),
  ('close', '关闭面板 (close)'),
];

const _numberRowModes = <(String, String)>[
  ('auto', '跟随设置（自动数字行）'),
  ('embedded', '已内置数字行'),
  ('disabled', '不显示独立数字行'),
];

class KeyboardDetailPage extends StatefulWidget {
  const KeyboardDetailPage({
    super.key,
    required this.controller,
    required this.previewTheme,
    required this.schemeName,
    this.initialKeyboardId,
  });

  final AppController controller;
  final Map<String, dynamic> previewTheme;
  final String schemeName;
  final String? initialKeyboardId;

  @override
  State<KeyboardDetailPage> createState() => _KeyboardDetailPageState();
}

class _KeyboardDetailPageState extends State<KeyboardDetailPage> {
  String _selectedId = '';
  bool _isBuiltIn = true;
  bool _isCurrent = false;
  bool _loading = true;
  bool _busy = false;
  bool _dirty = false;
  String? _status;
  String? _error;

  Map<String, dynamic> _layout = <String, dynamic>{};
  String _layerName = 'letters';
  final List<String> _layerHistory = <String>[];
  _EditorMode _mode = _EditorMode.editKeys;
  _KeyInspectorTab _keyTab = _KeyInspectorTab.basic;
  (int, int)? _selectedKey;

  // Live preview toggles
  bool? _previewDarkOverride;
  bool _asciiMode = false;
  bool _shifted = false;
  bool _showMnemonics = true;
  int _editRevision = 0;

  // Effect test state
  final TextEditingController _testController = TextEditingController();
  final List<String> _testUndoStack = <String>[];
  final List<String> _testRedoStack = <String>[];
  String _lastCommit = '';
  String _testFeedback = '点击、长按或四向滑动预览键盘上的任意按键即可实时测试';
  bool _selectionMode = false;

  AppController get _controller => widget.controller;

  @override
  void initState() {
    super.initState();
    _controller.addListener(_onControllerChanged);
    final initial =
        widget.initialKeyboardId ??
        _controller.keyboards.where((k) => k.current).firstOrNull?.id ??
        _controller.keyboards.firstOrNull?.id ??
        'keytao.qwerty26';
    _loadDocument(initial);
  }

  @override
  void dispose() {
    _controller.removeListener(_onControllerChanged);
    _testController.dispose();
    super.dispose();
  }

  void _onControllerChanged() {
    if (!mounted) return;
    final current = _controller.keyboards
        .where((k) => k.id == _selectedId)
        .firstOrNull;
    if (current != null && current.current != _isCurrent) {
      setState(() => _isCurrent = current.current);
    } else {
      setState(() {});
    }
  }

  Future<void> _loadDocument(String id) async {
    setState(() {
      _loading = true;
      _error = null;
      _status = null;
    });
    try {
      final doc = await _controller.getKeyboardDocument(id);
      final parsed = _deepCopyMap(
        jsonDecode(doc.layoutJson) as Map<String, dynamic>,
      );
      if (!mounted) return;
      final rows = _rowsForLayer(parsed, 'letters');
      setState(() {
        _selectedId = doc.id;
        _isBuiltIn = doc.builtIn;
        _isCurrent = doc.current;
        _layout = parsed;
        _layerName = 'letters';
        _layerHistory.clear();
        _selectedKey = rows.isNotEmpty && rows.first.isNotEmpty ? (0, 0) : null;
        _showMnemonics = doc.hasMnemonics;
        _dirty = false;
        _loading = false;
        _editRevision++;
      });
    } catch (failure) {
      if (!mounted) return;
      setState(() {
        _error = errorText(failure);
        _loading = false;
      });
    }
  }

  static Map<String, dynamic> _deepCopyMap(Map<String, dynamic> source) =>
      jsonDecode(jsonEncode(source)) as Map<String, dynamic>;

  List<List<Map<String, dynamic>>> _rowsForLayer(
    Map<String, dynamic> layout,
    String layer,
  ) {
    final dynamic raw = switch (layer) {
      'letters' => layout['rows'],
      'numbers' => layout['numberRows'],
      'symbols' => layout['symbolRows'],
      _ => (layout['layers'] as Map<String, dynamic>?)?[layer],
    };
    if (raw is! List) return <List<Map<String, dynamic>>>[];
    return [
      for (final row in raw)
        if (row is List)
          [
            for (final key in row)
              if (key is Map) Map<String, dynamic>.from(key),
          ],
    ];
  }

  void _writeRowsForLayer(
    String layer,
    List<List<Map<String, dynamic>>> rows,
  ) {
    if (layer == 'letters') {
      _layout['rows'] = rows;
    } else if (layer == 'numbers') {
      _layout['numberRows'] = rows;
    } else if (layer == 'symbols') {
      _layout['symbolRows'] = rows;
    } else {
      final layers = Map<String, dynamic>.from(
        (_layout['layers'] as Map?) ?? const {},
      );
      layers[layer] = rows;
      _layout['layers'] = layers;
    }
  }

  List<String> get _allLayerNames {
    final custom = ((_layout['layers'] as Map?)?.keys ?? const <dynamic>[])
        .map((k) => k.toString())
        .where((k) => k.isNotEmpty && k != 'letters' && k != 'numbers' && k != 'symbols')
        .toList()
      ..sort();
    return ['letters', 'numbers', 'symbols', ...custom];
  }

  String _layerLabel(String layer) => switch (layer) {
    'letters' => '主键盘',
    'numbers' => '数字层',
    'symbols' => '符号层',
    _ => layer,
  };

  Map<String, dynamic>? get _selectedKeyMap {
    final sel = _selectedKey;
    if (sel == null) return null;
    final rows = _rowsForLayer(_layout, _layerName);
    if (sel.$1 < 0 || sel.$1 >= rows.length) return null;
    if (sel.$2 < 0 || sel.$2 >= rows[sel.$1].length) return null;
    return rows[sel.$1][sel.$2];
  }

  void _mutateLayout(VoidCallback mutation) {
    setState(() {
      mutation();
      _recomputeHasMnemonics();
      _dirty = true;
      _status = null;
      _error = null;
      _editRevision++;
    });
  }

  void _mutateSelectedKey(void Function(Map<String, dynamic> key) mutation) {
    final sel = _selectedKey;
    if (sel == null) return;
    _mutateLayout(() {
      final rows = _rowsForLayer(_layout, _layerName);
      if (sel.$1 < 0 ||
          sel.$1 >= rows.length ||
          sel.$2 < 0 ||
          sel.$2 >= rows[sel.$1].length) {
        return;
      }
      final key = Map<String, dynamic>.from(rows[sel.$1][sel.$2]);
      mutation(key);
      rows[sel.$1][sel.$2] = key;
      _writeRowsForLayer(_layerName, rows);
    });
  }

  void _recomputeHasMnemonics() {
    bool hasInRows(List<List<Map<String, dynamic>>> rows) => rows.any(
      (row) => row.any((k) {
        final m = k['mnemonic'];
        if (m is! Map) return false;
        final top = (m['top'] as String?)?.trim() ?? '';
        final topBadge = (m['topBadge'] as String?)?.trim() ?? '';
        final bottom = (m['bottomLines'] as List?) ?? const [];
        return top.isNotEmpty ||
            topBadge.isNotEmpty ||
            bottom.any((l) => l.toString().trim().isNotEmpty);
      }),
    );
    var found = hasInRows(_rowsForLayer(_layout, 'letters'));
    if (!found && _layout['layers'] is Map) {
      for (final name in (_layout['layers'] as Map).keys) {
        if (hasInRows(_rowsForLayer(_layout, name.toString()))) {
          found = true;
          break;
        }
      }
    }
    _layout['hasMnemonics'] = found;
  }

  Map<String, dynamic> _effectivePreviewTheme(BuildContext context) {
    final s = _controller.uiSettings;
    final systemDark =
        MediaQuery.platformBrightnessOf(context) == Brightness.dark;
    if (_previewDarkOverride == null) return widget.previewTheme;
    final accent =
        s?.accentColor ??
        '#${(ChromeAccent.of(context).toARGB32() & 0xFFFFFF).toRadixString(16).padLeft(6, '0')}';
    return _controller.cachedCandidatePanelPreview(
      colorScheme: _previewDarkOverride!
          ? UiColorSchemeDto.dark
          : UiColorSchemeDto.light,
      accentColor: accent,
      cornerRadius: s?.cornerRadius,
      backgroundColor: s?.backgroundColor ?? '',
      fontSize: s?.fontSize ?? 18,
      systemDark: systemDark,
    );
  }

  Future<void> _perform(Future<void> Function() action) async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await action();
    } catch (failure) {
      if (mounted) setState(() => _error = errorText(failure));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _save({bool apply = true}) => _perform(() async {
    final saved = await _controller.saveKeyboardDocument(
      _selectedId,
      jsonEncode(_layout),
      apply: apply,
    );
    if (!mounted) return;
    final wasForked = saved.id != _selectedId;
    await _loadDocument(saved.id);
    if (!mounted) return;
    setState(() {
      _status = wasForked
          ? '已保存为自定义键盘「${saved.name}」${saved.current ? '并启用' : ''}'
          : '已保存「${saved.name}」';
    });
  });

  Future<void> _applyCurrent() => _perform(() async {
    if (await _controller.applyKeyboardProfile(_selectedId) && mounted) {
      setState(() {
        _isCurrent = true;
        _status = '已启用当前键盘';
      });
    }
  });

  Future<void> _duplicate() => _perform(() async {
    final dup = await _controller.duplicateKeyboardProfile(_selectedId);
    if (!mounted) return;
    await _loadDocument(dup.id);
    if (!mounted) return;
    setState(() => _status = '已创建副本「${dup.name}」');
  });

  Future<void> _rename() => _perform(() async {
    final textController = TextEditingController(
      text: (_layout['name'] as String?) ?? '',
    );
    final nextName = await showFDialog<String>(
      context: context,
      builder: (context, _, animation) => AppDialog(
        title: '重命名键盘',
        animation: animation,
        actions: [
          ActionButton('取消', onPress: () => Navigator.pop(context)),
          ActionButton(
            '确定',
            primary: true,
            onPress: () => Navigator.pop(context, textController.text.trim()),
          ),
        ],
        child: TextField(
          controller: textController,
          autofocus: true,
          decoration: const InputDecoration(
            labelText: '键盘名称',
            border: OutlineInputBorder(),
          ),
        ),
      ),
    );
    textController.dispose();
    if (nextName == null || nextName.isEmpty) return;
    final renamed = await _controller.renameKeyboardProfile(
      _selectedId,
      nextName,
    );
    if (!mounted) return;
    setState(() {
      _layout['name'] = renamed.name;
      _status = '已重命名为「${renamed.name}」';
    });
  });

  Future<void> _delete() => _perform(() async {
    final name = (_layout['name'] as String?) ?? _selectedId;
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
        child: Text('确定删除「$name」吗？此操作无法撤销。'),
      ),
    );
    if (confirmed != true) return;
    if (await _controller.deleteKeyboardProfile(_selectedId) && mounted) {
      final fallback =
          _controller.keyboards.where((k) => k.current).firstOrNull?.id ??
          _controller.keyboards.firstOrNull?.id ??
          'keytao.qwerty26';
      await _loadDocument(fallback);
      if (mounted) setState(() => _status = '已删除「$name」');
    }
  });

  Future<void> _export({bool copy = false}) => _perform(() async {
    final exported = await _controller.exportKeyboardProfile(_selectedId);
    if (!mounted) return;
    if (copy && !exported.useFile) {
      await Clipboard.setData(ClipboardData(text: exported.shareString));
      if (mounted) setState(() => _status = '键盘分享串已复制');
      return;
    }
    await _controller.shareKeyboardFile(exported);
    if (mounted) setState(() => _status = '已导出 ${exported.fileName}');
  });

  Future<void> _pasteImport() => _perform(() async {
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    final text = data?.text;
    if (text == null || text.trim().isEmpty) {
      throw StateError('剪贴板为空');
    }
    await _importText(text);
  });

  Future<void> _fileImport() => _perform(() async {
    final file = await openFile();
    if (file == null) return;
    if (await file.length() > 384 * 1024) {
      throw StateError('键盘配置文件超过 384 KiB 上限');
    }
    await _importText(await file.readAsString());
  });

  Future<void> _importText(String text) async {
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
          child: Text('键盘「${preview.name}」已存在，请选择覆盖或另存为副本。'),
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
    await _loadDocument(result.id);
    if (mounted) setState(() => _status = '已导入「${result.name}」');
  }

  Future<void> _addLayerDialog() async {
    final textController = TextEditingController();
    final name = await showFDialog<String>(
      context: context,
      builder: (context, _, animation) => AppDialog(
        title: '新建自定义键盘层',
        animation: animation,
        actions: [
          ActionButton('取消', onPress: () => Navigator.pop(context)),
          ActionButton(
            '创建',
            primary: true,
            onPress: () => Navigator.pop(context, textController.text.trim()),
          ),
        ],
        child: TextField(
          controller: textController,
          autofocus: true,
          decoration: const InputDecoration(
            labelText: '层名称（如 custom_symbols / numpad）',
            border: OutlineInputBorder(),
          ),
        ),
      ),
    );
    textController.dispose();
    if (name == null || name.isEmpty) return;
    if (const {'letters', 'numbers', 'symbols'}.contains(name)) {
      setState(() => _error = '不能与内置层名称冲突');
      return;
    }
    _mutateLayout(() {
      final layers = Map<String, dynamic>.from(
        (_layout['layers'] as Map?) ?? const {},
      );
      layers.putIfAbsent(
        name,
        () => [
          [
            {'label': '1', 'value': '1'},
            {'label': '2', 'value': '2'},
            {'label': '3', 'value': '3'},
            {
              'label': '返回',
              'action': {'type': 'keyboardMode', 'value': 'back'},
            },
          ],
        ],
      );
      _layout['layers'] = layers;
      _layerName = name;
      _selectedKey = (0, 0);
    });
  }

  void _deleteCustomLayer(String layer) {
    if (const {'letters', 'numbers', 'symbols'}.contains(layer)) return;
    _mutateLayout(() {
      final layers = Map<String, dynamic>.from(
        (_layout['layers'] as Map?) ?? const {},
      );
      layers.remove(layer);
      _layout['layers'] = layers;
      _layerName = 'letters';
      final rows = _rowsForLayer(_layout, 'letters');
      _selectedKey = rows.isNotEmpty && rows.first.isNotEmpty ? (0, 0) : null;
    });
  }

  Future<void> _openRawJsonDialog() async {
    final textController = TextEditingController(
      text: const JsonEncoder.withIndent('  ').convert(_layout),
    );
    String? dialogError;
    final applied = await showFDialog<Map<String, dynamic>>(
      context: context,
      builder: (context, _, animation) => StatefulBuilder(
        builder: (context, setDialogState) => AppDialog(
          title: '编辑原始布局 JSON',
          animation: animation,
          constraints: const BoxConstraints(maxWidth: 680, maxHeight: 620),
          actions: [
            ActionButton('取消', onPress: () => Navigator.pop(context)),
            ActionButton(
              '应用到预览',
              primary: true,
              onPress: () {
                try {
                  final decoded =
                      jsonDecode(textController.text) as Map<String, dynamic>;
                  if (decoded['rows'] is! List ||
                      (decoded['rows'] as List).isEmpty) {
                    setDialogState(() => dialogError = '必须包含非空 rows 字段');
                    return;
                  }
                  Navigator.pop(context, decoded);
                } catch (e) {
                  setDialogState(() => dialogError = 'JSON 解析失败：$e');
                }
              },
            ),
          ],
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            spacing: 8,
            children: [
              if (dialogError != null) ErrorMessage(dialogError!),
              SizedBox(
                height: 360,
                child: TextField(
                  controller: textController,
                  maxLines: null,
                  expands: true,
                  style: const TextStyle(fontFamily: 'monospace', fontSize: 12),
                  decoration: const InputDecoration(
                    border: OutlineInputBorder(),
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
    textController.dispose();
    if (applied != null) {
      _mutateLayout(() {
        _layout = _deepCopyMap(applied);
        _layerName = 'letters';
        final rows = _rowsForLayer(_layout, 'letters');
        _selectedKey = rows.isNotEmpty && rows.first.isNotEmpty ? (0, 0) : null;
      });
    }
  }

  // ---------------------------------------------------------------------------
  // Row / Key Structural Operations
  // ---------------------------------------------------------------------------

  void _insertKey({required bool after}) {
    final sel = _selectedKey;
    if (sel == null) return;
    _mutateLayout(() {
      final rows = _rowsForLayer(_layout, _layerName);
      if (sel.$1 >= rows.length) return;
      final index = after ? sel.$2 + 1 : sel.$2;
      rows[sel.$1].insert(index, <String, dynamic>{
        'label': '新',
        'value': '新',
      });
      _writeRowsForLayer(_layerName, rows);
      _selectedKey = (sel.$1, index);
    });
  }

  void _duplicateSelectedKey() {
    final sel = _selectedKey;
    if (sel == null) return;
    _mutateLayout(() {
      final rows = _rowsForLayer(_layout, _layerName);
      if (sel.$1 >= rows.length || sel.$2 >= rows[sel.$1].length) return;
      final copy = _deepCopyMap(rows[sel.$1][sel.$2]);
      rows[sel.$1].insert(sel.$2 + 1, copy);
      _writeRowsForLayer(_layerName, rows);
      _selectedKey = (sel.$1, sel.$2 + 1);
    });
  }

  void _moveSelectedKey(int delta) {
    final sel = _selectedKey;
    if (sel == null) return;
    _mutateLayout(() {
      final rows = _rowsForLayer(_layout, _layerName);
      if (sel.$1 >= rows.length) return;
      final row = rows[sel.$1];
      final nextCol = sel.$2 + delta;
      if (nextCol < 0 || nextCol >= row.length) return;
      final item = row.removeAt(sel.$2);
      row.insert(nextCol, item);
      _writeRowsForLayer(_layerName, rows);
      _selectedKey = (sel.$1, nextCol);
    });
  }

  void _deleteSelectedKey() {
    final sel = _selectedKey;
    if (sel == null) return;
    _mutateLayout(() {
      final rows = _rowsForLayer(_layout, _layerName);
      if (sel.$1 >= rows.length || rows[sel.$1].length <= 1) return;
      rows[sel.$1].removeAt(sel.$2);
      _writeRowsForLayer(_layerName, rows);
      _selectedKey = (sel.$1, math.min(sel.$2, rows[sel.$1].length - 1));
    });
  }

  void _insertRow({required bool below}) {
    final sel = _selectedKey;
    final rows = _rowsForLayer(_layout, _layerName);
    final insertAt = sel == null ? rows.length : (below ? sel.$1 + 1 : sel.$1);
    _mutateLayout(() {
      rows.insert(insertAt, <Map<String, dynamic>>[
        <String, dynamic>{'label': 'a', 'value': 'a'},
        <String, dynamic>{'label': 'b', 'value': 'b'},
        <String, dynamic>{'label': 'c', 'value': 'c'},
      ]);
      _writeRowsForLayer(_layerName, rows);
      _selectedKey = (insertAt, 0);
    });
  }

  void _deleteSelectedRow() {
    final sel = _selectedKey;
    if (sel == null) return;
    _mutateLayout(() {
      final rows = _rowsForLayer(_layout, _layerName);
      if (rows.length <= 1 || sel.$1 >= rows.length) return;
      rows.removeAt(sel.$1);
      _writeRowsForLayer(_layerName, rows);
      final nextRow = math.min(sel.$1, rows.length - 1);
      _selectedKey = (nextRow, 0);
    });
  }

  // ---------------------------------------------------------------------------
  // Interactive Effect Test Engine
  // ---------------------------------------------------------------------------

  void _pushUndoSnapshot() {
    if (_testUndoStack.isEmpty || _testUndoStack.last != _testController.text) {
      _testUndoStack.add(_testController.text);
      if (_testUndoStack.length > 50) _testUndoStack.removeAt(0);
    }
    _testRedoStack.clear();
  }

  void _insertTextAtSelection(String text) {
    if (text.isEmpty) return;
    _pushUndoSnapshot();
    final value = _testController.value;
    final sel = value.selection;
    final start = sel.isValid ? sel.start : value.text.length;
    final end = sel.isValid ? sel.end : value.text.length;
    final nextText = value.text.replaceRange(start, end, text);
    _lastCommit = text;
    _testController.value = TextEditingValue(
      text: nextText,
      selection: TextSelection.collapsed(offset: start + text.length),
    );
  }

  void _handleTestGesture(int row, int col, KeyboardPreviewGesture gesture) {
    final rows = _rowsForLayer(_layout, _layerName);
    if (row < 0 || row >= rows.length || col < 0 || col >= rows[row].length) {
      return;
    }
    final key = rows[row][col];
    final gestureLabel = switch (gesture) {
      KeyboardPreviewGesture.tap => '点击',
      KeyboardPreviewGesture.longPress => '长按',
      KeyboardPreviewGesture.swipeUp => '上滑',
      KeyboardPreviewGesture.swipeDown => '下滑',
      KeyboardPreviewGesture.swipeLeft => '左滑',
      KeyboardPreviewGesture.swipeRight => '右滑',
    };

    if (gesture == KeyboardPreviewGesture.longPress) {
      final altsRaw = _asciiMode
          ? (key['asciiAlternates'] ?? key['alternates'])
          : key['alternates'];
      if (altsRaw is List && altsRaw.isNotEmpty) {
        _showAlternatesTestDialog(
          altsRaw.whereType<Map>().map(Map<String, dynamic>.from).toList(),
        );
        setState(() => _testFeedback = '$gestureLabel · 打开长按候选 (${altsRaw.length} 项)');
        return;
      }
      final cmd = _commandMap(_asciiMode ? (key['asciiLongPress'] ?? key['longPress']) : key['longPress']);
      if (cmd != null) {
        _executeCommand(cmd, gestureLabel, key);
        return;
      }
    }

    if (gesture == KeyboardPreviewGesture.swipeUp) {
      final cmd = _commandMap(key['swipeUp']);
      if (cmd != null) {
        _executeCommand(cmd, gestureLabel, key);
        return;
      }
      final hint = (key['hint'] as String?)?.trim() ?? '';
      if (hint.isNotEmpty) {
        _insertTextAtSelection(hint);
        setState(() => _testFeedback = '$gestureLabel · 上滑提示「$hint」');
        return;
      }
      setState(() => _testFeedback = '$gestureLabel · 未配置上滑动作');
      return;
    }

    if (gesture == KeyboardPreviewGesture.swipeDown) {
      final cmd = _commandMap(key['swipeDown']);
      if (cmd != null) {
        _executeCommand(cmd, gestureLabel, key);
      } else {
        setState(() => _testFeedback = '$gestureLabel · 未配置下滑动作');
      }
      return;
    }

    if (gesture == KeyboardPreviewGesture.swipeLeft) {
      final cmd = _commandMap(key['swipeLeft']);
      if (cmd != null) {
        _executeCommand(cmd, gestureLabel, key);
      } else {
        setState(() => _testFeedback = '$gestureLabel · 未配置左滑动作');
      }
      return;
    }

    if (gesture == KeyboardPreviewGesture.swipeRight) {
      final cmd = _commandMap(key['swipeRight']);
      if (cmd != null) {
        _executeCommand(cmd, gestureLabel, key);
      } else {
        setState(() => _testFeedback = '$gestureLabel · 未配置右滑动作');
      }
      return;
    }

    // Tap
    final stack = key['stack'];
    if (stack is List && stack.isNotEmpty) {
      final items = stack.whereType<Map>().map(Map<String, dynamic>.from).toList();
      _showAlternatesTestDialog(items, title: '选择堆叠子键');
      setState(() => _testFeedback = '$gestureLabel · 堆叠键 (${items.length} 项)');
      return;
    }
    final action = _commandMap(
      _asciiMode ? (key['asciiAction'] ?? key['action']) : key['action'],
    );
    if (action != null) {
      _executeCommand(action, gestureLabel, key);
      return;
    }
    final rawText = _asciiMode
        ? ((key['asciiValue'] as String?) ??
              (key['asciiLabel'] as String?) ??
              (key['value'] as String?) ??
              (key['label'] as String?) ??
              '')
        : ((key['value'] as String?) ?? (key['label'] as String?) ?? '');
    final output = _shifted && rawText.length == 1
        ? rawText.toUpperCase()
        : rawText;
    _insertTextAtSelection(output);
    setState(() {
      if (_shifted) _shifted = false;
      _testFeedback = '$gestureLabel · 输入「$output」';
    });
  }

  Map<String, dynamic>? _commandMap(dynamic raw) =>
      raw is Map ? Map<String, dynamic>.from(raw) : null;

  Future<void> _showAlternatesTestDialog(
    List<Map<String, dynamic>> items, {
    String title = '长按候选测试',
  }) async {
    final chosen = await showFDialog<Map<String, dynamic>>(
      context: context,
      builder: (context, _, animation) => AppDialog(
        title: title,
        animation: animation,
        actions: [
          ActionButton('关闭', onPress: () => Navigator.pop(context)),
        ],
        child: Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            for (final item in items)
              ActionButton(
                (item['label'] as String?) ?? (item['value'] as String?) ?? '?',
                onPress: () => Navigator.pop(context, item),
              ),
          ],
        ),
      ),
    );
    if (chosen == null || !mounted) return;
    final cmd = _commandMap(chosen['action']);
    if (cmd != null) {
      _executeCommand(cmd, '候选', chosen);
    } else {
      final text =
          (chosen['value'] as String?) ?? (chosen['label'] as String?) ?? '';
      _insertTextAtSelection(text);
      setState(() => _testFeedback = '长按候选 · 输入「$text」');
    }
  }

  void _executeCommand(
    Map<String, dynamic> cmd,
    String gestureLabel,
    Map<String, dynamic> key,
  ) {
    final type = (cmd['type'] as String?) ?? '';
    final value = (cmd['value'] as String?) ?? '';
    switch (type) {
      case 'input':
      case 'directInput':
      case 'rimeInput':
        final text = value.isNotEmpty
            ? value
            : ((key['value'] as String?) ?? (key['label'] as String?) ?? '');
        final output = _shifted && text.length == 1 ? text.toUpperCase() : text;
        _insertTextAtSelection(output);
        setState(() {
          if (_shifted) _shifted = false;
          _testFeedback = '$gestureLabel · $type("$output")';
        });
      case 'space':
        _insertTextAtSelection(' ');
        setState(() => _testFeedback = '$gestureLabel · 空格');
      case 'enter':
        _insertTextAtSelection('\n');
        setState(() => _testFeedback = '$gestureLabel · 回车');
      case 'backspace':
      case 'backspaceGesture':
        _backspaceTest();
        setState(() => _testFeedback = '$gestureLabel · 退格 ($type)');
      case 'shift':
        setState(() {
          _shifted = !_shifted;
          _testFeedback = '$gestureLabel · Shift ${_shifted ? "开启" : "关闭"}';
        });
      case 'mode':
        setState(() {
          _asciiMode = !_asciiMode;
          _testFeedback = '$gestureLabel · 切换为 ${_asciiMode ? "英文" : "中文"}';
        });
      case 'keyboardMode':
        _switchPreviewLayer(value);
        setState(() => _testFeedback = '$gestureLabel · 切换键盘层 → $_layerName');
      case 'edit':
        _executeEditVerb(value, gestureLabel);
      default:
        setState(
          () => _testFeedback =
              '$gestureLabel · 触发原生指令 $type${value.isEmpty ? '' : '($value)'}',
        );
    }
  }

  void _switchPreviewLayer(String target) {
    setState(() {
      if (target == 'back') {
        _layerName = _layerHistory.isNotEmpty ? _layerHistory.removeLast() : 'letters';
      } else {
        final resolved = target == 'symbols:last'
            ? 'symbols'
            : (target.isEmpty ? 'letters' : target);
        if (_allLayerNames.contains(resolved)) {
          if (_layerName != resolved) _layerHistory.add(_layerName);
          _layerName = resolved;
        }
      }
      final rows = _rowsForLayer(_layout, _layerName);
      _selectedKey = rows.isNotEmpty && rows.first.isNotEmpty ? (0, 0) : null;
    });
  }

  void _backspaceTest() {
    final v = _testController.value;
    final sel = v.selection;
    if (!sel.isValid) return;
    _pushUndoSnapshot();
    if (!sel.isCollapsed) {
      _testController.value = TextEditingValue(
        text: v.text.replaceRange(sel.start, sel.end, ''),
        selection: TextSelection.collapsed(offset: sel.start),
      );
    } else if (sel.start > 0) {
      _testController.value = TextEditingValue(
        text: v.text.replaceRange(sel.start - 1, sel.start, ''),
        selection: TextSelection.collapsed(offset: sel.start - 1),
      );
    }
  }

  Future<void> _executeEditVerb(String verb, String gestureLabel) async {
    final v = _testController.value;
    final text = v.text;
    final sel = v.selection.isValid
        ? v.selection
        : TextSelection.collapsed(offset: text.length);

    void moveCursor(int nextOffset, {bool extend = false}) {
      final clamped = nextOffset.clamp(0, text.length);
      _testController.selection = extend || _selectionMode
          ? TextSelection(baseOffset: sel.baseOffset, extentOffset: clamped)
          : TextSelection.collapsed(offset: clamped);
    }

    switch (verb) {
      case 'cursorLeft':
        moveCursor(sel.extentOffset - 1);
      case 'cursorRight':
        moveCursor(sel.extentOffset + 1);
      case 'cursorUp':
        moveCursor(0);
      case 'cursorDown':
        moveCursor(text.length);
      case 'lineStart':
        final prevNl = text.lastIndexOf('\n', math.max(0, sel.extentOffset - 1));
        moveCursor(prevNl < 0 ? 0 : prevNl + 1);
      case 'lineEnd':
        final nextNl = text.indexOf('\n', sel.extentOffset);
        moveCursor(nextNl < 0 ? text.length : nextNl);
      case 'selectAll':
        _testController.selection = TextSelection(
          baseOffset: 0,
          extentOffset: text.length,
        );
      case 'selectLeft':
        moveCursor(sel.extentOffset - 1, extend: true);
      case 'selectRight':
        moveCursor(sel.extentOffset + 1, extend: true);
      case 'toggleSelection':
        _selectionMode = !_selectionMode;
      case 'copy':
        final selected = sel.isCollapsed
            ? text
            : text.substring(sel.start, sel.end);
        await Clipboard.setData(ClipboardData(text: selected));
      case 'cut':
        final selected = sel.isCollapsed
            ? text
            : text.substring(sel.start, sel.end);
        await Clipboard.setData(ClipboardData(text: selected));
        _pushUndoSnapshot();
        if (sel.isCollapsed) {
          _testController.clear();
        } else {
          _testController.value = TextEditingValue(
            text: text.replaceRange(sel.start, sel.end, ''),
            selection: TextSelection.collapsed(offset: sel.start),
          );
        }
      case 'paste':
        final data = await Clipboard.getData(Clipboard.kTextPlain);
        if (data?.text case final clip? when clip.isNotEmpty) {
          _insertTextAtSelection(clip);
        }
      case 'clearAll':
        _pushUndoSnapshot();
        _testController.clear();
      case 'forwardDelete':
        if (sel.start < text.length) {
          _pushUndoSnapshot();
          _testController.value = TextEditingValue(
            text: text.replaceRange(sel.start, sel.start + 1, ''),
            selection: TextSelection.collapsed(offset: sel.start),
          );
        }
      case 'tab':
        _insertTextAtSelection('\t');
      case 'repeatCommit':
        if (_lastCommit.isNotEmpty) _insertTextAtSelection(_lastCommit);
      case 'undo':
        if (_testUndoStack.isNotEmpty) {
          _testRedoStack.add(text);
          final prev = _testUndoStack.removeLast();
          _testController.value = TextEditingValue(
            text: prev,
            selection: TextSelection.collapsed(offset: prev.length),
          );
        }
      case 'redo':
        if (_testRedoStack.isNotEmpty) {
          _testUndoStack.add(text);
          final next = _testRedoStack.removeLast();
          _testController.value = TextEditingValue(
            text: next,
            selection: TextSelection.collapsed(offset: next.length),
          );
        }
    }
    if (mounted) {
      setState(() => _testFeedback = '$gestureLabel · edit($verb)');
    }
  }

  // ---------------------------------------------------------------------------
  // Build UI
  // ---------------------------------------------------------------------------

  @override
  Widget build(BuildContext context) {
    final palette = AppPalette.of(context);
    final accent = ChromeAccent.of(context);
    final keyboards = _controller.keyboards;
    final recommended = _controller.recommendedKeyboard;
    final previewTheme = _effectivePreviewTheme(context);
    final isDarkPreview =
        (previewTheme['ui'] as Map<String, dynamic>?)?['effectiveColorScheme'] ==
        'dark';
    final hasMnemonics = _layout['hasMnemonics'] == true;

    return Scaffold(
      key: const ValueKey('keyboard-detail-page'),
      backgroundColor: palette.surface,
      body: SafeArea(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            // Top bar
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
              decoration: BoxDecoration(
                border: Border(bottom: BorderSide(color: palette.hairline)),
              ),
              child: Row(
                spacing: 8,
                children: [
                  FButton.icon(
                    variant: FButtonVariant.ghost,
                    size: FButtonSizeVariant.sm,
                    onPress: () => Navigator.of(context).pop(),
                    child: const Icon(FLucideIcons.arrowLeft, size: 18),
                  ),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(
                          spacing: 6,
                          children: [
                            Flexible(
                              child: Text(
                                (_layout['name'] as String?) ?? '键盘详情与编辑',
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: context.theme.typography.body.md.copyWith(fontWeight: FontWeight.w600),
                              ),
                            ),
                            if (_dirty)
                              _SmallTag('未保存', accent)
                            else if (_isBuiltIn)
                              _SmallTag('内置', palette.muted)
                            else
                              _SmallTag('自定义', accent),
                          ],
                        ),
                      ],
                    ),
                  ),
                  if (!_isCurrent && !_dirty)
                    ActionButton(
                      '启用',
                      onPress: _busy || _loading ? null : _applyCurrent,
                    ),
                  ActionButton(
                    _isBuiltIn ? '保存为自定义' : '保存',
                    primary: true,
                    onPress: _busy || _loading || !_dirty
                        ? null
                        : () => _save(apply: true),
                  ),
                ],
              ),
            ),
            if (_loading)
              const Expanded(child: Center(child: FProgress()))
            else
              Expanded(
                child: ListView(
                  padding: const EdgeInsets.fromLTRB(12, 10, 12, 24),
                  children: [
                    // Keyboard switcher strip
                    SizedBox(
                      height: 36,
                      child: ListView.separated(
                        scrollDirection: Axis.horizontal,
                        itemCount: keyboards.length,
                        separatorBuilder: (_, _) => const SizedBox(width: 6),
                        itemBuilder: (context, index) {
                          final kb = keyboards[index];
                          final selected = kb.id == _selectedId;
                          final isRec = recommended?.id == kb.id;
                          return FTappable(
                            key: ValueKey('detail-kb-chip-${kb.id}'),
                            onPress: selected ? null : () => _loadDocument(kb.id),
                            child: Container(
                              padding: const EdgeInsets.symmetric(
                                horizontal: 10,
                                vertical: 6,
                              ),
                              decoration: BoxDecoration(
                                color: selected
                                    ? accent.withValues(alpha: 0.14)
                                    : palette.surface,
                                borderRadius: BorderRadius.circular(8),
                                border: Border.all(
                                  color: selected ? accent : palette.hairline,
                                  width: selected ? 1.5 : 1,
                                ),
                              ),
                              child: Row(
                                mainAxisSize: MainAxisSize.min,
                                spacing: 5,
                                children: [
                                  if (kb.current)
                                    Icon(
                                      FLucideIcons.check,
                                      size: 13,
                                      color: accent,
                                    ),
                                  Text(
                                    kb.name,
                                    style: TextStyle(
                                      fontSize: 12,
                                      fontWeight: selected
                                          ? FontWeight.w600
                                          : FontWeight.w500,
                                      color: selected ? accent : palette.ink,
                                    ),
                                  ),
                                  if (isRec) _SmallTag('推荐', accent),
                                ],
                              ),
                            ),
                          );
                        },
                      ),
                    ),
                    const SizedBox(height: 8),
                    // Quick management toolbar
                    Wrap(
                      spacing: 6,
                      runSpacing: 6,
                      children: [
                        _ActionChip(
                          '复制副本',
                          FLucideIcons.copyPlus,
                          _busy ? null : _duplicate,
                        ),
                        if (!_isBuiltIn)
                          _ActionChip(
                            '重命名',
                            FLucideIcons.pencil,
                            _busy ? null : _rename,
                          ),
                        _ActionChip(
                          '复制分享串',
                          FLucideIcons.copy,
                          _busy ? null : () => _export(copy: true),
                        ),
                        _ActionChip(
                          '导出文件',
                          FLucideIcons.download,
                          _busy ? null : () => _export(),
                        ),
                        _ActionChip(
                          '粘贴导入',
                          FLucideIcons.clipboardPaste,
                          _busy ? null : _pasteImport,
                        ),
                        _ActionChip(
                          '导入文件',
                          FLucideIcons.upload,
                          _busy ? null : _fileImport,
                        ),
                        _ActionChip(
                          '原始 JSON',
                          FLucideIcons.code,
                          _busy ? null : _openRawJsonDialog,
                        ),
                        if (!_isBuiltIn)
                          _ActionChip(
                            '删除',
                            FLucideIcons.trash2,
                            _busy ? null : _delete,
                          ),
                      ],
                    ),
                    if (_status case final msg?) ...[
                      const SizedBox(height: 8),
                      StatusMessage(msg, icon: FLucideIcons.circleCheck),
                    ],
                    if (_error case final msg?) ...[
                      const SizedBox(height: 8),
                      ErrorMessage(msg),
                    ],
                    const SizedBox(height: 10),
                    // Mode switcher + Preview toggles
                    Wrap(
                      spacing: 6,
                      runSpacing: 6,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        _ModeChip(
                          key: const ValueKey('mode-edit-keys'),
                          label: '编辑按键',
                          icon: FLucideIcons.slidersHorizontal,
                          selected: _mode == _EditorMode.editKeys,
                          onTap: () =>
                              setState(() => _mode = _EditorMode.editKeys),
                        ),
                        _ModeChip(
                          key: const ValueKey('mode-test-keyboard'),
                          label: '效果测试',
                          icon: FLucideIcons.play,
                          selected: _mode == _EditorMode.testKeyboard,
                          onTap: () =>
                              setState(() => _mode = _EditorMode.testKeyboard),
                        ),
                        _ModeChip(
                          key: const ValueKey('mode-layout-settings'),
                          label: '整体布局',
                          icon: FLucideIcons.layoutGrid,
                          selected: _mode == _EditorMode.layoutSettings,
                          onTap: () => setState(
                            () => _mode = _EditorMode.layoutSettings,
                          ),
                        ),
                        const SizedBox(width: 4),
                        _TogglePill(
                          label: isDarkPreview ? '深色' : '浅色',
                          active: isDarkPreview,
                          onTap: () => setState(
                            () => _previewDarkOverride = !isDarkPreview,
                          ),
                        ),
                        _TogglePill(
                          label: _asciiMode ? '英文' : '中文',
                          active: _asciiMode,
                          onTap: () =>
                              setState(() => _asciiMode = !_asciiMode),
                        ),
                        _TogglePill(
                          label: 'Shift',
                          active: _shifted,
                          onTap: () => setState(() => _shifted = !_shifted),
                        ),
                        if (hasMnemonics)
                          _TogglePill(
                            label: '助记',
                            active: _showMnemonics,
                            onTap: () => setState(
                              () => _showMnemonics = !_showMnemonics,
                            ),
                          ),
                      ],
                    ),
                    const SizedBox(height: 8),
                    // Layer switcher row
                    Wrap(
                      spacing: 6,
                      runSpacing: 6,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        for (final layer in _allLayerNames)
                          _LayerChip(
                            key: ValueKey('layer-chip-$layer'),
                            label: _layerLabel(layer),
                            selected: _layerName == layer,
                            onTap: () {
                              final rows = _rowsForLayer(_layout, layer);
                              setState(() {
                                _layerName = layer;
                                _selectedKey =
                                    rows.isNotEmpty && rows.first.isNotEmpty
                                    ? (0, 0)
                                    : null;
                              });
                            },
                            onDelete:
                                const {
                                  'letters',
                                  'numbers',
                                  'symbols',
                                }.contains(layer)
                                ? null
                                : () => _deleteCustomLayer(layer),
                          ),
                        _ActionChip('新建层', FLucideIcons.plus, _addLayerDialog),
                      ],
                    ),
                    const SizedBox(height: 10),
                    // Test input box (shown in Effect Test mode above preview)
                    if (_mode == _EditorMode.testKeyboard) ...[
                      _buildTestArea(context, palette, accent),
                      const SizedBox(height: 8),
                    ],
                    // Full Live Preview Keyboard
                    Container(
                      key: const ValueKey('keyboard-detail-live-preview'),
                      padding: const EdgeInsets.all(6),
                      decoration: BoxDecoration(
                        borderRadius: BorderRadius.circular(12),
                        border: Border.all(color: palette.hairline),
                      ),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        spacing: 6,
                        children: [
                          Row(
                            children: [
                              Expanded(
                                child: Text(
                                  _mode == _EditorMode.editKeys
                                      ? '点击下方键盘上的任意按键即可选中并实时编辑'
                                      : _mode == _EditorMode.testKeyboard
                                      ? '支持点击、长按（候选弹窗）和上下左右滑动实时测试'
                                      : '修改整体参数与分组配色后立即实时预览',
                                  style: context.theme.typography.body.xs
                                      .copyWith(color: palette.muted),
                                ),
                              ),
                              Text(
                                '实时预览 · ${_layerLabel(_layerName)}',
                                style: context.theme.typography.body.xs
                                    .copyWith(
                                      color: accent,
                                      fontWeight: FontWeight.w600,
                                    ),
                              ),
                            ],
                          ),
                          ClipRRect(
                            borderRadius: BorderRadius.circular(8),
                            child: MobileKeyboardPreview(
                              key: ValueKey(
                                'live-preview-$_selectedId-$_layerName-$_editRevision',
                              ),
                              theme: previewTheme,
                              schemeName: widget.schemeName,
                              layoutOverride: _layout,
                              layerName: _layerName,
                              asciiMode: _asciiMode,
                              shifted: _shifted,
                              mnemonicHintsOverride: _showMnemonics,
                              selectedKey: _mode == _EditorMode.editKeys
                                  ? _selectedKey
                                  : null,
                              selectedKeyColor: accent,
                              showToolbar: false,
                              fullHeight: true,
                              onKeyTap: _mode == _EditorMode.editKeys
                                  ? (row, col) => setState(
                                      () => _selectedKey = (row, col),
                                    )
                                  : null,
                              onKeyGesture: _mode == _EditorMode.testKeyboard
                                  ? _handleTestGesture
                                  : null,
                            ),
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(height: 12),
                    if (_mode == _EditorMode.editKeys)
                      _buildKeyInspector(context, palette, accent)
                    else if (_mode == _EditorMode.layoutSettings)
                      _buildLayoutSettings(context, palette, accent),
                  ],
                ),
              ),
          ],
        ),
      ),
    );
  }

  Widget _buildTestArea(
    BuildContext context,
    AppPalette palette,
    Color accent,
  ) => Container(
    key: const ValueKey('keyboard-test-area'),
    padding: const EdgeInsets.all(10),
    decoration: BoxDecoration(
      color: accent.withValues(alpha: 0.06),
      borderRadius: BorderRadius.circular(10),
      border: Border.all(color: accent.withValues(alpha: 0.3)),
    ),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      spacing: 8,
      children: [
        Row(
          spacing: 8,
          children: [
            Expanded(
              child: TextField(
                key: const ValueKey('keyboard-test-input'),
                controller: _testController,
                readOnly: true,
                showCursor: true,
                minLines: 1,
                maxLines: 3,
                decoration: const InputDecoration(
                  hintText: '在下方预览键盘上输入或滑动进行效果测试…',
                  isDense: true,
                  border: OutlineInputBorder(),
                ),
              ),
            ),
            ActionButton(
              '清空',
              onPress: () {
                _pushUndoSnapshot();
                _testController.clear();
                setState(() => _testFeedback = '已清空测试区');
              },
            ),
          ],
        ),
        Row(
          spacing: 6,
          children: [
            Icon(FLucideIcons.activity, size: 14, color: accent),
            Expanded(
              child: Text(
                _testFeedback,
                key: const ValueKey('keyboard-test-feedback'),
                style: context.theme.typography.body.xs.copyWith(
                  color: palette.ink,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ),
          ],
        ),
      ],
    ),
  );

  Widget _buildKeyInspector(
    BuildContext context,
    AppPalette palette,
    Color accent,
  ) {
    final sel = _selectedKey;
    final keyMap = _selectedKeyMap;
    if (sel == null || keyMap == null) {
      return StatusMessage('点击上方键盘任意按键开始编辑', icon: FLucideIcons.mousePointerClick);
    }
    final rows = _rowsForLayer(_layout, _layerName);
    final rowLen = rows[sel.$1].length;
    final groupNames =
        ((_layout['keyGroups'] as Map?)?.keys ?? const <dynamic>[])
            .map((e) => e.toString())
            .toList()
          ..sort();

    return Container(
      key: const ValueKey('keyboard-key-inspector'),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: palette.surface,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: palette.hairline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        spacing: 10,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  '第 ${sel.$1 + 1} 行 · 第 ${sel.$2 + 1} 键「${keyMap['label'] ?? ''}」',
                  style: context.theme.typography.body.sm.copyWith(fontWeight: FontWeight.w600),
                ),
              ),
              Text(
                '行内 $rowLen 键',
                style: context.theme.typography.body.xs.copyWith(
                  color: palette.muted,
                ),
              ),
            ],
          ),
          // Structural row/key actions
          Wrap(
            spacing: 6,
            runSpacing: 6,
            children: [
              _ActionChip(
                '左移',
                FLucideIcons.arrowLeft,
                sel.$2 > 0 ? () => _moveSelectedKey(-1) : null,
              ),
              _ActionChip(
                '右移',
                FLucideIcons.arrowRight,
                sel.$2 < rowLen - 1 ? () => _moveSelectedKey(1) : null,
              ),
              _ActionChip(
                '左加键',
                FLucideIcons.plus,
                () => _insertKey(after: false),
              ),
              _ActionChip(
                '右加键',
                FLucideIcons.plus,
                () => _insertKey(after: true),
              ),
              _ActionChip('复制键', FLucideIcons.copy, _duplicateSelectedKey),
              _ActionChip(
                '删键',
                FLucideIcons.trash2,
                rowLen > 1 ? _deleteSelectedKey : null,
              ),
              _ActionChip(
                '上加行',
                FLucideIcons.betweenHorizontalStart,
                () => _insertRow(below: false),
              ),
              _ActionChip(
                '下加行',
                FLucideIcons.betweenHorizontalEnd,
                () => _insertRow(below: true),
              ),
              _ActionChip(
                '删本行',
                FLucideIcons.rows3,
                rows.length > 1 ? _deleteSelectedRow : null,
              ),
            ],
          ),
          // Sub-tabs
          Wrap(
            spacing: 6,
            runSpacing: 6,
            children: [
              for (final entry in const [
                (_KeyInspectorTab.basic, '基础显示'),
                (_KeyInspectorTab.action, '主动作'),
                (_KeyInspectorTab.swipe, '四向滑动'),
                (_KeyInspectorTab.longPress, '长按与候选'),
                (_KeyInspectorTab.extra, '槽位/助记/堆叠'),
              ])
                _ModeChip(
                  key: ValueKey('inspector-tab-${entry.$1.name}'),
                  label: entry.$2,
                  selected: _keyTab == entry.$1,
                  onTap: () => setState(() => _keyTab = entry.$1),
                ),
            ],
          ),
          const Divider(height: 1),
          switch (_keyTab) {
            _KeyInspectorTab.basic => _buildKeyBasicTab(
              sel,
              keyMap,
              groupNames,
            ),
            _KeyInspectorTab.action => _buildKeyActionTab(sel, keyMap),
            _KeyInspectorTab.swipe => _buildKeySwipeTab(sel, keyMap),
            _KeyInspectorTab.longPress => _buildKeyLongPressTab(sel, keyMap),
            _KeyInspectorTab.extra => _buildKeyExtraTab(
              sel,
              keyMap,
              groupNames,
            ),
          },
        ],
      ),
    );
  }

  Widget _buildKeyBasicTab(
    (int, int) sel,
    Map<String, dynamic> keyMap,
    List<String> groupNames,
  ) {
    final weight = ((keyMap['weight'] as num?)?.toDouble() ?? 1.0).clamp(
      0.5,
      6.0,
    );
    final rowSpan = ((keyMap['rowSpan'] as num?)?.toInt() ?? 1).clamp(1, 4);
    final style = (keyMap['style'] as String?) ?? '';
    final group = (keyMap['group'] as String?) ?? '';

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      spacing: 10,
      children: [
        Row(
          spacing: 8,
          children: [
            Expanded(
              child: _InlineTextField(
                fieldKey: 'key-label-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
                inputKey: const ValueKey('key-label-input'),
                label: '键面文字 (label)',
                value: (keyMap['label'] as String?) ?? '',
                onChanged: (v) => _mutateSelectedKey((k) => k['label'] = v),
              ),
            ),
            Expanded(
              child: _InlineTextField(
                fieldKey: 'key-hint-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
                inputKey: const ValueKey('key-hint-input'),
                label: '右上提示 (hint)',
                value: (keyMap['hint'] as String?) ?? '',
                onChanged: (v) => _mutateSelectedKey((k) {
                  if (v.isEmpty) {
                    k.remove('hint');
                  } else {
                    k['hint'] = v;
                  }
                }),
              ),
            ),
          ],
        ),
        Row(
          spacing: 8,
          children: [
            Expanded(
              child: _InlineTextField(
                fieldKey: 'key-value-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
                label: '输入值 (value)',
                value: (keyMap['value'] as String?) ?? '',
                onChanged: (v) => _mutateSelectedKey((k) {
                  if (v.isEmpty) {
                    k.remove('value');
                  } else {
                    k['value'] = v;
                  }
                }),
              ),
            ),
            Expanded(
              child: _InlineTextField(
                fieldKey: 'key-rime-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
                label: 'Rime 编码值 (rimeValue)',
                value: (keyMap['rimeValue'] as String?) ?? '',
                onChanged: (v) => _mutateSelectedKey((k) {
                  if (v.isEmpty) {
                    k.remove('rimeValue');
                  } else {
                    k['rimeValue'] = v;
                  }
                }),
              ),
            ),
          ],
        ),
        Row(
          spacing: 8,
          children: [
            Expanded(
              child: _InlineTextField(
                fieldKey:
                    'key-asciiLabel-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
                label: '英文键面 (asciiLabel)',
                value: (keyMap['asciiLabel'] as String?) ?? '',
                onChanged: (v) => _mutateSelectedKey((k) {
                  if (v.isEmpty) {
                    k.remove('asciiLabel');
                  } else {
                    k['asciiLabel'] = v;
                  }
                }),
              ),
            ),
            Expanded(
              child: _InlineTextField(
                fieldKey:
                    'key-asciiValue-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
                label: '英文输入值 (asciiValue)',
                value: (keyMap['asciiValue'] as String?) ?? '',
                onChanged: (v) => _mutateSelectedKey((k) {
                  if (v.isEmpty) {
                    k.remove('asciiValue');
                  } else {
                    k['asciiValue'] = v;
                  }
                }),
              ),
            ),
          ],
        ),
        Row(
          children: [
            Text('键宽权重：${weight.toStringAsFixed(2)}'),
            Expanded(
              child: Slider(
                value: weight,
                min: 0.5,
                max: 6.0,
                divisions: 55,
                onChanged: (v) => _mutateSelectedKey((k) {
                  final rounded = (v * 100).round() / 100;
                  if ((rounded - 1.0).abs() < 0.005) {
                    k.remove('weight');
                  } else {
                    k['weight'] = rounded;
                  }
                }),
              ),
            ),
          ],
        ),
        Wrap(
          spacing: 12,
          runSpacing: 8,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            Row(
              mainAxisSize: MainAxisSize.min,
              spacing: 6,
              children: [
                const Text('跨行数'),
                DropdownButton<int>(
                  value: rowSpan,
                  items: const [
                    DropdownMenuItem(value: 1, child: Text('1 行')),
                    DropdownMenuItem(value: 2, child: Text('2 行')),
                    DropdownMenuItem(value: 3, child: Text('3 行')),
                    DropdownMenuItem(value: 4, child: Text('4 行')),
                  ],
                  onChanged: (v) {
                    if (v == null) return;
                    _mutateSelectedKey((k) {
                      if (v == 1) {
                        k.remove('rowSpan');
                      } else {
                        k['rowSpan'] = v;
                      }
                    });
                  },
                ),
              ],
            ),
            Row(
              mainAxisSize: MainAxisSize.min,
              spacing: 6,
              children: [
                const Text('外观风格'),
                DropdownButton<String>(
                  value: style == 'accent' ? 'accent' : '',
                  items: const [
                    DropdownMenuItem(value: '', child: Text('默认')),
                    DropdownMenuItem(value: 'accent', child: Text('强调色 (accent)')),
                  ],
                  onChanged: (v) => _mutateSelectedKey((k) {
                    if (v == null || v.isEmpty) {
                      k.remove('style');
                    } else {
                      k['style'] = v;
                    }
                  }),
                ),
              ],
            ),
            Row(
              mainAxisSize: MainAxisSize.min,
              spacing: 6,
              children: [
                const Text('配色分组'),
                DropdownButton<String>(
                  value: groupNames.contains(group) ? group : '',
                  items: [
                    const DropdownMenuItem(value: '', child: Text('默认')),
                    for (final g in groupNames)
                      DropdownMenuItem(value: g, child: Text(g)),
                  ],
                  onChanged: (v) => _mutateSelectedKey((k) {
                    if (v == null || v.isEmpty) {
                      k.remove('group');
                    } else {
                      k['group'] = v;
                    }
                  }),
                ),
              ],
            ),
          ],
        ),
      ],
    );
  }

  Widget _buildKeyActionTab((int, int) sel, Map<String, dynamic> keyMap) =>
      Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        spacing: 12,
        children: [
          _CommandEditor(
            keyPrefix: 'act-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
            title: '中文 / 默认主动作 (action)',
            command: _commandMap(keyMap['action']),
            layerNames: _allLayerNames,
            onChanged: (cmd) => _mutateSelectedKey((k) {
              if (cmd == null) {
                k.remove('action');
              } else {
                k['action'] = cmd;
              }
            }),
          ),
          _CommandEditor(
            keyPrefix: 'aact-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
            title: '英文模式动作 (asciiAction，留空则复用主动作)',
            command: _commandMap(keyMap['asciiAction']),
            layerNames: _allLayerNames,
            onChanged: (cmd) => _mutateSelectedKey((k) {
              if (cmd == null) {
                k.remove('asciiAction');
              } else {
                k['asciiAction'] = cmd;
              }
            }),
          ),
        ],
      );

  Widget _buildKeySwipeTab((int, int) sel, Map<String, dynamic> keyMap) =>
      Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        spacing: 12,
        children: [
          for (final entry in const [
            ('swipeUp', '上滑动作 (swipeUp)'),
            ('swipeDown', '下滑动作 (swipeDown)'),
            ('swipeLeft', '左滑动作 (swipeLeft)'),
            ('swipeRight', '右滑动作 (swipeRight)'),
          ])
            _CommandEditor(
              keyPrefix:
                  '${entry.$1}-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
              title: entry.$2,
              command: _commandMap(keyMap[entry.$1]),
              layerNames: _allLayerNames,
              onChanged: (cmd) => _mutateSelectedKey((k) {
                if (cmd == null) {
                  k.remove(entry.$1);
                } else {
                  k[entry.$1] = cmd;
                }
              }),
            ),
        ],
      );

  Widget _buildKeyLongPressTab((int, int) sel, Map<String, dynamic> keyMap) {
    final alts = [
      if (keyMap['alternates'] is List)
        for (final item in keyMap['alternates'] as List)
          if (item is Map) Map<String, dynamic>.from(item),
    ];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      spacing: 12,
      children: [
        _CommandEditor(
          keyPrefix: 'lp-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
          title: '长按动作 (longPress)',
          command: _commandMap(keyMap['longPress']),
          layerNames: _allLayerNames,
          onChanged: (cmd) => _mutateSelectedKey((k) {
            if (cmd == null) {
              k.remove('longPress');
            } else {
              k['longPress'] = cmd;
            }
          }),
        ),
        _CommandEditor(
          keyPrefix: 'alp-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
          title: '英文长按动作 (asciiLongPress)',
          command: _commandMap(keyMap['asciiLongPress']),
          layerNames: _allLayerNames,
          onChanged: (cmd) => _mutateSelectedKey((k) {
            if (cmd == null) {
              k.remove('asciiLongPress');
            } else {
              k['asciiLongPress'] = cmd;
            }
          }),
        ),
        Row(
          children: [
            const Expanded(
              child: Text(
                '长按候选气泡 (alternates)',
                style: TextStyle(fontWeight: FontWeight.w600),
              ),
            ),
            ActionButton(
              '添加候选',
              onPress: () => _mutateSelectedKey((k) {
                final list = [
                  ...alts,
                  <String, dynamic>{'label': '★', 'value': '★'},
                ];
                k['alternates'] = list;
              }),
            ),
          ],
        ),
        if (alts.isEmpty)
          const Text('未配置长按候选，点击「添加候选」后长按该键可弹出选择')
        else
          for (var i = 0; i < alts.length; i++)
            Row(
              spacing: 8,
              children: [
                Expanded(
                  child: _InlineTextField(
                    fieldKey:
                        'alt-lbl-${sel.$1}-${sel.$2}-$i-${alts.length}-$_selectedId',
                    label: '显示',
                    value: (alts[i]['label'] as String?) ?? '',
                    onChanged: (v) => _mutateSelectedKey((k) {
                      final next = List<Map<String, dynamic>>.from(alts);
                      next[i] = {...next[i], 'label': v};
                      k['alternates'] = next;
                    }),
                  ),
                ),
                Expanded(
                  child: _InlineTextField(
                    fieldKey:
                        'alt-val-${sel.$1}-${sel.$2}-$i-${alts.length}-$_selectedId',
                    label: '上屏值',
                    value: (alts[i]['value'] as String?) ?? '',
                    onChanged: (v) => _mutateSelectedKey((k) {
                      final next = List<Map<String, dynamic>>.from(alts);
                      next[i] = {...next[i], 'value': v};
                      k['alternates'] = next;
                    }),
                  ),
                ),
                FButton.icon(
                  variant: FButtonVariant.ghost,
                  size: FButtonSizeVariant.sm,
                  onPress: () => _mutateSelectedKey((k) {
                    final next = List<Map<String, dynamic>>.from(alts)
                      ..removeAt(i);
                    if (next.isEmpty) {
                      k.remove('alternates');
                    } else {
                      k['alternates'] = next;
                    }
                  }),
                  child: const Icon(FLucideIcons.trash2, size: 15),
                ),
              ],
            ),
      ],
    );
  }

  Widget _buildKeyExtraTab(
    (int, int) sel,
    Map<String, dynamic> keyMap,
    List<String> groupNames,
  ) {
    final slots = keyMap['slots'] is Map
        ? Map<String, dynamic>.from(keyMap['slots'] as Map)
        : <String, dynamic>{};
    final mnemonic = keyMap['mnemonic'] is Map
        ? Map<String, dynamic>.from(keyMap['mnemonic'] as Map)
        : <String, dynamic>{};
    final stack = [
      if (keyMap['stack'] is List)
        for (final item in keyMap['stack'] as List)
          if (item is Map) Map<String, dynamic>.from(item),
    ];

    void updateSlots(String field, String value) => _mutateSelectedKey((k) {
      final next = Map<String, dynamic>.from(slots);
      if (value.isEmpty) {
        next.remove(field);
      } else {
        next[field] = value;
      }
      if (next.isEmpty) {
        k.remove('slots');
      } else {
        k['slots'] = next;
      }
    });

    void updateMnemonic(void Function(Map<String, dynamic>) fn) =>
        _mutateSelectedKey((k) {
          final next = Map<String, dynamic>.from(mnemonic);
          fn(next);
          next.removeWhere(
            (_, v) =>
                v == null ||
                (v is String && v.isEmpty) ||
                (v is List && v.isEmpty),
          );
          if (next.isEmpty) {
            k.remove('mnemonic');
          } else {
            k['mnemonic'] = next;
          }
        });

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      spacing: 10,
      children: [
        const Text('多槽位显示 (slots)', style: TextStyle(fontWeight: FontWeight.w600)),
        Row(
          spacing: 8,
          children: [
            Expanded(
              child: _InlineTextField(
                fieldKey: 'slot-tl-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
                label: '左上 (topLeft)',
                value: (slots['topLeft'] as String?) ?? '',
                onChanged: (v) => updateSlots('topLeft', v),
              ),
            ),
            Expanded(
              child: _InlineTextField(
                fieldKey: 'slot-tc-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
                label: '中上 (topCenter)',
                value: (slots['topCenter'] as String?) ?? '',
                onChanged: (v) => updateSlots('topCenter', v),
              ),
            ),
            Expanded(
              child: _InlineTextField(
                fieldKey: 'slot-tr-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
                label: '右上 (topRight)',
                value: (slots['topRight'] as String?) ?? '',
                onChanged: (v) => updateSlots('topRight', v),
              ),
            ),
          ],
        ),
        Row(
          spacing: 8,
          children: [
            Expanded(
              child: _InlineTextField(
                fieldKey: 'slot-sub-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
                label: '副标题 (labelSub)',
                value: (slots['labelSub'] as String?) ?? '',
                onChanged: (v) => updateSlots('labelSub', v),
              ),
            ),
            Expanded(
              child: _InlineTextField(
                fieldKey: 'slot-bc-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
                label: '底部文字 (bottomCenter)',
                value: (slots['bottomCenter'] as String?) ?? '',
                onChanged: (v) => updateSlots('bottomCenter', v),
              ),
            ),
          ],
        ),
        const Divider(),
        const Text('键面助记 (mnemonic)', style: TextStyle(fontWeight: FontWeight.w600)),
        Row(
          spacing: 8,
          children: [
            Expanded(
              child: _InlineTextField(
                fieldKey: 'mn-top-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
                label: '顶部助记 (top)',
                value: (mnemonic['top'] as String?) ?? '',
                onChanged: (v) => updateMnemonic((m) => m['top'] = v),
              ),
            ),
            Expanded(
              child: _InlineTextField(
                fieldKey: 'mn-badge-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
                label: '角标 (topBadge)',
                value: (mnemonic['topBadge'] as String?) ?? '',
                onChanged: (v) => updateMnemonic((m) => m['topBadge'] = v),
              ),
            ),
          ],
        ),
        _InlineTextField(
          fieldKey: 'mn-bot-${sel.$1}-${sel.$2}-$_selectedId-$_layerName',
          label: '底部助记行（用 | 分隔多行）',
          value: ((mnemonic['bottomLines'] as List?) ?? const [])
              .map((e) => e.toString())
              .join(' | '),
          onChanged: (v) => updateMnemonic((m) {
            final lines = v
                .split('|')
                .map((s) => s.trim())
                .where((s) => s.isNotEmpty)
                .toList();
            m['bottomLines'] = lines;
          }),
        ),
        const Divider(),
        Row(
          children: [
            const Expanded(
              child: Text(
                '纵向堆叠子键 (stack，搭配跨行使用)',
                style: TextStyle(fontWeight: FontWeight.w600),
              ),
            ),
            ActionButton(
              '添加子键',
              onPress: () => _mutateSelectedKey((k) {
                k['stack'] = [
                  ...stack,
                  <String, dynamic>{'label': '，', 'value': '，'},
                ];
              }),
            ),
          ],
        ),
        for (var i = 0; i < stack.length; i++)
          Row(
            spacing: 8,
            children: [
              Expanded(
                child: _InlineTextField(
                  fieldKey:
                      'stk-lbl-${sel.$1}-${sel.$2}-$i-${stack.length}-$_selectedId',
                  label: '子键文字',
                  value: (stack[i]['label'] as String?) ?? '',
                  onChanged: (v) => _mutateSelectedKey((k) {
                    final next = List<Map<String, dynamic>>.from(stack);
                    next[i] = {...next[i], 'label': v};
                    k['stack'] = next;
                  }),
                ),
              ),
              Expanded(
                child: _InlineTextField(
                  fieldKey:
                      'stk-val-${sel.$1}-${sel.$2}-$i-${stack.length}-$_selectedId',
                  label: '子键值',
                  value: (stack[i]['value'] as String?) ?? '',
                  onChanged: (v) => _mutateSelectedKey((k) {
                    final next = List<Map<String, dynamic>>.from(stack);
                    next[i] = {...next[i], 'value': v};
                    k['stack'] = next;
                  }),
                ),
              ),
              FButton.icon(
                variant: FButtonVariant.ghost,
                size: FButtonSizeVariant.sm,
                onPress: () => _mutateSelectedKey((k) {
                  final next = List<Map<String, dynamic>>.from(stack)
                    ..removeAt(i);
                  if (next.isEmpty) {
                    k.remove('stack');
                  } else {
                    k['stack'] = next;
                  }
                }),
                child: const Icon(FLucideIcons.trash2, size: 15),
              ),
            ],
          ),
      ],
    );
  }

  Widget _buildLayoutSettings(
    BuildContext context,
    AppPalette palette,
    Color accent,
  ) {
    final height = ((_layout['height'] as num?)?.toDouble() ?? 266).clamp(
      160.0,
      420.0,
    );
    final hGap = ((_layout['horizontalGap'] as num?)?.toDouble() ?? 4).clamp(
      0.0,
      20.0,
    );
    final vGap = ((_layout['verticalGap'] as num?)?.toDouble() ?? 5).clamp(
      0.0,
      20.0,
    );
    final inset = ((_layout['outerInset'] as num?)?.toDouble() ?? 5).clamp(
      0.0,
      24.0,
    );
    final numberRow = (_layout['numberRow'] as String?) ?? 'auto';
    final recSchemas = ((_layout['recommendedSchemas'] as List?) ?? const [])
        .map((e) => e.toString())
        .toList();
    final keyGroups = Map<String, dynamic>.from(
      (_layout['keyGroups'] as Map?) ?? const {},
    );

    return Container(
      key: const ValueKey('keyboard-layout-settings'),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: palette.surface,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: palette.hairline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        spacing: 12,
        children: [
          Text(
            '基本信息',
            style: context.theme.typography.body.sm.copyWith(
              fontWeight: FontWeight.w600,
            ),
          ),
          _InlineTextField(
            fieldKey: 'meta-name-$_selectedId',
            label: '键盘名称',
            value: (_layout['name'] as String?) ?? '',
            onChanged: (v) => _mutateLayout(() => _layout['name'] = v),
          ),
          _InlineTextField(
            fieldKey: 'meta-desc-$_selectedId',
            label: '说明描述',
            value: (_layout['description'] as String?) ?? '',
            onChanged: (v) => _mutateLayout(() {
              if (v.trim().isEmpty) {
                _layout.remove('description');
              } else {
                _layout['description'] = v;
              }
            }),
          ),
          _InlineTextField(
            fieldKey: 'meta-rec-$_selectedId',
            label: '推荐关联的方案 ID（逗号分隔，如 keytao, keydo）',
            value: recSchemas.join(', '),
            onChanged: (v) => _mutateLayout(() {
              _layout['recommendedSchemas'] = v
                  .split(RegExp(r'[,，\s]+'))
                  .map((s) => s.trim())
                  .where((s) => s.isNotEmpty)
                  .toList();
            }),
          ),
          Row(
            spacing: 10,
            children: [
              const Text('独立数字行模式'),
              DropdownButton<String>(
                value: _numberRowModes.any((m) => m.$1 == numberRow)
                    ? numberRow
                    : 'auto',
                items: [
                  for (final m in _numberRowModes)
                    DropdownMenuItem(value: m.$1, child: Text(m.$2)),
                ],
                onChanged: (v) {
                  if (v != null) _mutateLayout(() => _layout['numberRow'] = v);
                },
              ),
            ],
          ),
          const Divider(),
          Text(
            '几何尺寸',
            style: context.theme.typography.body.sm.copyWith(
              fontWeight: FontWeight.w600,
            ),
          ),
          _MetricSlider(
            label: '键盘高度',
            value: height,
            min: 180,
            max: 380,
            unit: 'dp',
            onChanged: (v) => _mutateLayout(() => _layout['height'] = v.round()),
          ),
          _MetricSlider(
            label: '水平间距',
            value: hGap,
            min: 0,
            max: 16,
            unit: 'dp',
            onChanged: (v) =>
                _mutateLayout(() => _layout['horizontalGap'] = v.round()),
          ),
          _MetricSlider(
            label: '垂直间距',
            value: vGap,
            min: 0,
            max: 16,
            unit: 'dp',
            onChanged: (v) =>
                _mutateLayout(() => _layout['verticalGap'] = v.round()),
          ),
          _MetricSlider(
            label: '外边距',
            value: inset,
            min: 0,
            max: 20,
            unit: 'dp',
            onChanged: (v) =>
                _mutateLayout(() => _layout['outerInset'] = v.round()),
          ),
          const Divider(),
          Row(
            children: [
              Expanded(
                child: Text(
                  '键面配色分组 (keyGroups)',
                  style: context.theme.typography.body.sm.copyWith(fontWeight: FontWeight.w600),
                ),
              ),
              ActionButton(
                '新增分组',
                onPress: () {
                  final nextIndex = keyGroups.length + 1;
                  final name = 'group$nextIndex';
                  _mutateLayout(() {
                    keyGroups[name] = {
                      'light': {
                        'background': '#E8F0FE',
                        'foreground': '#1A365D',
                      },
                      'dark': {
                        'background': '#2A3B55',
                        'foreground': '#E2EEFF',
                      },
                    };
                    _layout['keyGroups'] = keyGroups;
                  });
                },
              ),
            ],
          ),
          if (keyGroups.isEmpty)
            const Text('暂无配色分组。新建分组后可在按键「基础显示」中绑定配色。')
          else
            for (final entry in keyGroups.entries)
              _KeyGroupCard(
                groupName: entry.key,
                groupData: Map<String, dynamic>.from(
                  (entry.value as Map?) ?? const {},
                ),
                onChanged: (next) => _mutateLayout(() {
                  keyGroups[entry.key] = next;
                  _layout['keyGroups'] = keyGroups;
                }),
                onDelete: () => _mutateLayout(() {
                  keyGroups.remove(entry.key);
                  _layout['keyGroups'] = keyGroups;
                }),
              ),
        ],
      ),
    );
  }
}

// -----------------------------------------------------------------------------
// Reusable Editor Widgets
// -----------------------------------------------------------------------------

class _CommandEditor extends StatelessWidget {
  const _CommandEditor({
    required this.keyPrefix,
    required this.title,
    required this.command,
    required this.layerNames,
    required this.onChanged,
  });

  final String keyPrefix;
  final String title;
  final Map<String, dynamic>? command;
  final List<String> layerNames;
  final ValueChanged<Map<String, dynamic>?> onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = AppPalette.of(context);
    final type = (command?['type'] as String?) ?? '';
    final value = (command?['value'] as String?) ?? '';
    final fallback = (command?['fallbackValue'] as String?) ?? '';
    final knownType = _commandTypes.any((e) => e.$1 == type);

    void update({String? nextType, String? nextValue, String? nextFallback}) {
      final t = nextType ?? type;
      if (t.isEmpty) {
        onChanged(null);
        return;
      }
      final v = nextValue ?? value;
      final fb = nextFallback ?? fallback;
      onChanged(<String, dynamic>{
        'type': t,
        if (v.isNotEmpty) 'value': v,
        if (fb.isNotEmpty) 'fallbackValue': fb,
      });
    }

    final layerChoices = <(String, String)>[
      for (final l in layerNames) (l, l),
      ('back', '返回上一层 (back)'),
      ('symbols:last', '上次符号层 (symbols:last)'),
      ('emoji:last', '上次表情层 (emoji:last)'),
    ];

    return Container(
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: palette.hairline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        spacing: 8,
        children: [
          Text(title, style: const TextStyle(fontWeight: FontWeight.w600, fontSize: 12)),
          DropdownButtonFormField<String>(
            key: ValueKey('$keyPrefix-type-$type'),
            initialValue: knownType ? type : '',
            isExpanded: true,
            decoration: const InputDecoration(
              labelText: '指令类型',
              isDense: true,
              border: OutlineInputBorder(),
            ),
            items: [
              for (final item in _commandTypes)
                DropdownMenuItem(value: item.$1, child: Text(item.$2)),
            ],
            onChanged: (next) {
              if (next == null) return;
              final defaultVal = switch (next) {
                'edit' => 'cursorLeft',
                'panel' => 'clipboard',
                'keyboardMode' => 'numbers',
                _ => '',
              };
              update(nextType: next, nextValue: defaultVal);
            },
          ),
          if (type == 'edit')
            DropdownButtonFormField<String>(
              key: ValueKey('$keyPrefix-edit-$value'),
              initialValue: _editVerbs.any((e) => e.$1 == value)
                  ? value
                  : _editVerbs.first.$1,
              isExpanded: true,
              decoration: const InputDecoration(
                labelText: '编辑动作',
                isDense: true,
                border: OutlineInputBorder(),
              ),
              items: [
                for (final v in _editVerbs)
                  DropdownMenuItem(value: v.$1, child: Text(v.$2)),
              ],
              onChanged: (v) => update(nextValue: v ?? ''),
            )
          else if (type == 'panel')
            DropdownButtonFormField<String>(
              key: ValueKey('$keyPrefix-panel-$value'),
              initialValue: _panelTargets.any((e) => e.$1 == value)
                  ? value
                  : _panelTargets.first.$1,
              isExpanded: true,
              decoration: const InputDecoration(
                labelText: '功能面板',
                isDense: true,
                border: OutlineInputBorder(),
              ),
              items: [
                for (final p in _panelTargets)
                  DropdownMenuItem(value: p.$1, child: Text(p.$2)),
              ],
              onChanged: (v) => update(nextValue: v ?? ''),
            )
          else if (type == 'keyboardMode')
            DropdownButtonFormField<String>(
              key: ValueKey('$keyPrefix-layer-$value'),
              initialValue: layerChoices.any((e) => e.$1 == value)
                  ? value
                  : layerChoices.first.$1,
              isExpanded: true,
              decoration: const InputDecoration(
                labelText: '目标键盘层',
                isDense: true,
                border: OutlineInputBorder(),
              ),
              items: [
                for (final l in layerChoices)
                  DropdownMenuItem(value: l.$1, child: Text(l.$2)),
              ],
              onChanged: (v) => update(nextValue: v ?? ''),
            )
          else if (type.isNotEmpty)
            _InlineTextField(
              fieldKey: '$keyPrefix-val-$type',
              label: '参数值 (value)',
              value: value,
              onChanged: (v) => update(nextValue: v),
            ),
          if (type.isNotEmpty)
            _InlineTextField(
              fieldKey: '$keyPrefix-fb-$type',
              label: '回退值 (fallbackValue，可选)',
              value: fallback,
              onChanged: (v) => update(nextFallback: v),
            ),
        ],
      ),
    );
  }
}

class _InlineTextField extends StatefulWidget {
  const _InlineTextField({
    required this.fieldKey,
    this.inputKey,
    required this.label,
    required this.value,
    required this.onChanged,
  });

  final String fieldKey;
  final Key? inputKey;
  final String label;
  final String value;
  final ValueChanged<String> onChanged;

  @override
  State<_InlineTextField> createState() => _InlineTextFieldState();
}

class _InlineTextFieldState extends State<_InlineTextField> {
  late final TextEditingController _controller;

  @override
  void initState() {
    super.initState();
    _controller = TextEditingController(text: widget.value);
  }

  @override
  void didUpdateWidget(covariant _InlineTextField oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.fieldKey != widget.fieldKey ||
        (oldWidget.value != widget.value && _controller.text != widget.value)) {
      _controller.value = TextEditingValue(
        text: widget.value,
        selection: TextSelection.collapsed(offset: widget.value.length),
      );
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => TextField(
    key: widget.inputKey,
    controller: _controller,
    decoration: InputDecoration(
      labelText: widget.label,
      isDense: true,
      border: const OutlineInputBorder(),
    ),
    onChanged: widget.onChanged,
  );
}

class _MetricSlider extends StatelessWidget {
  const _MetricSlider({
    required this.label,
    required this.value,
    required this.min,
    required this.max,
    required this.unit,
    required this.onChanged,
  });

  final String label;
  final double value;
  final double min;
  final double max;
  final String unit;
  final ValueChanged<double> onChanged;

  @override
  Widget build(BuildContext context) => Row(
    children: [
      SizedBox(width: 96, child: Text('$label：${value.round()}$unit')),
      Expanded(
        child: Slider(
          value: value.clamp(min, max),
          min: min,
          max: max,
          divisions: (max - min).round(),
          onChanged: onChanged,
        ),
      ),
    ],
  );
}

class _KeyGroupCard extends StatelessWidget {
  const _KeyGroupCard({
    required this.groupName,
    required this.groupData,
    required this.onChanged,
    required this.onDelete,
  });

  final String groupName;
  final Map<String, dynamic> groupData;
  final ValueChanged<Map<String, dynamic>> onChanged;
  final VoidCallback onDelete;

  @override
  Widget build(BuildContext context) {
    final palette = AppPalette.of(context);
    final light = Map<String, dynamic>.from(
      (groupData['light'] as Map?) ?? const {},
    );
    final dark = Map<String, dynamic>.from(
      (groupData['dark'] as Map?) ?? const {},
    );

    void updateColor(bool isDark, String field, String hex) {
      final next = Map<String, dynamic>.from(groupData);
      final target = Map<String, dynamic>.from(isDark ? dark : light);
      if (hex.trim().isEmpty) {
        target.remove(field);
      } else {
        target[field] = hex.trim();
      }
      next[isDark ? 'dark' : 'light'] = target;
      onChanged(next);
    }

    return Container(
      padding: const EdgeInsets.all(8),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: palette.hairline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        spacing: 8,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  groupName,
                  style: const TextStyle(fontWeight: FontWeight.w600),
                ),
              ),
              FButton.icon(
                variant: FButtonVariant.ghost,
                size: FButtonSizeVariant.sm,
                onPress: onDelete,
                child: const Icon(FLucideIcons.trash2, size: 15),
              ),
            ],
          ),
          Row(
            spacing: 8,
            children: [
              Expanded(
                child: _InlineTextField(
                  fieldKey: 'grp-$groupName-lbg',
                  label: '浅色底色',
                  value: (light['background'] as String?) ?? '',
                  onChanged: (v) => updateColor(false, 'background', v),
                ),
              ),
              Expanded(
                child: _InlineTextField(
                  fieldKey: 'grp-$groupName-lfg',
                  label: '浅色字色',
                  value: (light['foreground'] as String?) ?? '',
                  onChanged: (v) => updateColor(false, 'foreground', v),
                ),
              ),
            ],
          ),
          Row(
            spacing: 8,
            children: [
              Expanded(
                child: _InlineTextField(
                  fieldKey: 'grp-$groupName-dbg',
                  label: '深色底色',
                  value: (dark['background'] as String?) ?? '',
                  onChanged: (v) => updateColor(true, 'background', v),
                ),
              ),
              Expanded(
                child: _InlineTextField(
                  fieldKey: 'grp-$groupName-dfg',
                  label: '深色字色',
                  value: (dark['foreground'] as String?) ?? '',
                  onChanged: (v) => updateColor(true, 'foreground', v),
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

class _SmallTag extends StatelessWidget {
  const _SmallTag(this.label, this.color);

  final String label;
  final Color color;

  @override
  Widget build(BuildContext context) => Container(
    padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 1),
    decoration: BoxDecoration(
      color: color.withValues(alpha: 0.14),
      borderRadius: BorderRadius.circular(4),
    ),
    child: Text(
      label,
      style: TextStyle(fontSize: 10, fontWeight: FontWeight.w600, color: color),
    ),
  );
}

class _ActionChip extends StatelessWidget {
  const _ActionChip(this.label, this.icon, this.onTap);

  final String label;
  final IconData icon;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final palette = AppPalette.of(context);
    return FTappable(
      onPress: onTap,
      child: Opacity(
        opacity: onTap == null ? 0.45 : 1.0,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 5),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(6),
            border: Border.all(color: palette.hairline),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            spacing: 4,
            children: [
              Icon(icon, size: 13, color: palette.ink),
              Text(label, style: TextStyle(fontSize: 12, color: palette.ink)),
            ],
          ),
        ),
      ),
    );
  }
}

class _ModeChip extends StatelessWidget {
  const _ModeChip({
    super.key,
    required this.label,
    this.icon,
    required this.selected,
    required this.onTap,
  });

  final String label;
  final IconData? icon;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = AppPalette.of(context);
    final accent = ChromeAccent.of(context);
    return FTappable(
      onPress: onTap,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
        decoration: BoxDecoration(
          color: selected ? accent.withValues(alpha: 0.14) : palette.surface,
          borderRadius: BorderRadius.circular(8),
          border: Border.all(
            color: selected ? accent : palette.hairline,
            width: selected ? 1.5 : 1,
          ),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          spacing: 5,
          children: [
            if (icon != null)
              Icon(icon, size: 13, color: selected ? accent : palette.ink),
            Text(
              label,
              style: TextStyle(
                fontSize: 12,
                fontWeight: selected ? FontWeight.w600 : FontWeight.w500,
                color: selected ? accent : palette.ink,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _TogglePill extends StatelessWidget {
  const _TogglePill({
    required this.label,
    required this.active,
    required this.onTap,
  });

  final String label;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = AppPalette.of(context);
    final accent = ChromeAccent.of(context);
    return FTappable(
      onPress: onTap,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 5),
        decoration: BoxDecoration(
          color: active ? accent.withValues(alpha: 0.12) : Colors.transparent,
          borderRadius: BorderRadius.circular(999),
          border: Border.all(color: active ? accent : palette.hairline),
        ),
        child: Text(
          label,
          style: TextStyle(
            fontSize: 11,
            fontWeight: FontWeight.w600,
            color: active ? accent : palette.muted,
          ),
        ),
      ),
    );
  }
}

class _LayerChip extends StatelessWidget {
  const _LayerChip({
    super.key,
    required this.label,
    required this.selected,
    required this.onTap,
    this.onDelete,
  });

  final String label;
  final bool selected;
  final VoidCallback onTap;
  final VoidCallback? onDelete;

  @override
  Widget build(BuildContext context) {
    final palette = AppPalette.of(context);
    final accent = ChromeAccent.of(context);
    return FTappable(
      onPress: onTap,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 5),
        decoration: BoxDecoration(
          color: selected ? accent.withValues(alpha: 0.12) : palette.surface,
          borderRadius: BorderRadius.circular(6),
          border: Border.all(color: selected ? accent : palette.hairline),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          spacing: 4,
          children: [
            Text(
              label,
              style: TextStyle(
                fontSize: 12,
                fontWeight: selected ? FontWeight.w600 : FontWeight.w400,
                color: selected ? accent : palette.ink,
              ),
            ),
            if (onDelete != null)
              GestureDetector(
                onTap: onDelete,
                child: Icon(FLucideIcons.x, size: 12, color: palette.muted),
              ),
          ],
        ),
      ),
    );
  }
}
