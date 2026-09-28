Name: key-tao
Version: %{keytao_version}
Release: 1
Summary: KeyTao input method app
License: LicenseRef-KeyTao-Unspecified
URL: https://github.com/xkinput/keytao-app
Packager: Rea <hi@rea.ink>
Requires: gtk3, libEGL.so.1()(64bit), libGL.so.1()(64bit), procps-ng

# Preserve Flutter's already-built bundle and its relative library paths.
%global debug_package %{nil}
%global __os_install_post %{nil}
%global _build_id_links none

%description
KeyTao Flutter application and Linux input method daemon with bundled Rime data.

%install
mkdir -p "%{buildroot}"
cp -a "%{keytao_root}/." "%{buildroot}/"

%files
%defattr(-,root,root,-)
/usr/bin/keytao-app
/usr/bin/keytao-ime
/usr/lib/KeyTao
/usr/share/applications/KeyTao.desktop
/usr/share/applications/keytao-wayland-launcher.desktop
/usr/share/icons/hicolor/*/apps/keytao-app.png
/usr/share/ibus/component/keytao.xml
/etc/xdg/autostart/keytao-ime.desktop
