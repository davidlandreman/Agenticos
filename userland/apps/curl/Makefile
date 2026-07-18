# curl — command-line HTTP/HTTPS transfer tool.
#
# Cross-builds a static-musl curl with private, pinned dependencies. The
# OpenSSL and zlib recipes are deliberately identical to the qualified
# profile in ../links2/Makefile (static, single-threaded TLS,
# OPENSSLDIR=/etc/ssl). Scope is HTTP + HTTPS over IPv4 only; every other
# curl protocol is disabled at configure time. The compiled-in CA bundle is
# the kernel-managed /etc/ssl/cert.pem trust snapshot, so HTTPS fails
# closed when boot withholds the trust store.

MUSL_CC      ?= x86_64-linux-musl-gcc
MUSL_AR      ?= $(patsubst %-gcc,%-ar,$(MUSL_CC))
MUSL_RANLIB  ?= $(patsubst %-gcc,%-ranlib,$(MUSL_CC))
MUSL_STRIP   ?= $(patsubst %-gcc,%-strip,$(MUSL_CC))
MUSL_READELF ?= $(patsubst %-gcc,%-readelf,$(MUSL_CC))

CURL_VERSION := 8.21.0
CURL_SHA256  := d9b327997999045a24cda50f3983e69e51c516bd8be6ef9842fc7f99135e33bb
CURL_URL     := https://curl.se/download/curl-$(CURL_VERSION).tar.gz
ZLIB_VERSION  := 1.3.2
ZLIB_SHA256   := bb329a0a2cd0274d05519d61c667c062e06990d72e125ee2dfa8de64f0119d16
ZLIB_URL      := https://zlib.net/fossils/zlib-$(ZLIB_VERSION).tar.gz
OPENSSL_VERSION := 3.5.7
OPENSSL_SHA256  := a8c0d28a529ca480f9f36cf5792e2cd21984552a3c8e4aa11a24aa31aeac98e8
OPENSSL_URL     := https://github.com/openssl/openssl/releases/download/openssl-$(OPENSSL_VERSION)/openssl-$(OPENSSL_VERSION).tar.gz

OUT_DIR        := build
TARBALLS_DIR   := $(OUT_DIR)/tarballs
CURL_TARBALL   := $(TARBALLS_DIR)/curl-$(CURL_VERSION).tar.gz
ZLIB_TARBALL   := $(TARBALLS_DIR)/zlib-$(ZLIB_VERSION).tar.gz
OPENSSL_TARBALL := $(TARBALLS_DIR)/openssl-$(OPENSSL_VERSION).tar.gz
CURL_SRC       := $(OUT_DIR)/curl-$(CURL_VERSION)
ZLIB_SRC       := $(OUT_DIR)/zlib-$(ZLIB_VERSION)
OPENSSL_SRC    := $(OUT_DIR)/openssl-$(OPENSSL_VERSION)
DEPS_PREFIX    := $(abspath $(OUT_DIR)/deps)
ZLIB_LIB       := $(DEPS_PREFIX)/lib/libz.a
OPENSSL_STAMP  := $(DEPS_PREFIX)/.openssl-$(OPENSSL_VERSION)-installed
OPENSSL_SSL_LIB := $(DEPS_PREFIX)/lib/libssl.a
OPENSSL_CRYPTO_LIB := $(DEPS_PREFIX)/lib/libcrypto.a
CURL_BUILT     := $(CURL_SRC)/src/curl
CURL_CONFIG_H  := $(CURL_SRC)/lib/curl_config.h
BIN            := $(OUT_DIR)/curl
NPROC          := $(shell sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)

CURL_CFLAGS   := -O2 -fno-pie
CURL_CPPFLAGS := -I$(DEPS_PREFIX)/include
CURL_LDFLAGS  := -static -no-pie -L$(DEPS_PREFIX)/lib

.PHONY: all clean distclean fetch
.DELETE_ON_ERROR:

all: $(BIN)

fetch: $(CURL_TARBALL) $(ZLIB_TARBALL) $(OPENSSL_TARBALL)

$(TARBALLS_DIR):
	mkdir -p $@

