#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
pkgname="batteryremainingd"
pkgver="2.1.1"
tarball="${pkgname}-${pkgver}.tar.gz"

if [ ! -f "$tarball" ]; then
    echo "creating source tarball: $tarball"
    tar czf "$tarball" \
        --transform "s|^|${pkgname}-${pkgver}/|" \
        Cargo.toml Cargo.lock src/main.rs batteryremainingd.service
fi

makepkg -si "$@"
