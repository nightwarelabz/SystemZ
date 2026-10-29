#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
mkdir -p tools/boot-splash/out
python3 boot/embed.py boot/generated/splash_art.inc
gcc -std=c17 -O1 -g -Wall -Wextra -Werror -fsanitize=address,undefined \
  -fno-omit-frame-pointer -Iboot/generated boot/splash.c tools/boot-splash/host.c \
  -o tools/boot-splash/out/check
tools/boot-splash/out/check "$@"
make -C loader
