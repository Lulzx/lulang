PYTHON ?= python3
CC ?= cc
UNAME_S := $(shell uname -s)

ifeq ($(UNAME_S),Darwin)
NATIVE_LIB := native/liblutelegram_runtime.dylib
SHARED_FLAGS := -dynamiclib
else
NATIVE_LIB := native/liblutelegram_runtime.so
SHARED_FLAGS := -shared
endif

.PHONY: all generate native check clean

all: generate native

generate: api/bot_api_doc.json src/runtime.lu
	$(PYTHON) ../../tools/telegram_codegen.py api/bot_api_doc.json src/lib.lu --runtime src/runtime.lu

api/bot_api_doc.json:
	$(PYTHON) ../../tools/telegram_doc.py $@

native: $(NATIVE_LIB)

$(NATIVE_LIB): native/lutelegram_runtime.c
	$(CC) -O2 -fPIC $(SHARED_FLAGS) -o $@ $< -lcurl

check: generate
	cargo run --quiet --manifest-path ../../Cargo.toml -- check src/lib.lu

clean:
	rm -f src/lib.lu native/liblutelegram_runtime.dylib native/liblutelegram_runtime.so