$(CURL_TARBALL): | $(TARBALLS_DIR)
	@echo "📥 Fetching curl-$(CURL_VERSION)..."
	curl -fsSL --retry 3 --retry-all-errors --max-time 300 -o $@.tmp $(CURL_URL)
	@actual=$$(shasum -a 256 $@.tmp | awk '{print $$1}'); \
	  if [ "$$actual" != "$(CURL_SHA256)" ]; then \
	    echo "❌ curl SHA256 mismatch:"; echo "   expected $(CURL_SHA256)"; echo "   got      $$actual"; \
	    rm -f $@.tmp; exit 1; \
	  fi
	mv $@.tmp $@

$(ZLIB_TARBALL): | $(TARBALLS_DIR)
	@echo "📥 Fetching zlib-$(ZLIB_VERSION)..."
	curl -fsSL --retry 3 --retry-all-errors --max-time 300 -o $@.tmp $(ZLIB_URL)
	@actual=$$(shasum -a 256 $@.tmp | awk '{print $$1}'); \
	  test "$$actual" = "$(ZLIB_SHA256)" || { echo "❌ zlib SHA256 mismatch: $$actual"; rm -f $@.tmp; exit 1; }
	mv $@.tmp $@

$(OPENSSL_TARBALL): | $(TARBALLS_DIR)
	@echo "📥 Fetching openssl-$(OPENSSL_VERSION)..."
	curl -fsSL --retry 3 --retry-all-errors --max-time 300 -o $@.tmp $(OPENSSL_URL)
	@actual=$$(shasum -a 256 $@.tmp | awk '{print $$1}'); \
	  test "$$actual" = "$(OPENSSL_SHA256)" || { echo "❌ OpenSSL SHA256 mismatch: $$actual"; rm -f $@.tmp; exit 1; }
	mv $@.tmp $@

$(ZLIB_LIB): $(ZLIB_TARBALL)
	rm -rf $(ZLIB_SRC)
	tar xzf $< -C $(OUT_DIR)
	cd $(ZLIB_SRC) && CHOST=x86_64-linux-musl CC=$(MUSL_CC) AR=$(MUSL_AR) RANLIB=$(MUSL_RANLIB) \
	  CFLAGS="-O2 -fno-pie" LDFLAGS="-static -no-pie" ./configure --static --prefix=$(DEPS_PREFIX) >/dev/null
	$(MAKE) -C $(ZLIB_SRC) -j$(NPROC) >/dev/null
	$(MAKE) -C $(ZLIB_SRC) install >/dev/null

$(OPENSSL_STAMP): $(OPENSSL_TARBALL) Makefile
	rm -rf $(OPENSSL_SRC)
	tar xzf $< -C $(OUT_DIR)
	@echo "🔐 Configuring OpenSSL $(OPENSSL_VERSION) (static, single-threaded TLS)..."
	cd $(OPENSSL_SRC) && \
	  CC=$(MUSL_CC) AR=$(MUSL_AR) RANLIB=$(MUSL_RANLIB) \
	  perl Configure linux-x86_64 --prefix=$(DEPS_PREFIX) --libdir=lib --openssldir=/etc/ssl \
	    -O2 -fno-pie no-shared no-module no-dso no-pinshared no-pic \
	    no-threads no-async no-secure-memory no-apps no-tests no-docs \
	    no-legacy no-fips no-engine no-quic no-zlib no-zlib-dynamic \
	    no-autoload-config >/dev/null
	$(MAKE) -C $(OPENSSL_SRC) -j$(NPROC) build_sw >/dev/null
	$(MAKE) -C $(OPENSSL_SRC) install_dev >/dev/null
	@test -f $(OPENSSL_SSL_LIB) && test -f $(OPENSSL_CRYPTO_LIB)
	@grep -q 'OPENSSL_NO_AUTOLOAD_CONFIG' $(DEPS_PREFIX)/include/openssl/configuration.h
	@grep -q 'OPENSSL_NO_DSO' $(DEPS_PREFIX)/include/openssl/configuration.h
	@grep -q 'OPENSSL_NO_QUIC' $(DEPS_PREFIX)/include/openssl/configuration.h
	@! grep -q 'OPENSSL_THREADS' $(DEPS_PREFIX)/include/openssl/configuration.h
	@touch $@

