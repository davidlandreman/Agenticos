# GCC — native x86-64 C compiler: driver, cc1, collect2, cpp, and libgcc.
#
# This is an out-of-tree Canadian cross from the macOS build host to static
# x86_64-linux-musl executables (build = macOS, host = target =
# x86_64-linux-musl).  Unlike the flat binutils ELFs, the shipped artifact is
# a tarball of the pruned install prefix: the driver locates cc1/collect2 and
# libgcc/CRT/headers through its configured --prefix=/host/gcc, so the tree
# layout is load-bearing.
#
# Assembler and linker are NOT configured with --with-as/--with-ld: configure
# would execute those paths on the build host for feature probes, and /bin/as
# exists on macOS as a Mach-O assembler.  Feature probes instead use the
# cross toolchain's x86_64-linux-musl-as from PATH, and the installed driver
# falls back to PATH lookup on-target, finding the shipped GNU binutils at
# /bin/as and /bin/ld.

MUSL_CC      ?= x86_64-linux-musl-gcc
MUSL_GXX     ?= x86_64-linux-musl-g++
CROSS_PREFIX := $(patsubst %gcc,%,$(MUSL_CC))
MUSL_AR      ?= $(CROSS_PREFIX)ar
MUSL_RANLIB  ?= $(CROSS_PREFIX)ranlib
MUSL_STRIP   ?= $(CROSS_PREFIX)strip
MUSL_READELF ?= $(CROSS_PREFIX)readelf

GCC_VERSION := 14.2.0
GCC_SHA256  := a7b39bc69cbf9e25826c5a60ab26477001f7c08d85cec04bc0e29cabed6f3cc9
GCC_URL     := https://ftp.gnu.org/gnu/gcc/gcc-$(GCC_VERSION)/gcc-$(GCC_VERSION).tar.xz

# GMP/MPFR/MPC ride at the exact versions gcc-14.2.0 pins: the tarball names
# are parsed out of the shipped contrib/download_prerequisites and each
# download is verified against the SHA512 manifest shipped in the same tree
# (contrib/prerequisites.sha512), so the pins live upstream, not here.  isl
# is deliberately skipped (graphite off).
PREREQ_BASE := https://gcc.gnu.org/pub/gcc/infrastructure

OUT_DIR       := build
TARBALLS_DIR  := $(OUT_DIR)/tarballs
TARBALL       := $(TARBALLS_DIR)/gcc-$(GCC_VERSION).tar.xz
SRC_DIR       := $(OUT_DIR)/gcc-$(GCC_VERSION)
EXTRACTED     := $(SRC_DIR)/.agenticos-extracted
PREREQS       := $(SRC_DIR)/.agenticos-prereqs
BUILD_DIR     := $(OUT_DIR)/obj
CONFIGURED    := $(BUILD_DIR)/.agenticos-configured
BUILT         := $(BUILD_DIR)/.agenticos-built
DESTDIR_ROOT  := $(OUT_DIR)/destdir
OUTPUT_DIR    := $(OUT_DIR)/out
INSTALL_TREE  := $(OUTPUT_DIR)/gcc
INSTALL_TAR   := $(OUT_DIR)/gcc-install.tar.gz

# The version directory inside the prefix; --with-gcc-major-version-only
# keeps guest paths short (lib/gcc/x86_64-linux-musl/14/...).
GCC_VERDIR := 14
LIBEXEC_SUB := libexec/gcc/x86_64-linux-musl/$(GCC_VERDIR)
LIBSUB      := lib/gcc/x86_64-linux-musl/$(GCC_VERDIR)

SHIPPED_EXES := bin/gcc bin/cpp $(LIBEXEC_SUB)/cc1 $(LIBEXEC_SUB)/collect2

NPROC := $(shell sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)

.PHONY: all clean distclean fetch validate
.DELETE_ON_ERROR:

all: $(INSTALL_TAR)

