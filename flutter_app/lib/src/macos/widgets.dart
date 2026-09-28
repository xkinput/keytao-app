import 'dart:async';

import 'package:flutter/cupertino.dart';
import 'package:flutter/material.dart' show SelectableText;
import 'package:flutter/services.dart';
import 'package:macos_ui/macos_ui.dart';

import '../app/controller.dart';
import '../app/options.dart';
import '../rust/api/types.dart';

class MacosForm extends StatelessWidget {
  const MacosForm({super.key, required this.children, this.scrollController});
  final List<Widget> children;
  final ScrollController? scrollController;

  @override
  Widget build(BuildContext context) => SingleChildScrollView(
    controller: scrollController,
    padding: const EdgeInsets.all(24),
    child: Align(
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 680),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: children,
        ),
      ),
    ),
  );
}

class MacosGroup extends StatelessWidget {
  const MacosGroup({
    super.key,
    required this.title,
    required this.children,
    this.icon,
    this.trailing,
  });
  final String title;
  final List<Widget> children;
  final IconData? icon;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(bottom: 20),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.only(left: 12, bottom: 8),
          child: Row(
            children: [
              if (icon != null) ...[
                MacosIcon(icon, size: 16),
                const SizedBox(width: 8),
              ],
              Expanded(
                child: Text(
                  title,
                  style: MacosTheme.of(context).typography.body.copyWith(
                    color: MacosDynamicColor.resolve(
                      MacosColors.secondaryLabelColor,
                      context,
                    ),
                  ),
                ),
              ),
              ?trailing,
            ],
          ),
        ),
        DecoratedBox(
          decoration: BoxDecoration(
            color: MacosDynamicColor.resolve(
              MacosColors.controlBackgroundColor,
              context,
            ),
            borderRadius: BorderRadius.circular(10),
            border: Border.all(
              color: MacosTheme.of(context).dividerColor,
              width: 0.5,
            ),
          ),
          child: MacosRows(children: children),
        ),
      ],
    ),
  );
}

class MacosRows extends StatelessWidget {
  const MacosRows({super.key, required this.children});
  final List<Widget> children;

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      for (var index = 0; index < children.length; index++) ...[
        if (index > 0)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 12),
            child: Container(
              height: 0.5,
              color: MacosTheme.of(context).dividerColor,
            ),
          ),
        children[index],
      ],
    ],
  );
}

class MacosSettingRow extends StatelessWidget {
  const MacosSettingRow(
    this.label, {
    super.key,
    required this.child,
    this.icon,
    this.stacked = false,
  });
  final String label;
  final Widget child;
  final IconData? icon;
  final bool stacked;

  Widget get _label => Row(
    mainAxisSize: MainAxisSize.min,
    children: [
      if (icon != null) ...[
        MacosIcon(icon, size: 16),
        const SizedBox(width: 8),
      ],
      Flexible(child: Text(label)),
    ],
  );

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
    child: stacked
        ? Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [_label, const SizedBox(height: 8), child],
          )
        : Row(
            children: [
              Expanded(child: _label),
              const SizedBox(width: 20),
              Flexible(
                flex: 2,
                child: Align(alignment: Alignment.centerRight, child: child),
              ),
            ],
          ),
  );
}

class MacosValueRow extends StatelessWidget {
  const MacosValueRow(this.label, this.value, {super.key});
  final String label;
  final String value;

  @override
  Widget build(BuildContext context) => MacosSettingRow(
    label,
    child: SelectableText(value, textAlign: TextAlign.right),
  );
}

class MacosActionRow extends StatelessWidget {
  const MacosActionRow({super.key, required this.children});
  final List<Widget> children;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.all(12),
    child: Wrap(
      alignment: WrapAlignment.end,
      spacing: 8,
      runSpacing: 8,
      children: children,
    ),
  );
}

