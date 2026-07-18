# Links 2 — text and native AgenticOS GUI browser.
#
# Cross-builds a static-musl binary with the no_std Rust AgenticOS driver and
# private, pinned image dependencies. The text-mode entry point remains intact.

MUSL_CC      ?= x86_64-linux-musl-gcc
MUSL_AR      ?= $(patsubst %-gcc,%-ar,$(MUSL_CC))
MUSL_RANLIB  ?= $(patsubst %-gcc,%-ranlib,$(MUSL_CC))
MUSL_STRIP   ?= $(patsubst %-gcc,%-strip,$(MUSL_CC))

LINKS_VERSION := 2.30
LINKS_SHA256  := c4631c6b5a11527cdc3cb7872fc23b7f2b25c2b021d596be410dadb40315f166
LINKS_URL     := https://links.twibright.com/download/links-$(LINKS_VERSION).tar.bz2
ZLIB_VERSION  := 1.3.2
ZLIB_SHA256   := bb329a0a2cd0274d05519d61c667c062e06990d72e125ee2dfa8de64f0119d16
ZLIB_URL      := https://zlib.net/fossils/zlib-$(ZLIB_VERSION).tar.gz
PNG_VERSION   := 1.6.58
PNG_SHA256    := 28eb403f51f0f7405249132cecfe82ea5c0ef97f1b32c5a65828814ae0d34775
PNG_URL       := https://download.sourceforge.net/libpng/libpng-$(PNG_VERSION).tar.xz