fetch: $(TARBALL)

$(TARBALLS_DIR):
	mkdir -p $@

$(TARBALL): | $(TARBALLS_DIR)
	@echo "📥 Fetching GCC $(GCC_VERSION)..."
	curl -fsSL --max-time 900 -o $@.tmp $(GCC_URL)
	@actual=$$(shasum -a 256 $@.tmp | awk '{print $$1}'); \
	  if [ "$$actual" != "$(GCC_SHA256)" ]; then \
	    echo "❌ gcc SHA256 mismatch:"; \
	    echo "   expected $(GCC_SHA256)"; \
	    echo "   got      $$actual"; \
	    rm -f $@.tmp; exit 1; \
	  fi
	mv $@.tmp $@

$(EXTRACTED): $(TARBALL)
	rm -rf $(SRC_DIR) $(BUILD_DIR)
	tar xJf $< -C $(OUT_DIR)
	@touch $@

$(PREREQS): $(EXTRACTED)
	@echo "📥 Fetching GMP/MPFR/MPC at gcc-pinned versions..."
	@set -e; cd $(SRC_DIR); \
	  for lib in gmp mpfr mpc; do \
	    ball=$$(sed -n "s/^$$lib='\(.*\)'/\1/p" contrib/download_prerequisites); \
	    [ -n "$$ball" ] || { echo "❌ could not parse $$lib tarball name"; exit 1; }; \
	    if [ ! -f ../tarballs/$$ball ]; then \
	      curl -fsSL --max-time 600 -o ../tarballs/$$ball.tmp $(PREREQ_BASE)/$$ball; \
	      mv ../tarballs/$$ball.tmp ../tarballs/$$ball; \
	    fi; \
	    expected=$$(grep " $$ball$$" contrib/prerequisites.sha512 | awk '{print $$1}'); \
	    actual=$$(shasum -a 512 ../tarballs/$$ball | awk '{print $$1}'); \
	    if [ "$$actual" != "$$expected" ]; then \
	      echo "❌ $$ball SHA512 mismatch against contrib/prerequisites.sha512"; \
	      rm -f ../tarballs/$$ball; exit 1; \
	    fi; \
	    dir=$$(echo $$ball | sed 's/\.tar\..*$$//'); \
	    rm -rf $$dir $$lib; \
	    tar xf ../tarballs/$$ball; \
	    ln -s $$dir $$lib; \
	    echo "✅ $$ball"; \
	  done
	@touch $@

$(CONFIGURED): $(PREREQS)
	@echo "⚙️  Configuring GCC $(GCC_VERSION) (Canadian cross)..."
	mkdir -p $(BUILD_DIR)
	@build_triplet=$$($(SRC_DIR)/config.guess); \
	  build_sysroot=$$(cd $$($(MUSL_CC) -print-sysroot) && pwd); \
	  cd $(BUILD_DIR) && \
	  CC=$(MUSL_CC) CXX=$(MUSL_GXX) AR=$(MUSL_AR) RANLIB=$(MUSL_RANLIB) \
	  CFLAGS="-Os" CXXFLAGS="-Os" LDFLAGS="-static -no-pie" \
	  ../gcc-$(GCC_VERSION)/configure \
	    --build=$$build_triplet \
	    --host=x86_64-linux-musl \
	    --target=x86_64-linux-musl \
	    --prefix=/host/gcc \
	    --with-gcc-major-version-only \
	    --with-sysroot=/host/sysroot \
	    --with-build-sysroot=$$build_sysroot \
	    --with-native-system-header-dir=/include \
	    --with-pkgversion="AgenticOS" \
	    --enable-languages=c \
	    --disable-shared \
	    --enable-static \
	    --disable-bootstrap \
	    --disable-multilib \
	    --disable-nls \
	    --disable-werror \
	    --disable-lto \
	    --disable-plugin \
	    --disable-libsanitizer \
	    --disable-libssp \
	    --disable-libquadmath \
	    --disable-libgomp \
	    --disable-libatomic \
	    --disable-libitm \
	    --disable-libvtv \
	    --disable-decimal-float \
	    --disable-fixed-point \
	    --disable-fixincludes \
	    --disable-gnu-indirect-function \
	    --enable-tls \
	    --enable-initfini-array \
	    --without-isl \
	    --without-zstd >/dev/null
	@touch $@