class MacosChoice<T> extends StatefulWidget {
  const MacosChoice({
    super.key,
    required this.label,
    required this.value,
    required this.options,
    this.disabled = const {},
    this.onChanged,
    this.icons = const {},
  });
  final String label;
  final T value;
  final Map<T, String> options;
  final Set<T> disabled;
  final ValueChanged<T>? onChanged;
  final Map<T, IconData> icons;

  @override
  State<MacosChoice<T>> createState() => _MacosChoiceState<T>();
}

class _MacosChoiceState<T> extends State<MacosChoice<T>> {
  late final _tabs = MacosTabController(
    length: widget.options.length,
    initialIndex: _selectedIndex,
  )..addListener(_selected);
  bool _syncing = false;
  int get _selectedIndex => widget.options.keys
      .toList()
      .indexOf(widget.value)
      .clamp(0, widget.options.length - 1);

  void _sync() {
    _syncing = true;
    _tabs.index = _selectedIndex;
    _syncing = false;
  }

  void _selected() {
    if (_syncing) return;
    final value = widget.options.keys.elementAt(_tabs.index);
    if (widget.onChanged == null || widget.disabled.contains(value)) {
      _sync();
    } else {
      widget.onChanged!(value);
    }
  }

  @override
  void didUpdateWidget(covariant MacosChoice<T> oldWidget) {
    super.didUpdateWidget(oldWidget);
    _sync();
  }

  void _move(int delta) {
    final keys = widget.options.keys.toList();
    for (
      var index = _selectedIndex + delta;
      index >= 0 && index < keys.length;
      index += delta
    ) {
      if (!widget.disabled.contains(keys[index])) {
        widget.onChanged?.call(keys[index]);
        return;
      }
    }
  }

  @override
  void dispose() {
    _tabs.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) => MacosSettingRow(
      widget.label,
      stacked: constraints.maxWidth < 560,
      child: widget.options.length >= 4
          ? MacosPopupButton<T>(
              value: widget.options.containsKey(widget.value)
                  ? widget.value
                  : null,
              hint: Text('${widget.value}'),
              onChanged: widget.onChanged == null
                  ? null
                  : (value) {
                      if (value != null) widget.onChanged!(value);
                    },
              items: [
                for (final entry in widget.options.entries)
                  MacosPopupMenuItem(
                    value: entry.key,
                    enabled: !widget.disabled.contains(entry.key),
                    child: Text(entry.value),
                  ),
              ],
            )
          : Semantics(
              label: widget.label,
              value: widget.options[widget.value],
              enabled: widget.onChanged != null,
              child: Focus(
                canRequestFocus: widget.onChanged != null,
                onKeyEvent: (_, event) {
                  if (event is KeyDownEvent && widget.onChanged != null) {
                    if (event.logicalKey == LogicalKeyboardKey.arrowLeft) {
                      _move(-1);
                      return KeyEventResult.handled;
                    }
                    if (event.logicalKey == LogicalKeyboardKey.arrowRight) {
                      _move(1);
                      return KeyEventResult.handled;
                    }
                  }
                  return KeyEventResult.ignored;
                },
                child: Builder(
                  builder: (context) => DecoratedBox(
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(5),
                      border: Border.all(
                        color: Focus.of(context).hasFocus
                            ? MacosTheme.of(context).primaryColor
                            : MacosColors.transparent,
                        width: 2,
                      ),
                    ),
                    child: IgnorePointer(
                      ignoring: widget.onChanged == null,
                      child: Opacity(
                        opacity: widget.onChanged == null ? 0.5 : 1,
                        child: LayoutBuilder(
                          builder: (context, constraints) =>
                              MacosSegmentedControl(
                                controller: _tabs,
                                tabs: [
                                  for (final entry in widget.options.entries)
                                    _ChoiceTab(
                                      label: entry.value,
                                      icon: widget.icons[entry.key],
                                      maxWidth:
                                          (constraints.maxWidth -
                                              1 -
                                              (widget.options.length - 1) * 2) /
                                          widget.options.length,
                                      enabled: !widget.disabled.contains(
                                        entry.key,
                                      ),
                                    ),
                                ],
                              ),
                        ),
                      ),
                    ),
                  ),
                ),
              ),
            ),
    ),
  );
}

