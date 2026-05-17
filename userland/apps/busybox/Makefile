# BusyBox — single-binary coreutils for the ring-3 userland.
#
# Builds BusyBox 1.36.1 statically against musl via the host's
# x86_64-linux-musl cross-compiler. The kernel ELF loader only accepts
# ET_EXEC (-static -no-pie); the readelf check in build.sh asserts that.
#
# Recipe:
#   1. fetch + verify the busybox tarball into build/tarballs/.
#   2. extract into build/busybox-<ver>/.
#   3. `make defconfig` for a known starting point.
#   4. apply busybox.config overrides (static link, disable unsupported
#      applets — networking, init, daemons, module loading, mount, etc.)
#      then `make oldconfig` to canonicalize.
#   5. `make CC=$(MUSL_CC) CONFIG_EXTRA_LDFLAGS="-no-pie"` builds the
#      busybox binary.
#   6. strip the resulting binary and copy to build/busybox.
#
# Override the toolchain by setting MUSL_CC:
#   make MUSL_CC=x86_64-linux-musl-gcc-13

MUSL_CC      ?= x86_64-linux-musl-gcc
# Strip the trailing "gcc" from MUSL_CC to derive the toolchain prefix
# BusyBox expects via CROSS_COMPILE — that hooks up ar/ranlib/strip/ld
# in one go. Default toolchains usually live under
# x86_64-linux-musl-{gcc,ar,ranlib,strip,ld}.
CROSS_COMPILE ?= $(patsubst %gcc,%,$(MUSL_CC))
MUSL_STRIP   ?= $(CROSS_COMPILE)strip

BUSYBOX_VERSION := 1.36.1
BUSYBOX_SHA256  := b8cc24c9574d809e7279c3be349795c5d5ceb6fdf19ca709f80cde50e47de314
BUSYBOX_URL     := https://busybox.net/downloads/busybox-$(BUSYBOX_VERSION).tar.bz2

OUT_DIR         := build
TARBALLS_DIR    := $(OUT_DIR)/tarballs
BUSYBOX_SRC     := $(OUT_DIR)/busybox-$(BUSYBOX_VERSION)
BUSYBOX_BUILT   := $(BUSYBOX_SRC)/busybox
BIN             := $(OUT_DIR)/busybox

OVERRIDES       := $(CURDIR)/busybox.config

# Build CPU count — match the zsh app's pattern.
NCPU := $(shell sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)

.PHONY: all clean distclean fetch config
.DELETE_ON_ERROR:

all: $(BIN)

# ---------------- fetch + extract ----------------

fetch: $(TARBALLS_DIR)/busybox-$(BUSYBOX_VERSION).tar.bz2

$(TARBALLS_DIR):
	mkdir -p $@

$(TARBALLS_DIR)/busybox-$(BUSYBOX_VERSION).tar.bz2: | $(TARBALLS_DIR)
	@echo "📥 Fetching busybox-$(BUSYBOX_VERSION)..."
	curl -fsSL --max-time 120 -o $@.tmp $(BUSYBOX_URL)
	@actual=$$(shasum -a 256 $@.tmp | awk '{print $$1}'); \
	  if [ "$$actual" != "$(BUSYBOX_SHA256)" ]; then \
	    echo "❌ busybox SHA256 mismatch:"; echo "   expected $(BUSYBOX_SHA256)"; echo "   got      $$actual"; \
	    rm -f $@.tmp; exit 1; \
	  fi
	mv $@.tmp $@

$(BUSYBOX_SRC)/Makefile: $(TARBALLS_DIR)/busybox-$(BUSYBOX_VERSION).tar.bz2
	rm -rf $(BUSYBOX_SRC)
	tar xjf $< -C $(OUT_DIR)
	@touch $@

# ---------------- configure ----------------

# `defconfig` first (predictable starting point across busybox versions),
# then append our overrides and run `oldconfig` so any conflicting
# defaults are resolved deterministically.
$(BUSYBOX_SRC)/.config: $(BUSYBOX_SRC)/Makefile $(OVERRIDES)
	@echo "⚙️  Configuring busybox-$(BUSYBOX_VERSION)..."
	$(MAKE) -C $(BUSYBOX_SRC) defconfig >/dev/null
	@# Strip each CONFIG_FOO=... line from the current config so the
	@# override wins (`>>` alone would leave duplicates and oldconfig
	@# picks the *first*, not the last).
	@awk -F= '/^CONFIG_[A-Z0-9_]+=/{print $$1}' $(OVERRIDES) | \
	  while read key; do \
	    sed -i.bak -e "/^$$key=/d" -e "/^# $$key /d" $(BUSYBOX_SRC)/.config; \
	  done
	@rm -f $(BUSYBOX_SRC)/.config.bak
	cat $(OVERRIDES) >> $(BUSYBOX_SRC)/.config
	$(MAKE) -C $(BUSYBOX_SRC) oldconfig >/dev/null

config: $(BUSYBOX_SRC)/.config
	@echo "✅ .config materialized at $(BUSYBOX_SRC)/.config"

# ---------------- build ----------------

$(BUSYBOX_BUILT): $(BUSYBOX_SRC)/.config
	@echo "🔨 Building busybox-$(BUSYBOX_VERSION)..."
	$(MAKE) -C $(BUSYBOX_SRC) -j$(NCPU) \
	  CROSS_COMPILE=$(CROSS_COMPILE) \
	  HOSTCC=cc \
	  CONFIG_EXTRA_CFLAGS="-fno-pie" \
	  CONFIG_EXTRA_LDFLAGS="-no-pie" \
	  SKIP_STRIP=y \
	  busybox

$(BIN): $(BUSYBOX_BUILT)
	cp $< $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@
	@size=$$(wc -c < $@ | tr -d ' '); echo "✅ Built $@ ($$size bytes, stripped)"

# ---------------- maintenance ----------------

clean:
	rm -rf $(BUSYBOX_SRC) $(BIN)

distclean:
	rm -rf $(OUT_DIR)
