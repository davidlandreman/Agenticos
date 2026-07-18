MUSL_CC ?= x86_64-linux-musl-gcc
READELF ?= $(patsubst %gcc,%readelf,$(MUSL_CC))
STRIP ?= $(patsubst %gcc,%strip,$(MUSL_CC))

CFLAGS := -O2 -Wall -Wextra -Werror -fno-pie
LDFLAGS := -static -no-pie
OUT_DIR := build
BIN := $(OUT_DIR)/NETTEST.ELF
PREBUILT := ../../prebuilt/network/NETTEST.ELF

.PHONY: all validate refresh clean
.DELETE_ON_ERROR:

all: $(BIN)

$(OUT_DIR):
	mkdir -p $@

$(BIN): src/network_test.c | $(OUT_DIR)
	$(MUSL_CC) $(CFLAGS) $< -o $@ $(LDFLAGS)

validate: all
	@type=$$($(READELF) -h $(BIN) | awk '/Type:/ { print $$2 }'); \
	 machine=$$($(READELF) -h $(BIN) | awk '/Machine:/ { print $$2 }'); \
	 if [ "$$type" != "EXEC" ] || [ "$$machine" != "Advanced" ]; then \
	   echo "invalid network fixture ELF: $(BIN) (type=$$type machine=$$machine)" >&2; exit 1; \
	 fi
	@if $(READELF) -l $(BIN) | grep -q INTERP; then \
	  echo "network fixture must not contain PT_INTERP: $(BIN)" >&2; exit 1; \
	fi

refresh: validate
	$(STRIP) --strip-all $(BIN)
	mkdir -p $(dir $(PREBUILT))
	cp $(BIN) $(dir $(PREBUILT))/.NETTEST.ELF.tmp
	mv -f $(dir $(PREBUILT))/.NETTEST.ELF.tmp $(PREBUILT)

clean:
	rm -rf $(OUT_DIR)