class _ChoiceTab extends MacosTab {
  const _ChoiceTab({
    required super.label,
    required this.enabled,
    super.active,
    this.icon,
    required this.maxWidth,
  });
  final bool enabled;
  final IconData? icon;
  final double maxWidth;

  @override
  MacosTab copyWith({String? label, bool? active}) => _ChoiceTab(
    label: label ?? this.label,
    active: active ?? this.active,
    enabled: enabled,
    icon: icon,
    maxWidth: maxWidth,
  );

  @override
  Widget build(BuildContext context) => Semantics(
    enabled: enabled,
    selected: active,
    child: Opacity(
      opacity: enabled ? 1 : 0.5,
      child: ConstrainedBox(
        constraints: BoxConstraints(maxWidth: maxWidth),
        child: DecoratedBox(
          decoration: BoxDecoration(
            color: active
                ? (MacosTheme.of(context).brightness == Brightness.dark
                      ? const Color(0xFF646669)
                      : MacosColors.white)
                : MacosColors.transparent,
            borderRadius: BorderRadius.circular(4),
          ),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (icon != null) ...[
                  MacosIcon(icon, size: 14),
                  const SizedBox(width: 5),
                ],
                Flexible(child: Text(label, textAlign: TextAlign.center)),
              ],
            ),
          ),
        ),
      ),
    ),
  );
}

class MacosFontSlider extends StatefulWidget {
  const MacosFontSlider({super.key, required this.value, this.onChanged});
  final double value;
  final ValueChanged<double>? onChanged;

  @override
  State<MacosFontSlider> createState() => _MacosFontSliderState();
}

class _MacosFontSliderState extends State<MacosFontSlider> {
  double? _draft;

  @override
  void didUpdateWidget(covariant MacosFontSlider oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.value != widget.value || widget.onChanged == null) {
      _draft = null;
    }
  }

  void _commit() {
    if (!mounted || _draft == null) return;
    final value = _draft!;
    setState(() => _draft = null);
    widget.onChanged?.call(value);
  }

  @override
  Widget build(BuildContext context) {
    final value = (_draft ?? widget.value).clamp(
      candidateFontMin,
      candidateFontMax,
    );
    return MacosSettingRow(
      AppStrings.candidateFontSize,
      icon: CupertinoIcons.textformat_size,
      child: SizedBox(
        width: 240,
        child: Row(
          children: [
            Expanded(
              child: Semantics(
                enabled: widget.onChanged != null,
                child: Focus(
                  canRequestFocus: widget.onChanged != null,
                  onKeyEvent: (_, event) {
                    if (event is! KeyDownEvent || widget.onChanged == null) {
                      return KeyEventResult.ignored;
                    }
                    final delta =
                        event.logicalKey == LogicalKeyboardKey.arrowLeft
                        ? -1
                        : event.logicalKey == LogicalKeyboardKey.arrowRight
                        ? 1
                        : 0;
                    if (delta == 0) return KeyEventResult.ignored;
                    widget.onChanged!(
                      (value + delta).clamp(candidateFontMin, candidateFontMax),
                    );
                    return KeyEventResult.handled;
                  },
                  child: Listener(
                    // macos_ui has no onChangeEnd; commit after its final gesture event.
                    onPointerUp: (_) => scheduleMicrotask(_commit),
                    onPointerCancel: (_) => setState(() => _draft = null),
                    child: IgnorePointer(
                      ignoring: widget.onChanged == null,
                      child: Opacity(
                        opacity: widget.onChanged == null ? 0.5 : 1,
                        child: MacosSlider(
                          value: value,
                          min: candidateFontMin,
                          max: candidateFontMax,
                          color: MacosTheme.of(context).primaryColor,
                          semanticLabel: '候选字号',
                          onChanged: (value) =>
                              setState(() => _draft = value.roundToDouble()),
                        ),
                      ),
                    ),
                  ),
                ),
              ),
            ),
            const SizedBox(width: 12),
            SizedBox(
              width: 32,
              child: Text('${value.round()}', textAlign: TextAlign.right),
            ),
          ],
        ),
      ),
    );
  }
}

