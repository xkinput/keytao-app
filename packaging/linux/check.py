#!/usr/bin/env python3
"""Offline packaging fixtures and the extracted-package layout contract."""
import configparser
import hashlib
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET

PROJECT = Path(__file__).resolve().parents[2]


def desktop(path):
    config = configparser.ConfigParser(interpolation=None)
    config.optionxform = str
    config.read(path)
    return config['Desktop Entry']


def check_layout(root):
    app = root / 'usr/lib/KeyTao'
    for name in ('keytao-app', 'keytao-ime'):
        entry = root / 'usr/bin' / name
        assert entry.is_symlink() and os.readlink(entry) == f'../lib/KeyTao/{name}', entry
        assert os.access(entry, os.X_OK), entry
    for name in ('lib/libflutter_linux_gtk.so', 'lib/libapp.so',
                 'lib/libkeytao_app_bridge.so', 'data/icudtl.dat',
                 'runtime/default-theme.yaml', 'runtime/rime-data/default.yaml',
                 'runtime/rime-data/key_bindings.yaml', 'runtime/rime-data/punctuation.yaml',
                 'runtime/rime-data/symbols.yaml', 'runtime/rime-data/essay.txt',
                 'runtime/lib/librime.so.1', 'runtime/lib/libmarisa.so.0',
                 'runtime/lib/libopencc.so.1.1', 'runtime/lib/libyaml-cpp.so.0.8',
                 'runtime/lib/liblua5.4.so.0', 'runtime/lib/libglog.so.1',
                 'runtime/lib/libleveldb.so.1d', 'runtime/lib/rime-plugins/librime-lua.so'):
        assert (app / name).is_file(), name
    assert any((app / 'data/flutter_assets').iterdir()), 'Flutter assets'
    assert any((app / 'runtime/rime-data/opencc').iterdir()), 'OpenCC data'
    assert (app / 'rime-data').is_symlink()
    assert os.readlink(app / 'rime-data') == 'runtime/rime-data'
    assert not (root / 'usr/lib/keytao-app').exists(), 'Wrong historical prefix'
    assert not (root / 'DEBIAN/postinst').exists(), 'Unexpected postinst'
    for source in (PROJECT / 'resources/addon-schemas').rglob('*'):
        if source.is_file():
            target = app / 'addon-schemas' / source.relative_to(PROJECT / 'resources/addon-schemas')
            assert target.read_bytes() == source.read_bytes(), target
    entry = desktop(root / 'usr/share/applications/KeyTao.desktop')
    assert dict(entry) == {
        'Categories': '', 'Comment': 'KeyTao input method app', 'Exec': 'keytao-app',
        'StartupWMClass': 'keytao-app', 'Icon': 'keytao-app', 'Name': 'KeyTao',
        'Terminal': 'false', 'Type': 'Application',
    }, dict(entry)
    autostart = desktop(root / 'etc/xdg/autostart/keytao-ime.desktop')
    assert autostart['Exec'] == 'keytao-ime'
    assert autostart['NotShowIn'] == 'GNOME;'
    assert autostart['Type'] == 'Application'
    launcher = root / 'usr/share/applications/keytao-wayland-launcher.desktop'
    assert launcher.read_bytes() == (PROJECT / 'crates/keytao-linux-ime/keytao-wayland-launcher.desktop').read_bytes()
    assert desktop(launcher)['X-KDE-Wayland-VirtualKeyboard'] == 'true'
    component = root / 'usr/share/ibus/component/keytao.xml'
    assert component.read_bytes() == (PROJECT / 'crates/keytao-linux-ime/keytao.xml').read_bytes()
    assert ET.parse(component).findtext('exec') == 'keytao-ime --ibus-engine'
    icons = sorted((PROJECT / 'packaging/icons').rglob('*.png'))
    assert icons, 'Missing packaging icons'
    for source in icons:
        width, height = struct.unpack('>II', source.read_bytes()[16:24])
        assert width == height
        installed = root / f'usr/share/icons/hicolor/{width}x{height}/apps/keytao-app.png'
        assert installed.is_file(), installed
        assert struct.unpack('>II', installed.read_bytes()[16:24]) == (width, height)
    # No packaged symlink may escape the extracted installation tree.
    for path in root.rglob('*'):
        if path.is_symlink():
            assert path.resolve().is_relative_to(root.resolve()) and path.exists(), path


