#!/bin/sh
set -eu
cd "$(dirname "$0")"

if ! pkg-config --exists raylib; then
  echo "raylib development files are required (pkg-config raylib)" >&2
  exit 1
fi

case "$(uname -s)" in
  Darwin)
    library="libraylib_luphysics.dylib"
    shared="-dynamiclib"
    ;;
  *)
    library="libraylib_luphysics.so"
    shared="-shared -fPIC"
    ;;
esac

# shellcheck disable=SC2046,SC2086
cc $shared -O2 -o "$library" examples/raylib_bridge.c \
  $(pkg-config --cflags --libs raylib)

combined=$(mktemp "${TMPDIR:-/tmp}/luphysics-raylib.XXXXXX.lu")
trap 'rm -f "$combined"' EXIT
cp src/lib.lu "$combined"
printf '\n' >> "$combined"
sed '/^use /d' examples/raylib.lu >> "$combined"

DYLD_LIBRARY_PATH="$PWD${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}" \
LD_LIBRARY_PATH="$PWD${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
lu run "$combined"
