# git — distributed version control.
#
# Cross-builds static-musl git with private, pinned dependencies. The
# zlib, OpenSSL, and libcurl recipes are deliberately identical to the
# qualified profile in ../curl/Makefile (static, single-threaded TLS,
# OPENSSLDIR=/etc/ssl, CA bundle /etc/ssl/cert.pem, IPv4 HTTP/HTTPS
# only). Two binaries ship:
#
#   build/git              → GIT.ELF        (/bin/git, all builtins)
#   build/git-remote-http  → GITRHTTP.ELF   (/bin/git-remote-http{,s})
#
# The HTTP(S) transport helper carries libcurl + OpenSSL; the main git
# binary does not link curl at all. Compiled-in paths target AgenticOS:
# gitexecdir=/bin (so git spawns /bin/git-remote-https through the
# virtual bin namespace), sysconfdir=/etc (system config is the
# kernel-managed /etc/gitconfig). Perl/python/tcl porcelain, gettext,
# expat (legacy dumb-HTTP WebDAV push), iconv, pthreads (pack/delta
# parallelism only; pthread groups pin to one CPU today), and unix
# sockets (credential-cache) are all compiled out.

MUSL_CC      ?= x86_64-linux-musl-gcc
MUSL_AR      ?= $(patsubst %-gcc,%-ar,$(MUSL_CC))
MUSL_RANLIB  ?= $(patsubst %-gcc,%-ranlib,$(MUSL_CC))
MUSL_STRIP   ?= $(patsubst %-gcc,%-strip,$(MUSL_CC))
MUSL_READELF ?= $(patsubst %-gcc,%-readelf,$(MUSL_CC))

GIT_VERSION := 2.52.0
GIT_SHA256  := 6880cb1e737e26f81cf7db9957ab2b5bb2aa1490d87619480b860816e0c10c32
GIT_URL     := https://mirrors.edge.kernel.org/pub/software/scm/git/git-$(GIT_VERSION).tar.gz
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
GIT_TARBALL    := $(TARBALLS_DIR)/git-$(GIT_VERSION).tar.gz
CURL_TARBALL   := $(TARBALLS_DIR)/curl-$(CURL_VERSION).tar.gz
ZLIB_TARBALL   := $(TARBALLS_DIR)/zlib-$(ZLIB_VERSION).tar.gz
OPENSSL_TARBALL := $(TARBALLS_DIR)/openssl-$(OPENSSL_VERSION).tar.gz
GIT_SRC        := $(OUT_DIR)/git-$(GIT_VERSION)
CURL_SRC       := $(OUT_DIR)/curl-$(CURL_VERSION)
ZLIB_SRC       := $(OUT_DIR)/zlib-$(ZLIB_VERSION)
OPENSSL_SRC    := $(OUT_DIR)/openssl-$(OPENSSL_VERSION)
DEPS_PREFIX    := $(abspath $(OUT_DIR)/deps)
ZLIB_LIB       := $(DEPS_PREFIX)/lib/libz.a
OPENSSL_STAMP  := $(DEPS_PREFIX)/.openssl-$(OPENSSL_VERSION)-installed
OPENSSL_SSL_LIB := $(DEPS_PREFIX)/lib/libssl.a
OPENSSL_CRYPTO_LIB := $(DEPS_PREFIX)/lib/libcrypto.a
LIBCURL_LIB    := $(DEPS_PREFIX)/lib/libcurl.a
CURL_CONFIG_H  := $(CURL_SRC)/lib/curl_config.h
GIT_BUILT      := $(GIT_SRC)/git
GIT_HELPER_BUILT := $(GIT_SRC)/git-remote-http
BIN            := $(OUT_DIR)/git
HELPER_BIN     := $(OUT_DIR)/git-remote-http
NPROC          := $(shell sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)

GIT_CFLAGS  := -O2 -fno-pie
GIT_LDFLAGS := -static -no-pie -L$(DEPS_PREFIX)/lib

# The variable block handed to git's own Makefile. uname_* overrides are
# load-bearing: the build host is macOS, so without them git would pick
# the Darwin platform block instead of Linux. Every NO_* is a deliberate
# scope cut documented in the header comment. CURLDIR + CURL_LDFLAGS
# bypass curl-config entirely; CC_LD_DYNPATH=-L neutralizes the rpath
# flag git would otherwise emit (meaningless for a static link).
GIT_MAKE_VARS := \
	CC=$(MUSL_CC) AR=$(MUSL_AR) SPATCH=false \
	uname_S=Linux uname_M=x86_64 uname_R=6.1.0 uname_O=GNU/Linux \
	CFLAGS="$(GIT_CFLAGS)" LDFLAGS="$(GIT_LDFLAGS)" \
	prefix=/ gitexecdir=/bin sysconfdir=/etc \
	template_dir=/etc/git-templates \
	DEFAULT_PAGER=less DEFAULT_EDITOR=vi \
	NO_GETTEXT=YesPlease NO_TCLTK=YesPlease NO_PERL=YesPlease \
	NO_PYTHON=YesPlease NO_EXPAT=YesPlease NO_ICONV=YesPlease \
	NO_OPENSSL=YesPlease NO_PTHREADS=YesPlease \
	NO_UNIX_SOCKETS=YesPlease NO_IPV6=YesPlease \
	NO_REGEX=NeedsStartEnd \
	NO_INSTALL_HARDLINKS=YesPlease SKIP_DASHED_BUILT_INS=YesPlease \
	CURLDIR=$(DEPS_PREFIX) CC_LD_DYNPATH=-L \
	CURL_LDFLAGS="-L$(DEPS_PREFIX)/lib -lcurl -lssl -lcrypto -lz"

.PHONY: all clean distclean fetch
.DELETE_ON_ERROR:

