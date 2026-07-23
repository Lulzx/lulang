# lutelegram

`lutelegram` is a document-generated Telegram Bot API client for LuLang. The
checked-in API snapshot is parsed from the
[official Telegram Bot API documentation](https://core.telegram.org/bots/api).
Running the generator again updates method functions, parameter and response
records, object types, union specifications, comments, and field accessors.

The current snapshot contains 362 object types, 26 unions, and 185 methods from
Telegram Bot API 10.2, released July 14, 2026.

## Architecture

The pipeline deliberately separates facts from mechanics:

1. `tools/telegram_doc.py` fetches or reads the official HTML and writes
   `api/bot_api_doc.json`.
2. `tools/telegram_codegen.py` consumes only that structured JSON.
3. `src/runtime.lu` provides the small handwritten JSON layer.
4. The generator writes `src/lib.lu`, including the complete documented API.
5. `native/lutelegram_runtime.c` performs HTTPS requests with libcurl.

Telegram objects are JSON-backed LuLang records:

```lu
let user = response.result
if response.ok {
  print(user_get_id(user), user_get_first_name(user))
}
```

This representation is intentional. Telegram's schema is recursive
(`Message.reply_to_message` is another `Message`), while LuLang records have
finite value layouts. A generated record containing `json: str` plus generated
typed accessors preserves the complete schema without introducing unsafe
recursive layouts.

Every method has a generated parameter record and response record. Required
primitive fields are typed. Required objects use their generated wrapper type.
Arrays and mixed unions use a JSON fragment. Optional parameters are passed as
one JSON object so Bot API additions remain source-compatible:

```lu
let params = SendMessageParams {
  // chat_id accepts Integer or String, so this is a JSON fragment.
  chat_id: "-1001234567890",
  text: "Hello from LuLang",
  optional: "{\"disable_notification\":true}",
}
let response = send_message(arg(0), params)
if not response.ok {
  print(response.error.error_code, response.error.description)
}
```

## Build and verify

The native bridge needs libcurl headers and library:

```sh
cd lib/lutelegram
make
make check
cargo run --quiet --manifest-path ../../Cargo.toml -- test --runs 100
```

When running a bot, put `native/` on the platform dynamic-library search path:

```sh
DYLD_LIBRARY_PATH=/path/to/lulang/lib/lutelegram/native lu run bot.lu BOT_TOKEN
```

Use `LD_LIBRARY_PATH` instead on Linux. Keep the token in an environment or
secret manager in production; the positional argument above is only a compact
example.

## Regenerate from Telegram

```sh
python3 tools/telegram_doc.py lib/lutelegram/api/bot_api_doc.json
python3 tools/telegram_codegen.py \
  lib/lutelegram/api/bot_api_doc.json \
  lib/lutelegram/src/lib.lu \
  --runtime lib/lutelegram/src/runtime.lu
```

Generation fails if Telegram changes the document structure enough to produce
an incomplete schema or a method return type cannot be determined. This makes
documentation drift visible instead of silently emitting a partial client.

## Current boundary

The transport currently sends JSON requests. Telegram `file_id` and HTTP URL
inputs work, but local multipart file uploads are not implemented yet.
Long-polling is available through the generated `get_updates` method; the bot's
dispatch loop remains ordinary LuLang code because LuLang does not yet have
first-class callbacks or async tasks.