class MacosOperationProgress extends StatelessWidget {
  const MacosOperationProgress({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final active = c.isInstalling || c.isDeploying || c.checkingStorage;
    if (!active && c.deploySteps.isEmpty && c.operationLogCount == 0) {
      return const SizedBox.shrink();
    }
    return Padding(
      padding: const EdgeInsets.all(12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final step in c.deploySteps)
            Padding(
              padding: const EdgeInsets.only(bottom: 8),
              child: Row(
                children: [
                  if (step.state == DeployStepState.running)
                    const ProgressCircle(radius: 7)
                  else
                    MacosIcon(
                      step.state == DeployStepState.ok
                          ? CupertinoIcons.checkmark_circle_fill
                          : CupertinoIcons.xmark_circle_fill,
                      size: 16,
                      color: MacosDynamicColor.resolve(
                        step.state == DeployStepState.ok
                            ? MacosColors.systemGreenColor
                            : MacosColors.systemRedColor,
                        context,
                      ),
                    ),
                  const SizedBox(width: 8),
                  Expanded(child: Text(step.message)),
                ],
              ),
            ),
          if (active && c.installFraction != null) ...[
            ProgressBar(value: c.installFraction! * 100),
            const SizedBox(height: 10),
          ],
          Row(
            children: [
              if (active && c.installFraction == null) ...[
                const ProgressCircle(radius: 8),
                const SizedBox(width: 8),
              ],
              Expanded(
                child: Semantics(
                  liveRegion: true,
                  child: Text(active ? c.progress : ''),
                ),
              ),
              if (c.operationLogCount > 0)
                PushButton(
                  controlSize: ControlSize.regular,
                  secondary: true,
                  onPressed: () => showMacosSheet<void>(
                    context: context,
                    builder: (context) => MacosSheet(
                      insetPadding: const EdgeInsets.all(32),
                      child: Padding(
                        padding: const EdgeInsets.all(20),
                        child: Column(
                          children: [
                            Text(
                              AppStrings.operationLogs,
                              style: MacosTheme.of(context).typography.headline,
                            ),
                            const SizedBox(height: 16),
                            Expanded(
                              child: ListenableBuilder(
                                listenable: c,
                                builder: (context, _) =>
                                    MacosLogView(lines: c.operationLogs),
                              ),
                            ),
                            const SizedBox(height: 16),
                            Row(
                              mainAxisAlignment: MainAxisAlignment.end,
                              children: [
                                PushButton(
                                  controlSize: ControlSize.regular,
                                  secondary: true,
                                  onPressed: c.clearOperationLogs,
                                  child: const Text(
                                    AppStrings.clearOperationLogs,
                                  ),
                                ),
                                const SizedBox(width: 8),
                                PushButton(
                                  controlSize: ControlSize.regular,
                                  onPressed: () => Navigator.pop(context),
                                  child: const Text(AppStrings.close),
                                ),
                              ],
                            ),
                          ],
                        ),
                      ),
                    ),
                  ),
                  child: Text(
                    AppStrings.operationLogCount(c.operationLogCount),
                  ),
                ),
            ],
          ),
        ],
      ),
    );
  }
}

class MacosLogView extends StatelessWidget {
  const MacosLogView({super.key, required this.lines, this.colorize = true});
  final List<String> lines;
  final bool colorize;

