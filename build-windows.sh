#!/usr/bin/env bash
#
# build-windows.sh — 交叉编译 gdb-rs 为 Windows x64 命令行 exe
#
# 原理：本机（Linux）直连 static.rust-lang.org 不可用时，改用 Docker 容器 +
#       USTC 镜像安装 x86_64-pc-windows-gnu 目标与 mingw 链接器，再交叉编译。
#
# 用法：
#   bash build-windows.sh
#
# 可覆盖的环境变量：
#   IMAGE              基础镜像（默认 rust:latest）
#   RUSTUP_DIST_SERVER Rust dist 镜像（默认 USTC）
#   DEBIAN_MIRROR      Debian 包镜像（默认 USTC）
#
# 产物：dist/gdb.exe（Windows 10/11 x64，自包含，无第三方 DLL 依赖）

set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
IMAGE="${IMAGE:-rust:latest}"
RUSTUP_MIRROR="${RUSTUP_DIST_SERVER:-https://mirrors.ustc.edu.cn/rust-static}"
DEBIAN_MIRROR="${DEBIAN_MIRROR:-https://mirrors.ustc.edu.cn/debian}"
TARGET="x86_64-pc-windows-gnu"
DIST="$ROOT/dist"
OUT_EXE="$ROOT/target/$TARGET/release/gdb.exe"

if ! command -v docker >/dev/null 2>&1; then
  echo "错误：未找到 docker。请先安装 Docker，或在 Windows 本机执行 'cargo build --release -p gdb_cli'。" >&2
  exit 1
fi

if ! docker info >/dev/null 2>&1; then
  echo "错误：Docker 守护进程未运行。" >&2
  exit 1
fi

echo "==> 使用镜像: $IMAGE"
echo "==> Rust dist 镜像: $RUSTUP_MIRROR"
echo "==> Debian 镜像: $DEBIAN_MIRROR"
echo "==> 目标: $TARGET"

mkdir -p "$DIST"

docker run --rm \
  -v "$ROOT:/work" \
  -w /work \
  -e "RUSTUP_DIST_SERVER=$RUSTUP_MIRROR" \
  -e "RUSTUP_UPDATE_ROOT=$RUSTUP_MIRROR/rustup" \
  -e "DEBIAN_MIRROR=$DEBIAN_MIRROR" \
  "$IMAGE" bash -c '
    set -euo pipefail

    echo "--> [1/4] 安装 Windows 目标 rust-std"
    rustup target add x86_64-pc-windows-gnu

    echo "--> [2/4] 配置 Debian 镜像并安装 mingw 链接器"
    printf "deb %s bookworm main\ndeb %s bookworm-updates main\n" \
      "$DEBIAN_MIRROR" "$DEBIAN_MIRROR" > /etc/apt/sources.list
    rm -f /etc/apt/sources.list.d/* 2>/dev/null || true
    apt-get update -qq
    apt-get install -y -qq gcc-mingw-w64-x86-64

    echo "--> [3/4] 交叉编译 release"
    cargo build --release --target x86_64-pc-windows-gnu -p gdb_cli

    echo "--> [4/4] 完成"
    ls -la target/x86_64-pc-windows-gnu/release/gdb.exe
  '

if [ ! -f "$OUT_EXE" ]; then
  echo "错误：未找到构建产物 $OUT_EXE" >&2
  exit 1
fi

cp "$OUT_EXE" "$DIST/gdb.exe"
echo
echo "==> 已生成: $DIST/gdb.exe"
echo "    可将该文件复制到 Windows 10/11 x64 直接运行（自包含，无需额外 DLL）。"
