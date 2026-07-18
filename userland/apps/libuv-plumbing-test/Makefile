MUSL_CC ?= x86_64-linux-musl-gcc
READELF ?= $(patsubst %gcc,%readelf,$(MUSL_CC))

CFLAGS := -O2 -Wall -Wextra -Werror -fno-pie
LDFLAGS := -static -no-pie
OUT_DIR := build
BIN := $(OUT_DIR)/UVPLUMB.ELF

.PHONY: all validate clean
.DELETE_ON_ERROR:

all: $(BIN)

$(OUT_DIR):
	mkdir -p $@

$(BIN): src/libuv_plumbing_test.c | $(OUT_DIR)
	$(MUSL_CC) $(CFLAGS) $< -o $@ $(LDFLAGS)

validate: all
	@type=$$($(READELF) -h $(BIN) | awk '/Type:/ { print $$2 }'); \
	 machine=$$($(READELF) -h $(BIN) | awk '/Machine:/ { print $$2 }'); \
	 if [ "$$type" != "EXEC" ] || [ "$$machine" != "Advanced" ]; then \
	   echo "invalid libuv plumbing fixture ELF: $(BIN) (type=$$type machine=$$machine)" >&2; exit 1; \
	 fi
	@if $(READELF) -l $(BIN) | grep -q INTERP; then \
	  echo "libuv plumbing fixture must not contain PT_INTERP: $(BIN)" >&2; exit 1; \
	fi

clean:
	rm -rf $(OUT_DIR)