def inventory(root):
    return {
        str(path.relative_to(root)): ('link', os.readlink(path)) if path.is_symlink()
        else ('file', hashlib.sha256(path.read_bytes()).hexdigest(), path.stat().st_mode & 0o777)
        for directory in ('usr', 'etc') for path in (root / directory).rglob('*')
        if path.is_symlink() or path.is_file()
    }


def fixtures():
    cmake = (PROJECT / 'flutter_app/linux/CMakeLists.txt').read_text()
    for setting in ('set(BINARY_NAME "keytao-app")', 'set(APPLICATION_ID "ink.rea.keytao-app")',
                    'set(CMAKE_INSTALL_RPATH "$ORIGIN/lib")'):
        assert setting in cmake, setting
    runner = (PROJECT / 'flutter_app/linux/runner/my_application.cc').read_text()
    assert 'gdk_set_program_class("keytao-app")' in runner
    assert 'gtk_window_set_title(window, "KeyTao")' in runner
    assert 'gtk_window_set_icon_name(window, "keytao-app")' in runner
    with tempfile.TemporaryDirectory(prefix='keytao-linux-check-') as directory:
        temp = Path(directory)
        bundle, runtime = temp / 'flutter', temp / 'runtime'
        for name in ('keytao-app', 'lib/libflutter_linux_gtk.so', 'lib/libapp.so',
                     'lib/libkeytao_app_bridge.so', 'data/icudtl.dat', 'data/flutter_assets/AssetManifest.bin'):
            path = bundle / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('fixture\n')
        (bundle / 'keytao-app').chmod(0o755)
        ime = temp / 'keytao-ime'
        ime.write_text('fixture\n')
        ime.chmod(0o755)
        for name in ('default-theme.yaml', 'rime-data/default.yaml', 'rime-data/key_bindings.yaml',
                     'rime-data/punctuation.yaml', 'rime-data/symbols.yaml', 'rime-data/essay.txt',
                     'rime-data/opencc/test.json',
                     'lib/librime.so.1', 'lib/libmarisa.so.0', 'lib/libopencc.so.1.1',
                     'lib/libyaml-cpp.so.0.8', 'lib/liblua5.4.so.0', 'lib/libglog.so.1',
                     'lib/libleveldb.so.1d', 'lib/rime-plugins/librime-lua.so'):
            path = runtime / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('fixture\n')
        for arch in ('x86_64', 'aarch64'):
            command = ['bash', str(PROJECT / 'packaging/linux/build-packages.sh'), str(bundle),
                       str(ime), str(runtime), str(temp / arch), '1.2.1-alpha.89', arch, '--stage-only']
            subprocess.run(command, check=True)
            root = temp / arch / 'root'
            check_layout(root)
            for relative in ('usr/lib/KeyTao/runtime/rime-data/default.yaml',
                             'usr/share/ibus/component/keytao.xml',
                             'etc/xdg/autostart/keytao-ime.desktop'):
                path = root / relative
                saved = path.read_bytes()
                path.unlink()
                try:
                    check_layout(root)
                except (AssertionError, KeyError, FileNotFoundError):
                    pass
                else:
                    raise AssertionError(f'Accepted missing {relative}')
                path.write_bytes(saved)
            command[6] = '1.2.1\nDepends: injected'
            result = subprocess.run(command, capture_output=True, text=True)
            assert result.returncode == 2 and 'Invalid package version' in result.stderr
        assert inventory(temp / 'x86_64/root') == inventory(temp / 'aarch64/root')
    print('PASS: both architecture staging fixtures, layout, missing-file and invalid-version rejection')


if __name__ == '__main__':
    if len(sys.argv) > 1:
        roots = [Path(arg) for arg in sys.argv[1:]]
        for root in roots:
            check_layout(root)
        if len(roots) == 2:
            assert inventory(roots[0]) == inventory(roots[1]), 'deb/rpm contents differ'
        print('PASS: extracted package layout and file inventory')
    else:
        fixtures()
