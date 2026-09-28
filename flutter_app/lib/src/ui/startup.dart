import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:forui/forui.dart';

import '../app/strings.dart';
import '../rust/api/types.dart';
import 'menus.dart';
import 'theme.dart';

class AppStartup extends StatelessWidget {
  const AppStartup({super.key, required this.platform, this.error});
  final BridgePlatform platform;
  final String? error;
  @override
  Widget build(BuildContext context) => KeyTaoTheme(
    platform: platform,
    home: AppMenus(
      platform: platform,
      child: FScaffold(
        child: Center(
          child: Padding(
            padding: const EdgeInsets.all(24),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (error == null) ...[
                  const SizedBox(width: 180, child: FProgress()),
                  const SizedBox(height: 16),
                  const Text(AppStrings.boot),
                ] else ...[
                  FAlert(
                    title: const Text('启动失败'),
                    subtitle: SelectableText(error!),
                  ),
                  if (platform == BridgePlatform.macOs) ...[
                    const SizedBox(height: 16),
                    FButton(
                      onPress: SystemNavigator.pop,
                      child: const Text('关闭'),
                    ),
                  ],
                ],
              ],
            ),
          ),
        ),
      ),
    ),
  );
}