$(CURL_BUILT): Makefile $(CURL_TARBALL) $(ZLIB_LIB) $(OPENSSL_STAMP)
	rm -rf $(CURL_SRC)
	tar xzf $(CURL_TARBALL) -C $(OUT_DIR)
	@echo "🔨 Configuring curl-$(CURL_VERSION) (static, HTTP/HTTPS, IPv4)..."
	cd $(CURL_SRC) && \
	  CC=$(MUSL_CC) AR=$(MUSL_AR) RANLIB=$(MUSL_RANLIB) \
	  CFLAGS="$(CURL_CFLAGS)" CPPFLAGS="$(CURL_CPPFLAGS)" LDFLAGS="$(CURL_LDFLAGS)" \
	  ./configure --host=x86_64-linux-musl \
	    --disable-shared --enable-static \
	    --with-openssl=$(DEPS_PREFIX) --with-zlib=$(DEPS_PREFIX) \
	    --with-ca-bundle=/etc/ssl/cert.pem --without-ca-path \
	    --disable-ipv6 --disable-threaded-resolver --disable-unix-sockets \
	    --disable-socketpair \
	    --disable-ftp --disable-file --disable-ldap --disable-ldaps \
	    --disable-rtsp --disable-dict --disable-telnet --disable-tftp \
	    --disable-pop3 --disable-imap --disable-smtp --disable-gopher \
	    --disable-mqtt --disable-smb --disable-websockets \
	    --disable-ntlm --disable-tls-srp --disable-aws \
	    --disable-manual --disable-docs --disable-libcurl-option \
	    --without-libpsl --without-libidn2 --without-nghttp2 \
	    --without-brotli --without-zstd >/dev/null
	@# The configure profile is load-bearing: assert the security-relevant
	@# outcomes so a curl or toolchain bump cannot silently change them.
	@grep -q '^#define USE_OPENSSL 1' $(CURL_CONFIG_H)
	@grep -q '^#define CURL_CA_BUNDLE "/etc/ssl/cert.pem"' $(CURL_CONFIG_H)
	@grep -q '^#define HAVE_LIBZ 1' $(CURL_CONFIG_H)
	@! grep -qE '^#define (USE_IPV6|ENABLE_IPV6) 1' $(CURL_CONFIG_H)
	@! grep -q '^#define USE_THREADS_POSIX 1' $(CURL_CONFIG_H)
	@grep -q '^#define CURL_DISABLE_FTP 1' $(CURL_CONFIG_H)
	@grep -q '^#define CURL_DISABLE_FILE 1' $(CURL_CONFIG_H)
	@# The multi-API wakeup descriptor needs eventfd2 or an AF_UNIX
	@# socketpair; AgenticOS implements neither. Disabling socketpair
	@# removes ENABLE_WAKEUP entirely (multihandle.h) — single-threaded
	@# curl never needs cross-thread wakeups.
	@grep -q '^#define CURL_DISABLE_SOCKETPAIR 1' $(CURL_CONFIG_H)
	@echo "🔨 Building curl-$(CURL_VERSION)..."
	@# CURL_LDFLAGS_BIN feeds libtool's link of the curl tool itself;
	@# -all-static is the libtool spelling that forbids every dynamic
	@# dependency (a plain -static in LDFLAGS is swallowed by libtool).
	$(MAKE) -C $(CURL_SRC) -j$(NPROC) CURL_LDFLAGS_BIN=-all-static >/dev/null

$(BIN): $(CURL_BUILT)
	cp $< $@.tmp
	@# libtool silently drops a plain -static from LDFLAGS for the tool
	@# link; CURL_LDFLAGS_BIN=-all-static above is what actually forces a
	@# fully static executable. Refuse to ship anything that still expects
	@# a dynamic loader — AgenticOS has none.
	@! $(MUSL_READELF) -l $@.tmp | grep -q 'INTERP' || { echo "❌ $@.tmp has PT_INTERP; not fully static"; rm -f $@.tmp; exit 1; }
	@! $(MUSL_READELF) -d $@.tmp 2>/dev/null | grep -q 'NEEDED' || { echo "❌ $@.tmp has DT_NEEDED; not fully static"; rm -f $@.tmp; exit 1; }
	$(MUSL_STRIP) --strip-all $@.tmp
	@size=$$(wc -c < $@.tmp | tr -d ' '); \
	  test "$$size" -lt 16777216 || { echo "❌ $@.tmp is $$size bytes; 16 MiB loader gate exceeded"; rm -f $@.tmp; exit 1; }
	mv -f $@.tmp $@
	@size=$$(wc -c < $@ | tr -d ' '); echo "✅ Built $@ ($$size bytes, stripped)"

clean:
	rm -rf $(CURL_SRC) $(ZLIB_SRC) $(OPENSSL_SRC) $(BIN)

distclean:
	rm -rf $(OUT_DIR)
