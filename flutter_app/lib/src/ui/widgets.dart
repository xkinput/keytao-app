import 'package:flutter/material.dart';
import 'package:forui/forui.dart';

import '../app/controller.dart';
import '../app/options.dart';

class Section extends StatelessWidget {
  const Section({
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
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      Padding(
        padding: const EdgeInsets.only(bottom: 10),
        child: Row(
          children: [
            if (icon != null) ...[
              Icon(icon, size: 17),
              const SizedBox(width: 8),
            ],
            Expanded(
              child: Text(
                title,
                style: context.theme.typography.body.sm.copyWith(
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
            ?trailing,
          ],
        ),
      ),
      if (children.isNotEmpty)
        FTileGroup(
          divider: FItemDivider.full,
          children: [
            for (final child in children)
              FTile.raw(
                style: const FItemStyleDelta.delta(
                  padding: EdgeInsetsGeometryDelta.value(EdgeInsets.zero),
                  rawContentStyle: FRawItemContentStyleDelta.delta(
                    padding: EdgeInsetsGeometryDelta.value(EdgeInsets.zero),
                  ),
                ),
                child: Padding(
                  padding: const EdgeInsets.all(14),
                  child: DefaultTextStyle.merge(
                    overflow: TextOverflow.visible,
                    child: child,
                  ),
                ),
              ),
          ],
        ),
    ],
  );
}

class SettingRow extends StatelessWidget {
  const SettingRow(
    this.label, {
    super.key,
    required this.child,
    this.description,
    this.stacked = false,
    this.icon,
  });
  final String label;
  final Widget child;
  final String? description;
  final bool stacked;
  final IconData? icon;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      final title = Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (icon != null) ...[Icon(icon, size: 16), const SizedBox(width: 8)],
          Flexible(child: Text(label)),
        ],
      );
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (stacked || constraints.maxWidth < 440) ...[
            title,
            const SizedBox(height: 10),
            child,
          ] else
            Row(
              children: [
                Expanded(child: title),
                const SizedBox(width: 16),
                Flexible(flex: 2, child: child),
              ],
            ),
          if (description != null) ...[
            const SizedBox(height: 8),
            Text(
              description!,
              style: context.theme.typography.body.xs.copyWith(
                color: context.theme.colors.mutedForeground,
                height: 1.5,
              ),
            ),
          ],
        ],
      );
    },
  );
}

class ChoiceSetting<T> extends StatelessWidget {
  const ChoiceSetting({
    super.key,
    required this.label,
    required this.value,
    required this.options,
    this.onChanged,
    this.disabled = const {},
    this.hint,
    this.currentLabel,
    this.icons = const {},
  });
  final String label;
  final T value;
  final Map<T, String> options;
  final Set<T> disabled;
  final ValueChanged<T>? onChanged;
  final String? hint;
  final String? currentLabel;
  final Map<T, IconData> icons;