OUT_DIR        := build
TARBALLS_DIR   := $(OUT_DIR)/tarballs
LINKS_TARBALL  := $(TARBALLS_DIR)/links-$(LINKS_VERSION).tar.bz2
ZLIB_TARBALL   := $(TARBALLS_DIR)/zlib-$(ZLIB_VERSION).tar.gz
PNG_TARBALL    := $(TARBALLS_DIR)/libpng-$(PNG_VERSION).tar.xz
LINKS_SRC      := $(OUT_DIR)/links-$(LINKS_VERSION)
ZLIB_SRC       := $(OUT_DIR)/zlib-$(ZLIB_VERSION)
PNG_SRC        := $(OUT_DIR)/libpng-$(PNG_VERSION)
DEPS_PREFIX    := $(abspath $(OUT_DIR)/deps)
ZLIB_LIB       := $(DEPS_PREFIX)/lib/libz.a
PNG_LIB        := $(DEPS_PREFIX)/lib/libpng16.a
LINKS_EXTRACTED := $(LINKS_SRC)/.agenticos-extracted
LINKS_BUILT    := $(LINKS_SRC)/links
BIN            := $(OUT_DIR)/links
PATCHES        := $(sort $(wildcard patches/*.patch))
NPROC          := $(shell sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)
RUST_MANIFEST  := driver-rs/Cargo.toml
RUST_SOURCES   := $(shell find driver-rs/src -type f) $(RUST_MANIFEST)
RUST_TARGET    := $(abspath $(OUT_DIR)/rust-target)
RUST_ARCHIVE   := $(RUST_TARGET)/x86_64-unknown-linux-musl/release/liblinks_agenticos_driver.a

LINKS_CFLAGS   := -O2 -fno-pie -DAGENTICOS_NO_PTHREADS -DGRDRV_AGENTICOS
LINKS_CPPFLAGS := -I$(DEPS_PREFIX)/include
LINKS_LDFLAGS  := -static -no-pie -L$(DEPS_PREFIX)/lib

.PHONY: all clean distclean fetch
.DELETE_ON_ERROR:

all: $(BIN)

fetch: $(LINKS_TARBALL) $(ZLIB_TARBALL) $(PNG_TARBALL)

$(TARBALLS_DIR):
	mkdir -p $@

$(LINKS_TARBALL): | $(TARBALLS_DIR)
	@echo "📥 Fetching links-$(LINKS_VERSION)..."
	curl -fsSL --retry 3 --retry-all-errors --continue-at - --max-time 300 -o $@.tmp $(LINKS_URL)
	@actual=$$(shasum -a 256 $@.tmp | awk '{print $$1}'); \
	  if [ "$$actual" != "$(LINKS_SHA256)" ]; then \
	    echo "❌ links SHA256 mismatch:"; echo "   expected $(LINKS_SHA256)"; echo "   got      $$actual"; \
	    rm -f $@.tmp; exit 1; \
	  fi
	mv $@.tmp $@

$(ZLIB_TARBALL): | $(TARBALLS_DIR)
	@echo "📥 Fetching zlib-$(ZLIB_VERSION)..."
	curl -fsSL --retry 3 --retry-all-errors --max-time 300 -o $@.tmp $(ZLIB_URL)
	@actual=$$(shasum -a 256 $@.tmp | awk '{print $$1}'); \
	  test "$$actual" = "$(ZLIB_SHA256)" || { echo "❌ zlib SHA256 mismatch: $$actual"; rm -f $@.tmp; exit 1; }
	mv $@.tmp $@

$(PNG_TARBALL): | $(TARBALLS_DIR)
	@echo "📥 Fetching libpng-$(PNG_VERSION)..."
	curl -fsSL --retry 3 --retry-all-errors --max-time 300 -o $@.tmp $(PNG_URL)
	@actual=$$(shasum -a 256 $@.tmp | awk '{print $$1}'); \
	  test "$$actual" = "$(PNG_SHA256)" || { echo "❌ libpng SHA256 mismatch: $$actual"; rm -f $@.tmp; exit 1; }
	mv $@.tmp $@

$(ZLIB_LIB): $(ZLIB_TARBALL)
	rm -rf $(ZLIB_SRC)
	tar xzf $< -C $(OUT_DIR)
	cd $(ZLIB_SRC) && CHOST=x86_64-linux-musl CC=$(MUSL_CC) AR=$(MUSL_AR) RANLIB=$(MUSL_RANLIB) \
	  CFLAGS="-O2 -fno-pie" LDFLAGS="-static -no-pie" ./configure --static --prefix=$(DEPS_PREFIX) >/dev/null
	$(MAKE) -C $(ZLIB_SRC) -j$(NPROC) >/dev/null
	$(MAKE) -C $(ZLIB_SRC) install >/dev/null

$(PNG_LIB): $(PNG_TARBALL) $(ZLIB_LIB)
	rm -rf $(PNG_SRC)
	tar xJf $< -C $(OUT_DIR)
	cd $(PNG_SRC) && CC=$(MUSL_CC) AR=$(MUSL_AR) RANLIB=$(MUSL_RANLIB) \
	  CPPFLAGS="-I$(DEPS_PREFIX)/include" LDFLAGS="-static -no-pie -L$(DEPS_PREFIX)/lib" \
	  ./configure --host=x86_64-linux-musl --prefix=$(DEPS_PREFIX) --disable-shared --enable-static \
	    --disable-tools --disable-tests >/dev/null
	$(MAKE) -C $(PNG_SRC) -j$(NPROC) >/dev/null
	$(MAKE) -C $(PNG_SRC) install >/dev/null

$(RUST_ARCHIVE): $(RUST_SOURCES)
	@echo "🦀 Building no_std AgenticOS graphics driver..."
	CARGO_TARGET_DIR=$(RUST_TARGET) cargo build --manifest-path $(RUST_MANIFEST) \
	  --target x86_64-unknown-linux-musl -Z build-std=core,compiler_builtins --release

$(LINKS_EXTRACTED): $(LINKS_TARBALL) $(PATCHES) driver/agenticos.c
	rm -rf $(LINKS_SRC)
	tar xjf $< -C $(OUT_DIR)
	@for patch_file in $(PATCHES); do \
	  echo "🩹 Applying $$patch_file"; \
	  patch -d $(LINKS_SRC) -p1 < "$$patch_file"; \
	done
	cp driver/agenticos.c $(LINKS_SRC)/agenticos.c
	@# The release tarball ships generated files; keep make from trying to run
	@# unavailable autotools merely because applying a patch refreshed mtimes.
	touch $(LINKS_SRC)/configure $(LINKS_SRC)/Makefile.in
	@touch $@

$(LINKS_BUILT): Makefile $(LINKS_EXTRACTED) $(ZLIB_LIB) $(PNG_LIB) $(RUST_ARCHIVE)
	@echo "🔨 Configuring links-$(LINKS_VERSION) (text + AgenticOS GUI)..."
	cd $(LINKS_SRC) && \
	  CC=$(MUSL_CC) AR=$(MUSL_AR) RANLIB=$(MUSL_RANLIB) \
	  CFLAGS="$(LINKS_CFLAGS)" CPPFLAGS="$(LINKS_CPPFLAGS)" LDFLAGS="$(LINKS_LDFLAGS)" \
	  LIBS="$(RUST_ARCHIVE)" \
	  ./configure --host=x86_64-linux-musl \
	    --enable-graphics --without-ipv6 --without-libevent --without-gpm \
	    --without-x --without-svgalib --without-fb --without-directfb \
	    --without-pmshell --without-windows --without-atheos --without-haiku --without-grx \
	    --disable-png-pkgconfig --without-openmp --without-ssl --without-brotli --without-zstd \
	    --without-bzip2 --without-lzma >/dev/null
	@touch $(LINKS_SRC)/aclocal.m4 $(LINKS_SRC)/configure $(LINKS_SRC)/config.h.in \
	  $(LINKS_SRC)/stamp-h.in $(LINKS_SRC)/Makefile.in
	@# The local patch must select the fork helper even though musl exposes
	@# pthreads. Keep this assertion next to the build so an upstream drift
	@# cannot silently reintroduce clone/futex requirements.
	@grep -q 'defined(HAVE_PTHREADS) && !defined(AGENTICOS_NO_PTHREADS)' $(LINKS_SRC)/os_dep.c
	@grep -q 'GRDRV_AGENTICOS' $(LINKS_SRC)/agenticos.c
	@echo "🔨 Building links-$(LINKS_VERSION)..."
	rm -f $(LINKS_BUILT)
	$(MAKE) -C $(LINKS_SRC) -j$(NPROC) \
	  CC=$(MUSL_CC) CFLAGS="$(LINKS_CFLAGS)" CPPFLAGS="$(LINKS_CPPFLAGS)" \
	  LDFLAGS="$(LINKS_LDFLAGS)" \
	  LIBS="$(RUST_ARCHIVE) -lpng -lz -lm -lpthread" >/dev/null

$(BIN): $(LINKS_BUILT)
	cp $< $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@
	@size=$$(wc -c < $@ | tr -d ' '); echo "✅ Built $@ ($$size bytes, stripped)"

clean:
	rm -rf $(LINKS_SRC) $(ZLIB_SRC) $(PNG_SRC) $(BIN)

distclean:
	rm -rf $(OUT_DIR)
