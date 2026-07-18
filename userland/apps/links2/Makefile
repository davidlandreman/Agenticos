# Links 2 — text-mode HTTP browser for AgenticOS.
#
# Cross-builds a deliberately small static-musl binary. Graphics, TLS, IPv6,
# libevent, GPM, and optional compression libraries are deferred so the first
# port exercises only the existing terminal and IPv4 socket platform.

MUSL_CC      ?= x86_64-linux-musl-gcc
MUSL_AR      ?= $(patsubst %-gcc,%-ar,$(MUSL_CC))
MUSL_RANLIB  ?= $(patsubst %-gcc,%-ranlib,$(MUSL_CC))
MUSL_STRIP   ?= $(patsubst %-gcc,%-strip,$(MUSL_CC))

LINKS_VERSION := 2.30
LINKS_SHA256  := c4631c6b5a11527cdc3cb7872fc23b7f2b25c2b021d596be410dadb40315f166
LINKS_URL     := https://links.twibright.com/download/links-$(LINKS_VERSION).tar.bz2

OUT_DIR        := build
TARBALLS_DIR   := $(OUT_DIR)/tarballs
LINKS_TARBALL  := $(TARBALLS_DIR)/links-$(LINKS_VERSION).tar.bz2
LINKS_SRC      := $(OUT_DIR)/links-$(LINKS_VERSION)
LINKS_EXTRACTED := $(LINKS_SRC)/.agenticos-extracted
LINKS_BUILT    := $(LINKS_SRC)/links
BIN            := $(OUT_DIR)/links
PATCHES        := $(sort $(wildcard patches/*.patch))
NPROC          := $(shell sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)

LINKS_CFLAGS  := -O2 -fno-pie -DAGENTICOS_NO_PTHREADS
LINKS_LDFLAGS := -static -no-pie

.PHONY: all clean distclean fetch
.DELETE_ON_ERROR:

all: $(BIN)

fetch: $(LINKS_TARBALL)

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

$(LINKS_EXTRACTED): $(LINKS_TARBALL) $(PATCHES)
	rm -rf $(LINKS_SRC)
	tar xjf $< -C $(OUT_DIR)
	@for patch_file in $(PATCHES); do \
	  echo "🩹 Applying $$patch_file"; \
	  patch -d $(LINKS_SRC) -p1 < "$$patch_file"; \
	done
	@touch $@

$(LINKS_BUILT): $(LINKS_EXTRACTED)
	@echo "🔨 Configuring links-$(LINKS_VERSION) (text/HTTP/IPv4 only)..."
	cd $(LINKS_SRC) && \
	  CC=$(MUSL_CC) AR=$(MUSL_AR) RANLIB=$(MUSL_RANLIB) \
	  CFLAGS="$(LINKS_CFLAGS)" LDFLAGS="$(LINKS_LDFLAGS)" \
	  ./configure --host=x86_64-linux-musl \
	    --disable-graphics --without-ipv6 --without-libevent --without-gpm \
	    --without-ssl --without-zlib --without-brotli --without-zstd \
	    --without-bzip2 --without-lzma >/dev/null
	@# The local patch must select the fork helper even though musl exposes
	@# pthreads. Keep this assertion next to the build so an upstream drift
	@# cannot silently reintroduce clone/futex requirements.
	@grep -q 'defined(HAVE_PTHREADS) && !defined(AGENTICOS_NO_PTHREADS)' $(LINKS_SRC)/os_dep.c
	@echo "🔨 Building links-$(LINKS_VERSION)..."
	$(MAKE) -C $(LINKS_SRC) -j$(NPROC) >/dev/null

$(BIN): $(LINKS_BUILT)
	cp $< $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@
	@size=$$(wc -c < $@ | tr -d ' '); echo "✅ Built $@ ($$size bytes, stripped)"

clean:
	rm -rf $(LINKS_SRC) $(BIN)

distclean:
	rm -rf $(OUT_DIR)