  @override
  Widget build(BuildContext context) => SettingRow(
    label,
    description: hint,
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (currentLabel != null) ...[
          Text(currentLabel!, style: context.theme.typography.body.xs),
          const SizedBox(height: 6),
        ],
        if (disabled.isNotEmpty)
          FSelectGroup<T>(
            enabled: onChanged != null,
            control: FMultiValueControl.lifted(
              value: {value},
              onChange: (values) {
                if (values.isNotEmpty) onChanged?.call(values.last);
              },
            ),
            children: [
              for (final entry in options.entries)
                FSelectGroupItemMixin.radio(
                  value: entry.key,
                  label: Text(entry.value),
                  enabled: onChanged != null && !disabled.contains(entry.key),
                ),
            ],
          )
        else
          Semantics(
            enabled: onChanged != null,
            child: ExcludeFocus(
              excluding: onChanged == null,
              child: IgnorePointer(
                ignoring: onChanged == null,
                child: Opacity(
                  opacity: onChanged == null ? .5 : 1,
                  child: FTabs(
                    control: FTabControl.lifted(
                      index: options.keys
                          .toList()
                          .indexOf(value)
                          .clamp(0, options.length - 1),
                      onChange: (index) =>
                          onChanged?.call(options.keys.elementAt(index)),
                    ),
                    style: const FTabsStyleDelta.delta(
                      spacing: 0,
                      minHeight: 32,
                    ),
                    children: [
                      for (final entry in options.entries)
                        FTabEntry(
                          label: FittedBox(
                            fit: BoxFit.scaleDown,
                            child: Row(
                              mainAxisSize: MainAxisSize.min,
                              children: [
                                if (icons[entry.key] case final icon?) ...[
                                  Icon(icon, size: 14),
                                  const SizedBox(width: 5),
                                ],
                                Text(
                                  entry.value,
                                  style: const TextStyle(fontSize: 12),
                                ),
                              ],
                            ),
                          ),
                          child: const SizedBox.shrink(),
                        ),
                    ],
                  ),
                ),
              ),
            ),
          ),
      ],
    ),
  );
}

class SwitchSetting extends StatelessWidget {
  const SwitchSetting(
    this.label, {
    super.key,
    required this.value,
    this.onChanged,
    this.hint,
    this.stateLabel,
  });
  final String label;
  final bool value;
  final ValueChanged<bool>? onChanged;
  final String? hint;
  final String? stateLabel;

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      Row(
        children: [
          Expanded(child: Text(label)),
          if (stateLabel != null) ...[
            Text(stateLabel!),
            const SizedBox(width: 8),
          ],
          FSwitch(
            semanticsLabel: AppStrings.toggle(label),
            value: value,
            enabled: onChanged != null,
            onChange: onChanged,
          ),
        ],
      ),
      if (hint != null) ...[
        const SizedBox(height: 8),
        Text(
          hint!,
          style: context.theme.typography.body.xs.copyWith(
            color: context.theme.colors.mutedForeground,
            height: 1.5,
          ),
        ),
      ],
    ],
  );
}

class SliderSetting extends StatefulWidget {
  const SliderSetting({
    super.key,
    required this.label,
    required this.value,
    required this.min,
    required this.max,
    required this.divisions,
    this.unit = '',
    this.onChanged,
    this.semanticLabel,
    this.icon = FLucideIcons.slidersHorizontal,
  });
  final String label;
  final double value;
  final double min;
  final double max;
  final int divisions;
  final String unit;
  final ValueChanged<double>? onChanged;
  final String? semanticLabel;
  final IconData icon;
  @override
  State<SliderSetting> createState() => _SliderSettingState();
}

class _SliderSettingState extends State<SliderSetting> {
  double? _draft;
  double _value(double fraction) =>
      widget.min +
      (fraction * widget.divisions).round() /
          widget.divisions *
          (widget.max - widget.min);
  String _label(double value) => '${value.round()}${widget.unit}';
  @override
  void didUpdateWidget(covariant SliderSetting oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.value != widget.value || widget.onChanged == null) {
      _draft = null;
    }
  }

  @override
  Widget build(BuildContext context) {
    final value = (_draft ?? widget.value).clamp(widget.min, widget.max);
    return Column(
      children: [
        Row(
          children: [
            Icon(widget.icon, size: 16),
            const SizedBox(width: 8),
            Expanded(child: Text(widget.label)),
            Text(_label(value)),
          ],
        ),
        const SizedBox(height: 12),
        FSlider(
          enabled: widget.onChanged != null,
          control: FSliderControl.liftedContinuous(
            value: FSliderValue(
              max: (value - widget.min) / (widget.max - widget.min),
            ),
            stepPercentage: 1 / widget.divisions,
            onChange: (value) => setState(() => _draft = _value(value.max)),
          ),
          tooltipBuilder: (_, value) => Text(_label(_value(value))),
          semanticValueFormatterCallback: (value) =>
              '${widget.semanticLabel ?? widget.label} ${_label(_value(value))}',
          onEnd: widget.onChanged == null
              ? null
              : (value) {
                  widget.onChanged!(_value(value.max));
                  if (mounted) setState(() => _draft = null);
                },
        ),
      ],
    );
  }
}

