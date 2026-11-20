import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart' show ScrollCacheExtent;
import 'package:flutter/services.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../rust/api/types.dart';
import 'theme.dart';
import 'widgets.dart';

const themeFontWeights = {
  'ultralight': '极细',
  'thin': '纤细',
  'light': '细',
  'regular': '常规',
  'medium': '中等',
  'semibold': '半粗',
  'bold': '粗',
  'heavy': '重',
  'black': '特粗',
};

String closestThemeFontWeight(String weight, List<String> supported) {
  if (supported.isEmpty || supported.contains(weight)) return weight;
  final weights = themeFontWeights.keys.toList();
  final current = weights.indexOf(weight);
  return supported.reduce(
    (a, b) =>
        (weights.indexOf(a) - current).abs() <=
            (weights.indexOf(b) - current).abs()
        ? a
        : b,
  );
}

class SystemFontCatalog {
  SystemFontCatalog._(this.controller);

  static final _catalogs = Expando<SystemFontCatalog>();
  static int _nextAlias = 0;

  static SystemFontCatalog forController(AppController controller) =>
      _catalogs[controller] ??= SystemFontCatalog._(controller);

  final AppController controller;
  Future<List<SystemFontFamilyDto>>? _listing;
  List<SystemFontFamilyDto>? families;
  final _loads = <String, Future<String>>{};
  final _aliases = <String, String>{};

  String? alias(String family) => _aliases[family];

  List<String>? supportedWeights(String family) => family.isEmpty
      ? null
      : families?.where((font) => font.family == family).firstOrNull?.weights;

  Future<List<SystemFontFamilyDto>> list() =>
      _listing ??= controller.listSystemFontFamilies().then(
        (fonts) => families = fonts,
        onError: (Object error, StackTrace stack) {
          _listing = null;
          Error.throwWithStackTrace(error, stack);
        },
      );

  Future<String> load(String family) => _loads.putIfAbsent(family, () async {
    final alias = 'keytao-system-font-${_nextAlias++}';
    final bytes = await controller.loadSystemFontBytes(family: family);
    final loader = FontLoader(alias)
      ..addFont(Future.value(ByteData.sublistView(bytes)));
    await loader.load();
    _aliases[family] = alias;
    return alias;
  });
}

Future<String?> showThemeFontPicker(
  BuildContext context, {
  required AppController controller,
  required String selected,
}) => showFDialog<String>(
  context: context,
  builder: (context, _, animation) {
    final content = SystemFontPicker(
      catalog: SystemFontCatalog.forController(controller),
      selected: selected,
    );
    final close = ActionButton('关闭', onPress: () => Navigator.pop(context));
    if (controller.isMobile) {
      return Scaffold(
        backgroundColor: AppPalette.of(context).surface,
        body: SafeArea(
          child: Padding(
            padding: const EdgeInsets.all(16),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              spacing: 12,
              children: [
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        '选择字体',
                        style: context.theme.typography.display.lg,
                      ),
                    ),
                    close,
                  ],
                ),
                Expanded(child: content),
              ],
            ),
          ),
        ),
      );
    }
    return AppDialog(
      title: '选择字体',
      animation: animation,
      constraints: const BoxConstraints(maxWidth: 560, maxHeight: 660),
      scrollable: false,
      actions: [close],
      child: SizedBox(
        width: 480,
        height: (MediaQuery.sizeOf(context).height * .65).clamp(220, 500),
        child: content,
      ),
    );
  },
);

class SystemFontPicker extends StatefulWidget {
  const SystemFontPicker({
    super.key,
    required this.catalog,
    required this.selected,
  });

  final SystemFontCatalog catalog;
  final String selected;

  @override
  State<SystemFontPicker> createState() => _SystemFontPickerState();
}

class _SystemFontPickerState extends State<SystemFontPicker> {
  late final _listing = widget.catalog.list();
  String _query = '';

  Widget _row(String family, String name, String? alias) => Semantics(
    selected: widget.selected == family,
    button: true,
    child: FTappable(
      onPress: () => Navigator.pop(context, family),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 12),
        child: Row(
          spacing: 8,
          children: [
            Expanded(
              child: Text(
                name,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontFamily: alias),
              ),
            ),
            if (widget.selected == family)
              const Icon(FLucideIcons.check, size: 16, semanticLabel: '已选择'),
          ],
        ),
      ),
    ),
  );

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    spacing: 8,
    children: [
      Semantics(
        label: '搜索字体',
        child: FTextField(
          hint: '搜索字体',
          control: FTextFieldControl.managed(
            onChange: (value) =>
                setState(() => _query = value.text.trim().toLowerCase()),
          ),
        ),
      ),
      Expanded(
        child: FutureBuilder<List<SystemFontFamilyDto>>(
          future: _listing,
          builder: (context, snapshot) {
            if (snapshot.hasError) return const ErrorMessage('字体加载失败');
            final fonts = snapshot.data;
            if (fonts == null) {
              return const Center(child: FProgress());
            }
            final filtered = fonts
                .where(
                  (font) =>
                      font.displayName.toLowerCase().contains(_query) ||
                      font.family.toLowerCase().contains(_query),
                )
                .toList();
            return ListView.builder(
              itemExtent: 48,
              scrollCacheExtent: const ScrollCacheExtent.pixels(0),
              itemCount: filtered.length + 1,
              itemBuilder: (context, index) {
                if (index == 0) return _row('', '默认', null);
                final font = filtered[index - 1];
                return FutureBuilder<String>(
                  key: ValueKey(font.family),
                  future: widget.catalog.load(font.family),
                  builder: (context, snapshot) =>
                      _row(font.family, font.displayName, snapshot.data),
                );
              },
            );
          },
        ),
      ),
    ],
  );
}
