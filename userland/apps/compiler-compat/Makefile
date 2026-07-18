MUSL_CC ?= x86_64-linux-musl-gcc
READELF ?= $(patsubst %gcc,%readelf,$(MUSL_CC))
STRIP ?= $(patsubst %gcc,%strip,$(MUSL_CC))

CFLAGS := -O2 -Wall -Wextra -fno-pie
LDFLAGS := -static -no-pie
OUT_DIR := build
PREBUILT_DIR := ../../prebuilt/compiler-compat
FIXTURES := CCCRT CCLIBC CCPROBE
BINS := $(addprefix $(OUT_DIR)/,$(addsuffix .ELF,$(FIXTURES)))

.PHONY: all validate clean
.DELETE_ON_ERROR:

all: $(BINS)

$(OUT_DIR):
	mkdir -p $@

$(OUT_DIR)/CCCRT.ELF: src/crt.c | $(OUT_DIR)
	$(MUSL_CC) $(CFLAGS) $< -o $@ $(LDFLAGS)

$(OUT_DIR)/CCLIBC.ELF: src/libc.c | $(OUT_DIR)
	$(MUSL_CC) $(CFLAGS) $< -o $@ $(LDFLAGS)

$(OUT_DIR)/CCPROBE.ELF: src/probe.c | $(OUT_DIR)
	$(MUSL_CC) $(CFLAGS) $< -o $@ $(LDFLAGS)

validate: all
	@for bin in $(BINS); do \
		type=$$($(READELF) -h $$bin | awk '/Type:/ { print $$2 }'); \
		machine=$$($(READELF) -h $$bin | awk '/Machine:/ { print $$2 }'); \
		if [ "$$type" != "EXEC" ] || [ "$$machine" != "Advanced" ]; then \
			echo "invalid fixture ELF: $$bin (type=$$type machine=$$machine)" >&2; exit 1; \
		fi; \
		if $(READELF) -l $$bin | grep -q INTERP; then \
			echo "fixture must not contain PT_INTERP: $$bin" >&2; exit 1; \
		fi; \
	done

clean:
	rm -rf $(OUT_DIR)
