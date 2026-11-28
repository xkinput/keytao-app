import 'dart:io';

class KeyTaoUninstaller {
  static const _shellCommand =
      r'''set -e; main_app="/Applications/KeyTao.app"; ime_app="/Library/Input Methods/KeyTao.app"; if [ ! -e "$main_app" ] && [ ! -L "$main_app" ] && [ ! -e "$ime_app" ] && [ ! -L "$ime_app" ]; then echo "未找到 KeyTao 官方安装包" >&2; exit 1; fi; lsregister="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"; for app in "$main_app" "$ime_app"; do if [ -d "$app" ]; then "$lsregister" -u "$app" >/dev/null 2>&1 || true; fi; done; /usr/bin/killall KeyTaoIME >/dev/null 2>&1 || true; /usr/bin/killall imklaunchagent >/dev/null 2>&1 || true; /usr/bin/killall TextInputMenuAgent >/dev/null 2>&1 || true; /bin/rm -rf -- "$main_app" "$ime_app"; /usr/sbin/pkgutil --forget ink.rea.keytao-app.pkg >/dev/null 2>&1 || true''';

  static Future<void> uninstall({required bool clearUserData}) async {
    if (!Platform.isMacOS) {
      throw UnsupportedError('KeyTao 应用内卸载目前仅支持 macOS。');
    }

    final appleScript =
        'do shell script "${_escapeAppleScript(_shellCommand)}" with administrator privileges';
    final result = await Process.run('/usr/bin/osascript', ['-e', appleScript]);
    if (result.exitCode != 0) {
      final message = result.stderr.toString().trim();
      if (_wasCancelled(message)) {
        throw const KeyTaoUninstallCancelled();
      }
      throw StateError(message.isEmpty ? '系统卸载命令失败' : message);
    }

    if (clearUserData) await _clearUserData();
  }

  static String _escapeAppleScript(String value) =>
      value.replaceAll(r'\', r'\\').replaceAll('"', r'\"');

  static bool _wasCancelled(String message) {
    final lower = message.toLowerCase();
    return lower.contains('cancel') || message.contains('取消');
  }

  static Future<void> _clearUserData() async =>
      clearUserDataUnderHome(Platform.environment['HOME']?.trim() ?? '');

  static Future<void> clearUserDataUnderHome(String home) async {
    if (!home.startsWith('/')) {
      throw StateError('无法确认当前用户目录，已保留个人资料。');
    }
    final resolvedHome = await Directory(home).resolveSymbolicLinks();
    if (resolvedHome == '/') {
      throw StateError('无法确认当前用户目录，已保留个人资料。');
    }
    final library = '$resolvedHome/Library';
    final paths = [
      '$library/keytao',
      '$library/Application Support/ink.rea.keytao-app',
      '$library/Caches/ink.rea.keytao-app',
      '$library/Preferences/ink.rea.keytao-app.plist',
    ];

    final failures = <String>[];
    for (final path in paths) {
      try {
        await _deleteEntity(path, library);
      } on FileSystemException {
        failures.add(path);
      }
    }
    if (failures.isNotEmpty) {
      throw StateError('应用已移除，但以下个人资料未能删除：${failures.join('、')}');
    }
  }

  static Future<void> _deleteEntity(String path, String library) async {
    final normalized = Directory(path).absolute.path;
    if (!normalized.startsWith('$library/')) {
      throw FileSystemException('拒绝删除 Library 目录之外的路径', path);
    }
    final type = await FileSystemEntity.type(path, followLinks: false);
    switch (type) {
      case FileSystemEntityType.notFound:
        return;
      case FileSystemEntityType.directory:
        await Directory(path).delete(recursive: true);
      case FileSystemEntityType.file:
        await File(path).delete();
      case FileSystemEntityType.link:
        await Link(path).delete();
      case FileSystemEntityType.pipe:
      case FileSystemEntityType.unixDomainSock:
        throw FileSystemException('拒绝删除非普通文件', path);
      default:
        throw FileSystemException('无法识别文件类型，已拒绝删除', path);
    }
  }
}

class KeyTaoUninstallCancelled implements Exception {
  const KeyTaoUninstallCancelled();
}
