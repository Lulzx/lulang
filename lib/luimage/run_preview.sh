#!/usr/bin/env sh
set -eu

package_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$package_dir"
mkdir -p target
lu build --lib -o target/luimage src/lib.lu
cc -O2 -I target examples/render.c target/libluimage.a -o target/render
target/render target/mandelbrot.pgm
printf 'wrote %s\n' "$package_dir/target/mandelbrot.pgm"
