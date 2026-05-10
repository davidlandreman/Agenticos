# ZSH — first real userland shell.
#
# Builds zsh statically against musl + ncurses-widec via the host's
# x86_64-linux-musl cross-compiler. The kernel ELF loader accepts only
# ET_EXEC (-static -no-pie); the readelf check in build.sh asserts that.
#
# zsh has no "static" build target of its own, so the recipe is:
#   1. fetch + verify ncurses-6.5 tarball, extract, configure, build, install
#      to build/ncurses-install/ (libncursesw.a + headers + terminfo).
#   2. fetch + verify zsh-5.9 tarball, extract, configure with CPPFLAGS
#      pointing at the installed ncurses, LDFLAGS=-static -no-pie, build.
#   3. strip the resulting binary and copy to build/zsh.
#
# Versions and SHA256s are pinned in this file so the recipe is
# reproducible across hosts. Bumping a version bumps the SHA in lockstep.
#
# Override the toolchain by setting MUSL_CC:
#   make MUSL_CC=x86_64-linux-musl-gcc-13

MUSL_CC      ?= x86_64-linux-musl-gcc
MUSL_AR      ?= $(patsubst %-gcc,%-ar,$(MUSL_CC))
MUSL_RANLIB  ?= $(patsubst %-gcc,%-ranlib,$(MUSL_CC))
MUSL_STRIP   ?= $(patsubst %-gcc,%-strip,$(MUSL_CC))

ZSH_VERSION     := 5.9
ZSH_SHA256      := 9b8d1ecedd5b5e81fbf1918e876752a7dd948e05c1a0dba10ab863842d45acd5
ZSH_URL         := https://www.zsh.org/pub/zsh-$(ZSH_VERSION).tar.xz

NCURSES_VERSION := 6.5
NCURSES_SHA256  := 136d91bc269a9a5785e5f9e980bc76ab57428f604ce3e5a5a90cebc767971cc6
NCURSES_URL     := https://ftp.gnu.org/pub/gnu/ncurses/ncurses-$(NCURSES_VERSION).tar.gz

OUT_DIR         := build
TARBALLS_DIR    := $(OUT_DIR)/tarballs
NCURSES_SRC     := $(OUT_DIR)/ncurses-$(NCURSES_VERSION)
NCURSES_PREFIX  := $(abspath $(OUT_DIR)/ncurses-install)
NCURSES_LIB     := $(NCURSES_PREFIX)/lib/libncursesw.a
ZSH_SRC         := $(OUT_DIR)/zsh-$(ZSH_VERSION)
ZSH_BUILT       := $(ZSH_SRC)/Src/zsh
BIN             := $(OUT_DIR)/zsh

# Compile flags. GCC 15 promoted three latent autoconf-test mismatches
# from warnings to errors:
#   - incompatible-pointer-types: zsh's ncurses-symbol detection probes
#     `char **test = boolcodes`, but ncurses 6.x exports
#     `const char *const boolcodes[]`. Without this flag the detection
#     fails, zsh defines its own boolcodes locally, and the link conflicts.
#   - implicit-function-declaration / int-conversion: similar autoconf
#     compatibility under stricter modern GCC defaults.
ZSH_CFLAGS  := -O2 -fno-pie \
               -Wno-error=incompatible-pointer-types \
               -Wno-error=implicit-function-declaration \
               -Wno-error=int-conversion
ZSH_LDFLAGS := -static -no-pie

.PHONY: all clean distclean fetch
.DELETE_ON_ERROR:

all: $(BIN)

# ---------------- fetch + extract ----------------

fetch: $(TARBALLS_DIR)/ncurses-$(NCURSES_VERSION).tar.gz \
       $(TARBALLS_DIR)/zsh-$(ZSH_VERSION).tar.xz

$(TARBALLS_DIR):
	mkdir -p $@

$(TARBALLS_DIR)/ncurses-$(NCURSES_VERSION).tar.gz: | $(TARBALLS_DIR)
	@echo "📥 Fetching ncurses-$(NCURSES_VERSION)..."
	curl -fsSL --max-time 120 -o $@.tmp $(NCURSES_URL)
	@actual=$$(shasum -a 256 $@.tmp | awk '{print $$1}'); \
	  if [ "$$actual" != "$(NCURSES_SHA256)" ]; then \
	    echo "❌ ncurses SHA256 mismatch:"; echo "   expected $(NCURSES_SHA256)"; echo "   got      $$actual"; \
	    rm -f $@.tmp; exit 1; \
	  fi
	mv $@.tmp $@