  @override
  Widget build(BuildContext context) => lines.isEmpty
      ? const Center(child: Text(AppStrings.noLogs))
      : ListView.builder(
          padding: const EdgeInsets.all(12),
          itemCount: lines.length,
          itemBuilder: (context, index) => Padding(
            padding: const EdgeInsets.only(bottom: 6),
            child: SelectableText(
              lines[index],
              style: MacosTheme.of(context).typography.body.copyWith(
                fontFamily: 'Menlo',
                color: colorize
                    ? switch (operationLogKind(lines[index])) {
                        LogLineKind.error => MacosDynamicColor.resolve(
                          MacosColors.systemRedColor,
                          context,
                        ),
                        LogLineKind.warning => MacosDynamicColor.resolve(
                          MacosColors.systemYellowColor,
                          context,
                        ),
                        LogLineKind.deploy => MacosDynamicColor.resolve(
                          MacosColors.systemGreenColor,
                          context,
                        ),
                        LogLineKind.merged => MacosTheme.of(
                          context,
                        ).primaryColor,
                        LogLineKind.normal => null,
                      }
                    : null,
              ),
            ),
          ),
        );
}

class MacosBadge extends StatelessWidget {
  const MacosBadge(this.text, {super.key, this.color});
  final String text;
  final Color? color;

  @override
  Widget build(BuildContext context) {
    final tint = color == null
        ? MacosDynamicColor.resolve(MacosColors.secondaryLabelColor, context)
        : MacosDynamicColor.resolve(color!, context);
    return DecoratedBox(
      decoration: BoxDecoration(
        color: tint.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(5),
      ),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 3),
        child: Text(text, style: TextStyle(color: tint)),
      ),
    );
  }
}

class MacosMessage extends StatelessWidget {
  const MacosMessage(this.text, {super.key, this.color, this.icon});
  final String text;
  final Color? color;
  final IconData? icon;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.all(12),
    child: Semantics(
      liveRegion: true,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (icon != null) ...[
            MacosIcon(
              icon,
              size: 16,
              color: color == null
                  ? null
                  : MacosDynamicColor.resolve(color!, context),
            ),
            const SizedBox(width: 8),
          ],
          Expanded(
            child: SelectableText(
              text,
              style: TextStyle(
                color: color == null
                    ? null
                    : MacosDynamicColor.resolve(color!, context),
              ),
            ),
          ),
        ],
      ),
    ),
  );
}

class MacosUpdateBanner extends StatelessWidget {
  const MacosUpdateBanner({super.key, required this.controller});
  final AppController controller;

  @override
  Widget build(BuildContext context) {
    final update = controller.appUpdate;
    if (update?.hasUpdate != true) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.only(bottom: 20),
      child: PushButton(
        controlSize: ControlSize.large,
        secondary: true,
        onPressed: controller.openAppUpdate,
        child: Row(
          children: [
            MacosIcon(
              CupertinoIcons.arrow_up_circle,
              color: MacosTheme.of(context).primaryColor,
            ),
            const SizedBox(width: 10),
            Expanded(
              child: Wrap(
                spacing: 8,
                runSpacing: 4,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  const Text(AppStrings.appUpdate),
                  MacosBadge('v${update!.latestVersion}'),
                  Text(AppStrings.currentVersion(update.currentVersion)),
                ],
              ),
            ),
            const MacosIcon(CupertinoIcons.arrow_up_right_square, size: 16),
          ],
        ),
      ),
    );
  }
}

class MacosInstallProgress extends StatelessWidget {
  const MacosInstallProgress({super.key, required this.active, this.progress});
  final bool active;
  final InstallProgressDto? progress;

  @override
  Widget build(BuildContext context) {
    if (!active) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.all(12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (progress == null)
            const Align(
              alignment: Alignment.centerLeft,
              child: ProgressCircle(radius: 8),
            )
          else ...[
            ProgressBar(value: progress!.percent.clamp(0, 100).toDouble()),
            const SizedBox(height: 8),
            Text(progress!.message),
          ],
        ],
      ),
    );
  }
}