$(BUILT): $(CONFIGURED)
	@echo "🔨 Building GCC $(GCC_VERSION) (this takes a while)..."
	$(MAKE) -C $(BUILD_DIR) -j$(NPROC) all-gcc all-target-libgcc >/dev/null
	@touch $@

# Install into a scratch DESTDIR, then prune to the shipped set: driver, cpp,
# cc1, collect2, libgcc/CRT objects, and GCC's internal headers.  Everything
# else (gcov, gcc-ar wrappers, target-prefixed driver copies, install-tools,
# share/) is dropped.
$(INSTALL_TAR): $(BUILT)
	@echo "📦 Installing and pruning GCC tree..."
	rm -rf $(DESTDIR_ROOT) $(INSTALL_TREE)
	$(MAKE) -C $(BUILD_DIR) DESTDIR=$(abspath $(DESTDIR_ROOT)) \
	  install-gcc install-target-libgcc >/dev/null
	mkdir -p $(INSTALL_TREE)/bin $(INSTALL_TREE)/$(LIBEXEC_SUB) $(INSTALL_TREE)/$(LIBSUB)
	@set -e; src=$(DESTDIR_ROOT)/host/gcc; \
	  for exe in $(SHIPPED_EXES); do \
	    cp $$src/$$exe $(INSTALL_TREE)/$$exe; \
	    $(MUSL_STRIP) --strip-all $(INSTALL_TREE)/$$exe; \
	  done; \
	  cp $$src/$(LIBSUB)/libgcc.a $(INSTALL_TREE)/$(LIBSUB)/; \
	  cp $$src/$(LIBSUB)/crt*.o $(INSTALL_TREE)/$(LIBSUB)/; \
	  cp -R $$src/$(LIBSUB)/include $(INSTALL_TREE)/$(LIBSUB)/include
	tar czf $@.tmp -C $(INSTALL_TREE) .
	mv -f $@.tmp $@
	@echo "✅ $(INSTALL_TAR) ($$(wc -c < $@ | tr -d ' ') bytes)"

validate: $(INSTALL_TAR)
	@for exe in $(SHIPPED_EXES); do \
	  binary=$(INSTALL_TREE)/$$exe; \
	  type=$$($(MUSL_READELF) -h $$binary | awk '/Type:/ { print $$2 }'); \
	  machine=$$($(MUSL_READELF) -h $$binary | awk -F: '/Machine:/ { sub(/^[[:space:]]*/, "", $$2); print $$2 }'); \
	  if [ "$$type" != EXEC ] || [ "$$machine" != "Advanced Micro Devices X86-64" ]; then \
	    echo "❌ invalid ELF identity: $$binary ($$type, $$machine)"; exit 1; \
	  fi; \
	  if $(MUSL_READELF) -l $$binary | grep -q ' INTERP '; then \
	    echo "❌ PT_INTERP present: $$binary"; exit 1; \
	  fi; \
	  if $(MUSL_READELF) -d $$binary 2>/dev/null | grep -q '(NEEDED)'; then \
	    echo "❌ DT_NEEDED present: $$binary"; exit 1; \
	  fi; \
	  echo "✅ $$binary ($$(wc -c < $$binary | tr -d ' ') bytes)"; \
	done

clean:
	rm -rf $(BUILD_DIR) $(DESTDIR_ROOT) $(OUTPUT_DIR) $(INSTALL_TAR)

distclean:
	rm -rf $(OUT_DIR)
