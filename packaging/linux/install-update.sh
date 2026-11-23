#!/bin/bash
set -euo pipefail
export PATH=/usr/sbin:/usr/bin:/sbin:/bin LC_ALL=C
umask 077

fail() {
  printf '%s\n' "$1" >&2
  exit 1
}

install_package() {
  # A closed app should not break a running package manager's output pipes.
  if ! (umask 022; "$@") </dev/null > "$stage/install.log" 2>&1; then
    printf '%s\n' 'The system package manager could not install the update.' >&2
    /usr/bin/tail -n 8 "$stage/install.log" >&2
    exit 1
  fi
}

if [[ $EUID -ne 0 || $# -ne 2 ]]; then
  fail 'KeyTao update installation requires system authorization.'
fi
if [[ $1 != deb && $1 != rpm ]] || ! [[ $2 =~ ^[a-f0-9]{64}$ ]]; then
  fail 'Invalid KeyTao upgrade package format or checksum.'
fi

# Copy from the caller's open file into a root-owned directory before validation.
stage=$(/usr/bin/mktemp -d /tmp/keytao-update.XXXXXXXXXX)
trap '/usr/bin/rm -rf -- "$stage"' EXIT
trap 'exit 1' HUP INT TERM
package="$stage/key-tao.$1"
max_bytes=$((512 * 1024 * 1024))
/usr/bin/head -c "$((max_bytes + 1))" > "$package"
bytes=$(/usr/bin/stat -c %s -- "$package")
if (( bytes == 0 || bytes > max_bytes )); then
  fail 'Invalid KeyTao upgrade package size.'
fi
checksum=$(/usr/bin/sha256sum -- "$package")
[[ ${checksum%% *} == "$2" ]] || fail 'KeyTao upgrade package SHA-256 mismatch.'

case "$1" in
  deb)
    [[ $(/usr/bin/dpkg-deb -f "$package" Package) == key-tao ]] || fail 'Unexpected DEB package name.'
    version=$(/usr/bin/dpkg-deb -f "$package" Version)
    # shellcheck disable=SC2016
    format='${db:Status-Status}\t${Version}'
    installed=$(/usr/bin/dpkg-query -W -f="$format" key-tao)
    [[ $installed == installed$'\t'* ]] || fail 'KeyTao is not installed.'
    /usr/bin/dpkg --compare-versions "${installed#*$'\t'}" lt "$version" \
      || fail 'The DEB package must be newer than the installed package.'
    # APT's _apt user reads the root-owned archive without write access.
    /usr/bin/chmod 0755 "$stage"
    /usr/bin/chmod 0644 "$package"
    DEBIAN_FRONTEND=noninteractive install_package /usr/bin/apt-get -q -y --no-remove --only-upgrade \
      -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold \
      install "$package"
    [[ $(/usr/bin/dpkg-query -W -f="$format" key-tao) == installed$'\t'"$version" ]] \
      || fail 'The requested KeyTao DEB version was not installed.'
    ;;
  rpm)
    [[ $(/usr/bin/rpm -qp --queryformat '%{NAME}' -- "$package") == key-tao ]] || fail 'Unexpected RPM package name.'
    format='%{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\n'
    version=$(/usr/bin/rpm -qp --queryformat "$format" -- "$package")
    installed=$(/usr/bin/rpm -q --queryformat "$format" key-tao)
    [[ -n $installed && $installed != "$version" ]] || fail 'The RPM package must update the installed KeyTao.'
    install_package /usr/bin/rpm --freshen -- "$package"
    [[ $(/usr/bin/rpm -q --queryformat "$format" key-tao) == "$version" ]] \
      || fail 'The requested KeyTao RPM version was not installed.'
    ;;
esac