class ActionButton extends StatelessWidget {
  const ActionButton(
    this.label, {
    super.key,
    this.onPress,
    this.icon,
    this.primary = false,
    this.tooltip,
  });
  final String label;
  final VoidCallback? onPress;
  final IconData? icon;
  final bool primary;
  final String? tooltip;
  @override
  Widget build(BuildContext context) {
    final button = FButton(
      onPress: onPress,
      variant: primary ? FButtonVariant.primary : FButtonVariant.outline,
      mainAxisSize: MainAxisSize.min,
      prefix: icon == null ? null : Icon(icon, size: 16),
      child: Text(label),
    );
    return tooltip == null
        ? button
        : FTooltip(tipBuilder: (_, _) => Text(tooltip!), child: button);
  }
}

class ActionRow extends StatelessWidget {
  const ActionRow({super.key, required this.children});
  final List<Widget> children;
  @override
  Widget build(BuildContext context) => Wrap(
    spacing: 8,
    runSpacing: 8,
    crossAxisAlignment: WrapCrossAlignment.center,
    children: children,
  );
}

class ValueRow extends StatelessWidget {
  const ValueRow(this.label, this.value, {super.key});
  final String label;
  final String value;
  @override
  Widget build(BuildContext context) => Row(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      SizedBox(width: 100, child: Text(label)),
      Expanded(
        child: SelectableText(
          value,
          style: context.theme.typography.body.sm.copyWith(
            color: context.theme.colors.mutedForeground,
          ),
        ),
      ),
    ],
  );
}

class AppPageBody extends StatelessWidget {
  const AppPageBody({
    super.key,
    required this.controller,
    required this.page,
    required this.children,
  });
  final AppController controller;
  final AppPage page;
  final List<Widget> children;
  @override
  Widget build(BuildContext context) => SingleChildScrollView(
    key: PageStorageKey(page.id),
    primary: false,
    padding: const EdgeInsets.fromLTRB(20, 12, 20, 28),
    child: Align(
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 720),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          spacing: 24,
          children: [
            if (controller.appUpdate?.hasUpdate == true)
              AppUpdateBanner(controller: controller),
            if (controller.isAndroid && controller.unhandledError != null)
              ErrorMessage(controller.unhandledError),
            ...children,
          ],
        ),
      ),
    ),
  );
}

class AppUpdateBanner extends StatelessWidget {
  const AppUpdateBanner({super.key, required this.controller});
  final AppController controller;
  @override
  Widget build(BuildContext context) {
    final update = controller.appUpdate!;
    return FTileGroup(
      children: [
        FTile(
          prefix: const Icon(FLucideIcons.arrowUpCircle),
          title: const Text(AppStrings.appUpdate),
          subtitle: Wrap(
            spacing: 8,
            runSpacing: 4,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              FBadge(child: Text('v${update.latestVersion}')),
              Text(AppStrings.currentVersion(update.currentVersion)),
            ],
          ),
          suffix: const Icon(FLucideIcons.externalLink, size: 16),
          onPress: controller.openAppUpdate,
        ),
      ],
    );
  }
}

