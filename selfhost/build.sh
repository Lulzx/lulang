#!/bin/sh
# selfhost/build.sh — drive the self-hosted AOT compiler (selfhost/codegen.lu).
#
#   selfhost/build.sh prog.lu [-o out]   compile prog.lu -> ./out via the
#                                        self-hosted compiler + clang
#   selfhost/build.sh --bootstrap        3-stage self-compilation: codegen.lu
#                                        compiles itself, the result compiles
#                                        itself again, and the stage-2/stage-3
#                                        IR must be byte-identical; installs
#                                        the fixpoint binary as target/release/luc
set -e
cd "$(dirname "$0")/.."
LU=target/release/lu
[ -x "$LU" ] || cargo build --release
# the triple clang stamps on compiled modules (matches src/llvm.rs's probe)
TRIPLE=$(echo 'int lu_probe;' | clang -x c - -S -emit-llvm -o - 2>/dev/null |
  sed -n 's/^target triple = "\(.*\)"$/\1/p')
TMP=${TMPDIR:-/tmp}
RT="$TMP/lu_selfhost_runtime.o"
# The runtime ABI evolves with codegen.lu; rebuilding this small object avoids
# silently linking a stale cache entry after declarations change.
clang -O3 -mcpu=native -c src/lu_runtime.c -o "$RT"

if [ "$1" = "--bootstrap" ]; then
  echo "stage 1: lu run codegen.lu codegen.lu (interpreted self-compilation)"
  $LU run selfhost/codegen.lu selfhost/codegen.lu "$TRIPLE" > "$TMP/lu_cg1.ll"
  clang -O3 -mcpu=native -o "$TMP/lu_cg1" "$TMP/lu_cg1.ll" "$RT"
  echo "stage 2: the compiled compiler compiles its own source"
  "$TMP/lu_cg1" selfhost/codegen.lu "$TRIPLE" > "$TMP/lu_cg2.ll"
  clang -O3 -mcpu=native -o "$TMP/lu_cg2" "$TMP/lu_cg2.ll" "$RT"
  echo "stage 3: and again"
  "$TMP/lu_cg2" selfhost/codegen.lu "$TRIPLE" > "$TMP/lu_cg3.ll"
  cmp "$TMP/lu_cg2.ll" "$TMP/lu_cg3.ll"
  echo "fixpoint: stage-2 and stage-3 IR are byte-identical"
  cmp -s "$TMP/lu_cg1.ll" "$TMP/lu_cg2.ll" && echo "fixpoint: stage-1 (interpreted) matches too"
  cp "$TMP/lu_cg2" target/release/luc
  echo "self-hosted compiler installed: target/release/luc prog.lu \"\$TRIPLE\" > prog.ll"
  exit 0
fi

SRC=$1
OUT=$(basename "$SRC" .lu)
[ "$2" = "-o" ] && OUT=$3
STEM=$(basename "$SRC" .lu)
if [ -x target/release/luc ]; then
  target/release/luc "$SRC" "$TRIPLE" > "$TMP/lu_sh_$STEM.ll"
else
  $LU run selfhost/codegen.lu "$SRC" "$TRIPLE" > "$TMP/lu_sh_$STEM.ll"
fi
LINK_FLAGS=$(
  sed -n 's/^; link: //p' "$TMP/lu_sh_$STEM.ll" |
    sort -u |
    while IFS= read -r lib; do
      if [ "${lib#*/}" != "$lib" ] ||
        [ "${lib%.so}" != "$lib" ] ||
        [ "${lib%.dylib}" != "$lib" ]; then
        printf '%s\n' "$lib"
      else
        printf '%s\n' "-l$lib"
      fi
    done
)
# Link entries are compiler-produced library names. Like LU_LINK_FLAGS in the
# host driver, whitespace in a library path is intentionally unsupported.
# shellcheck disable=SC2086
clang -O3 -mcpu=native -o "$OUT" "$TMP/lu_sh_$STEM.ll" "$RT" $LINK_FLAGS
echo "$OUT"
