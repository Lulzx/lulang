#!/usr/bin/env sh
set -eu

if [ -z "${TELEGRAM_BOT_TOKEN:-}" ]; then
  echo "error: set TELEGRAM_BOT_TOKEN" >&2
  exit 2
fi

package_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_dir=$(CDPATH= cd -- "$package_dir/../.." && pwd)
combined=$(mktemp "${TMPDIR:-/tmp}/lutelegram-echo.XXXXXX.lu")
trap 'rm -f "$combined"' EXIT HUP INT TERM

make -s -C "$package_dir" native
python3 "$repo_dir/tools/telegram_codegen.py" \
  "$package_dir/api/bot_api_doc.json" "$combined" \
  --runtime "$package_dir/src/runtime.lu" \
  --only-method getMe \
  --only-method deleteWebhook \
  --only-method getUpdates \
  --only-method sendMessage \
  --only-type User \
  --only-type Update \
  --only-type Message \
  --only-type Chat
printf '\n' >>"$combined"
sed '1,2d' "$package_dir/examples/echo_bot.lu" >>"$combined"

if [ "$(uname -s)" = "Darwin" ]; then
  DYLD_LIBRARY_PATH="$package_dir/native${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}" \
    cargo run --quiet --manifest-path "$repo_dir/Cargo.toml" -- run "$combined" "$TELEGRAM_BOT_TOKEN" "$@"
else
  LD_LIBRARY_PATH="$package_dir/native${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
    cargo run --quiet --manifest-path "$repo_dir/Cargo.toml" -- run "$combined" "$TELEGRAM_BOT_TOKEN" "$@"
fi