all: $(BIN) $(HELPER_BIN)

fetch: $(GIT_TARBALL) $(CURL_TARBALL) $(ZLIB_TARBALL) $(OPENSSL_TARBALL)

$(TARBALLS_DIR):
	mkdir -p $@

$(GIT_TARBALL): | $(TARBALLS_DIR)
	@echo "📥 Fetching git-$(GIT_VERSION)..."
	curl -fsSL --retry 3 --retry-all-errors --max-time 600 -o $@.tmp $(GIT_URL)
	@actual=$$(shasum -a 256 $@.tmp | awk '{print $$1}'); \
	  if [ "$$actual" != "$(GIT_SHA256)" ]; then \
	    echo "❌ git SHA256 mismatch:"; echo "   expected $(GIT_SHA256)"; echo "   got      $$actual"; \
	    rm -f $@.tmp; exit 1; \
	  fi
	mv $@.tmp $@

$(CURL_TARBALL): | $(TARBALLS_DIR)
	@echo "📥 Fetching curl-$(CURL_VERSION)..."
	curl -fsSL --retry 3 --retry-all-errors --max-time 300 -o $@.tmp $(CURL_URL)
	@actual=$$(shasum -a 256 $@.tmp | awk '{print $$1}'); \
	  test "$$actual" = "$(CURL_SHA256)" || { echo "❌ curl SHA256 mismatch: $$actual"; rm -f $@.tmp; exit 1; }
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

# libcurl as a static library for git-remote-http. Same configure
# profile as ../curl/Makefile; only the install step differs (git needs
# the dev files, not the curl tool).
$(LIBCURL_LIB): Makefile $(CURL_TARBALL) $(ZLIB_LIB) $(OPENSSL_STAMP)
	rm -rf $(CURL_SRC)
	tar xzf $(CURL_TARBALL) -C $(OUT_DIR)
	@echo "🔨 Configuring libcurl-$(CURL_VERSION) (static, HTTP/HTTPS, IPv4)..."
	cd $(CURL_SRC) && \
	  CC=$(MUSL_CC) AR=$(MUSL_AR) RANLIB=$(MUSL_RANLIB) \
	  CFLAGS="-O2 -fno-pie" CPPFLAGS="-I$(DEPS_PREFIX)/include" LDFLAGS="-static -no-pie -L$(DEPS_PREFIX)/lib" \
	  ./configure --host=x86_64-linux-musl --prefix=$(DEPS_PREFIX) \
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
	@grep -q '^#define USE_OPENSSL 1' $(CURL_CONFIG_H)
	@grep -q '^#define CURL_CA_BUNDLE "/etc/ssl/cert.pem"' $(CURL_CONFIG_H)
	@grep -q '^#define HAVE_LIBZ 1' $(CURL_CONFIG_H)
	@! grep -qE '^#define (USE_IPV6|ENABLE_IPV6) 1' $(CURL_CONFIG_H)
	@! grep -q '^#define USE_THREADS_POSIX 1' $(CURL_CONFIG_H)
	@grep -q '^#define CURL_DISABLE_FTP 1' $(CURL_CONFIG_H)
	@grep -q '^#define CURL_DISABLE_FILE 1' $(CURL_CONFIG_H)
	@grep -q '^#define CURL_DISABLE_SOCKETPAIR 1' $(CURL_CONFIG_H)
	@echo "🔨 Building libcurl-$(CURL_VERSION)..."
	$(MAKE) -C $(CURL_SRC)/lib -j$(NPROC) install >/dev/null
	$(MAKE) -C $(CURL_SRC)/include install >/dev/null
	@test -f $(LIBCURL_LIB)

$(GIT_BUILT): Makefile $(GIT_TARBALL) $(ZLIB_LIB) $(LIBCURL_LIB)
	rm -rf $(GIT_SRC)
	tar xzf $(GIT_TARBALL) -C $(OUT_DIR)
	@echo "🔨 Building git-$(GIT_VERSION) (static, musl)..."
	$(MAKE) -C $(GIT_SRC) -j$(NPROC) $(GIT_MAKE_VARS) git git-remote-http
	@test -f $(GIT_HELPER_BUILT)

$(GIT_HELPER_BUILT): $(GIT_BUILT)

# Shared finalize gates, mirroring ../curl/Makefile: refuse anything
# that still expects a dynamic loader, strip, and hold the 16 MiB
# userland loader gate.
define FINALIZE_ELF
	cp $< $@.tmp
	@! $(MUSL_READELF) -l $@.tmp | grep -q 'INTERP' || { echo "❌ $@.tmp has PT_INTERP; not fully static"; rm -f $@.tmp; exit 1; }
	@! $(MUSL_READELF) -d $@.tmp 2>/dev/null | grep -q 'NEEDED' || { echo "❌ $@.tmp has DT_NEEDED; not fully static"; rm -f $@.tmp; exit 1; }
	$(MUSL_STRIP) --strip-all $@.tmp
	@size=$$(wc -c < $@.tmp | tr -d ' '); \
	  test "$$size" -lt 16777216 || { echo "❌ $@.tmp is $$size bytes; 16 MiB loader gate exceeded"; rm -f $@.tmp; exit 1; }
	mv -f $@.tmp $@
	@size=$$(wc -c < $@ | tr -d ' '); echo "✅ Built $@ ($$size bytes, stripped)"
endef

$(BIN): $(GIT_BUILT)
	$(FINALIZE_ELF)

$(HELPER_BIN): $(GIT_HELPER_BUILT)
	$(FINALIZE_ELF)

clean:
	rm -rf $(GIT_SRC) $(CURL_SRC) $(ZLIB_SRC) $(OPENSSL_SRC) $(BIN) $(HELPER_BIN)

distclean:
	rm -rf $(OUT_DIR)