$(TARBALLS_DIR)/zsh-$(ZSH_VERSION).tar.xz: | $(TARBALLS_DIR)
	@echo "📥 Fetching zsh-$(ZSH_VERSION)..."
	curl -fsSL --max-time 120 -o $@.tmp $(ZSH_URL)
	@actual=$$(shasum -a 256 $@.tmp | awk '{print $$1}'); \
	  if [ "$$actual" != "$(ZSH_SHA256)" ]; then \
	    echo "❌ zsh SHA256 mismatch:"; echo "   expected $(ZSH_SHA256)"; echo "   got      $$actual"; \
	    rm -f $@.tmp; exit 1; \
	  fi
	mv $@.tmp $@

$(NCURSES_SRC)/configure: $(TARBALLS_DIR)/ncurses-$(NCURSES_VERSION).tar.gz
	rm -rf $(NCURSES_SRC)
	tar xzf $< -C $(OUT_DIR)
	@touch $@

$(ZSH_SRC)/configure: $(TARBALLS_DIR)/zsh-$(ZSH_VERSION).tar.xz
	rm -rf $(ZSH_SRC)
	tar xJf $< -C $(OUT_DIR)
	@touch $@

# ---------------- ncurses ----------------

# Minimal static ncurses-widec: no shared libs, no progs, no manpages,
# no C++ binding, no Ada, no tests. We only need libncursesw.a + headers
# + the terminfo database for zsh's terminal-handling code.
$(NCURSES_LIB): $(NCURSES_SRC)/configure
	@echo "🔨 Configuring ncurses-$(NCURSES_VERSION)..."
	cd $(NCURSES_SRC) && \
	  CC=$(MUSL_CC) AR=$(MUSL_AR) RANLIB=$(MUSL_RANLIB) \
	  CFLAGS="-O2 -fno-pie" \
	  ./configure --host=x86_64-linux-musl --prefix=$(NCURSES_PREFIX) \
	    --without-shared --without-debug --without-tests \
	    --without-cxx-binding --without-ada --without-manpages \
	    --without-progs --enable-widec --without-pkg-config \
	    --enable-pc-files=no >/dev/null
	@echo "🔨 Building ncurses-$(NCURSES_VERSION)..."
	$(MAKE) -C $(NCURSES_SRC) -j$(shell sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4) >/dev/null
	$(MAKE) -C $(NCURSES_SRC) install >/dev/null

# ---------------- zsh ----------------

$(ZSH_BUILT): $(ZSH_SRC)/configure $(NCURSES_LIB)
	@echo "🔨 Configuring zsh-$(ZSH_VERSION)..."
	cd $(ZSH_SRC) && \
	  CC=$(MUSL_CC) \
	  CPPFLAGS="-I$(NCURSES_PREFIX)/include -I$(NCURSES_PREFIX)/include/ncursesw" \
	  CFLAGS="$(ZSH_CFLAGS)" \
	  LDFLAGS="$(ZSH_LDFLAGS) -L$(NCURSES_PREFIX)/lib" \
	  LIBS="-lncursesw" \
	  ./configure --host=x86_64-linux-musl \
	    --disable-dynamic --disable-cap --disable-pcre --disable-restricted-r \
	    --disable-etcdir --disable-zshenv --disable-zshrc \
	    --disable-zlogin --disable-zprofile --disable-zlogout \
	    --without-tcsetpgrp --enable-multibyte >/dev/null
	@echo "🔨 Building zsh-$(ZSH_VERSION)..."
	$(MAKE) -C $(ZSH_SRC) -j$(shell sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4) >/dev/null

$(BIN): $(ZSH_BUILT)
	cp $< $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@
	@size=$$(wc -c < $@ | tr -d ' '); echo "✅ Built $@ ($$size bytes, stripped)"

# ---------------- maintenance ----------------

clean:
	rm -rf $(NCURSES_SRC) $(ZSH_SRC) $(NCURSES_PREFIX) $(BIN)

distclean:
	rm -rf $(OUT_DIR)