class AppBrand extends StatelessWidget {
  const AppBrand({super.key, required this.version, this.compact = false});
  final String version;
  final bool compact;
  @override
  Widget build(BuildContext context) => Row(
    children: [
      Image.asset(
        logoAsset,
        width: compact ? 28 : 48,
        height: compact ? 28 : 48,
        semanticLabel: 'KeyTao',
      ),
      const SizedBox(width: 10),
      Expanded(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              AppStrings.title,
              style: context.theme.typography.body.sm.copyWith(
                fontWeight: FontWeight.w600,
              ),
            ),
            Text(
              'v$version',
              style: context.theme.typography.body.xs.copyWith(
                color: context.theme.colors.mutedForeground,
              ),
            ),
            if (!compact) ...[
              const SizedBox(height: 6),
              Text(AppStrings.tagline, style: context.theme.typography.body.xs),
            ],
          ],
        ),
      ),
    ],
  );
}

Color successColor(BuildContext context) =>
    Theme.of(context).brightness == Brightness.dark
    ? const Color(0xFF86EFAC)
    : const Color(0xFF166534);
Color warningColor(BuildContext context) =>
    Theme.of(context).brightness == Brightness.dark
    ? const Color(0xFFFCD34D)
    : const Color(0xFF92400E);

class StatusMessage extends StatelessWidget {
  const StatusMessage(
    this.message, {
    super.key,
    this.icon = FLucideIcons.info,
    this.color,
  });
  final String message;
  final IconData icon;
  final Color? color;
  @override
  Widget build(BuildContext context) => Semantics(
    liveRegion: true,
    child: Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(
            icon,
            size: 16,
            color: color ?? context.theme.colors.mutedForeground,
          ),
          const SizedBox(width: 8),
          Expanded(
            child: SelectableText(
              message,
              style: context.theme.typography.body.sm.copyWith(
                color: color ?? context.theme.colors.mutedForeground,
                height: 1.5,
              ),
            ),
          ),
        ],
      ),
    ),
  );
}

class ErrorMessage extends StatelessWidget {
  const ErrorMessage(this.message, {super.key});
  final String? message;
  @override
  Widget build(BuildContext context) => message?.isNotEmpty == true
      ? StatusMessage(
          message!,
          icon: FLucideIcons.circleAlert,
          color: context.theme.colors.destructive,
        )
      : const SizedBox.shrink();
}

class LogLines extends StatelessWidget {
  const LogLines({
    super.key,
    required this.lines,
    required this.storageKey,
    this.height = 240,
    this.colorCoded = false,
  });
  final List<String> lines;
  final String storageKey;
  final double height;
  final bool colorCoded;
  @override
  Widget build(BuildContext context) => SizedBox(
    height: height,
    child: lines.isEmpty
        ? const Center(child: Text(AppStrings.noLogs))
        : ListView.builder(
            key: PageStorageKey(storageKey),
            primary: false,
            itemCount: lines.length,
            itemBuilder: (context, index) => Padding(
              padding: const EdgeInsets.only(bottom: 6),
              child: SelectableText(
                lines[index],
                style: context.theme.typography.body.xs.copyWith(
                  fontFamily: 'monospace',
                  color: !colorCoded
                      ? null
                      : switch (operationLogKind(lines[index])) {
                          LogLineKind.error => context.theme.colors.destructive,
                          LogLineKind.warning => warningColor(context),
                          LogLineKind.deploy => successColor(context),
                          LogLineKind.merged => context.theme.colors.primary,
                          LogLineKind.normal => context.theme.colors.foreground,
                        },
                ),
              ),
            ),
          ),
  );
}

class AppDialog extends StatelessWidget {
  const AppDialog({
    super.key,
    required this.title,
    required this.child,
    required this.actions,
    required this.animation,
  });
  final String title;
  final Widget child;
  final List<Widget> actions;
  final Animation<double> animation;
  @override
  Widget build(BuildContext context) => FDialog(
    animation: animation,
    builder: (context, _) => Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            title,
            style: context.theme.typography.display.lg.copyWith(
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(height: 16),
          Flexible(child: SingleChildScrollView(child: child)),
          const SizedBox(height: 20),
          Align(
            alignment: Alignment.centerRight,
            child: ActionRow(children: actions),
          ),
        ],
      ),
    ),
  );
}
