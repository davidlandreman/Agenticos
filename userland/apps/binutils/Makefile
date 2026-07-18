# GNU binutils — native x86-64 assembler, linker, archive, and ELF tools.
#
# This is an out-of-tree Canadian cross from the macOS build host to static
# x86_64-linux-musl executables.  Binutils links programs through libtool, so
# -all-static (not merely -static) is required to eliminate PT_INTERP.

MUSL_CC      ?= x86_64-linux-musl-gcc
CROSS_PREFIX := $(patsubst %gcc,%,$(MUSL_CC))
MUSL_AR      ?= $(CROSS_PREFIX)ar
MUSL_RANLIB  ?= $(CROSS_PREFIX)ranlib
MUSL_STRIP   ?= $(CROSS_PREFIX)strip
MUSL_READELF ?= $(CROSS_PREFIX)readelf
HOST_CC      ?= cc

BINUTILS_VERSION := 2.46.0
BINUTILS_SHA256  := d75a94f4d73e7a4086f7513e67e439e8fcdcbb726ffe63f4661744e6256b2cf2
BINUTILS_URL     := https://ftp.gnu.org/gnu/binutils/binutils-$(BINUTILS_VERSION).tar.xz

OUT_DIR       := build
TARBALLS_DIR  := $(OUT_DIR)/tarballs
TARBALL       := $(TARBALLS_DIR)/binutils-$(BINUTILS_VERSION).tar.xz
SRC_DIR       := $(OUT_DIR)/binutils-$(BINUTILS_VERSION)
EXTRACTED     := $(SRC_DIR)/.agenticos-extracted
BUILD_DIR     := $(OUT_DIR)/obj
CONFIGURED    := $(BUILD_DIR)/.agenticos-configured
BUILT         := $(BUILD_DIR)/.agenticos-built
OUTPUT_DIR    := $(OUT_DIR)/out

TOOLS := addr2line ar as c++filt elfedit ld nm objcopy objdump ranlib readelf size strings strip
OUTPUTS := $(addprefix $(OUTPUT_DIR)/,$(TOOLS))

NPROC := $(shell sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)

.PHONY: all clean distclean fetch validate
.DELETE_ON_ERROR:

all: $(OUTPUTS)

fetch: $(TARBALL)

$(TARBALLS_DIR):
	mkdir -p $@

$(TARBALL): | $(TARBALLS_DIR)
	@echo "📥 Fetching GNU binutils $(BINUTILS_VERSION)..."
	curl -fsSL --max-time 240 -o $@.tmp $(BINUTILS_URL)
	@actual=$$(shasum -a 256 $@.tmp | awk '{print $$1}'); \
	  if [ "$$actual" != "$(BINUTILS_SHA256)" ]; then \
	    echo "❌ binutils SHA256 mismatch:"; \
	    echo "   expected $(BINUTILS_SHA256)"; \
	    echo "   got      $$actual"; \
	    rm -f $@.tmp; exit 1; \
	  fi
	mv $@.tmp $@

$(EXTRACTED): $(TARBALL)
	rm -rf $(SRC_DIR) $(BUILD_DIR)
	tar xJf $< -C $(OUT_DIR)
	@touch $@

$(CONFIGURED): $(EXTRACTED)
	@echo "⚙️  Configuring GNU binutils $(BINUTILS_VERSION)..."
	mkdir -p $(BUILD_DIR)
	@build_triplet=$$($(SRC_DIR)/config.guess); \
	  cd $(BUILD_DIR) && \
	  CC_FOR_BUILD=$(HOST_CC) \
	  CC=$(MUSL_CC) AR=$(MUSL_AR) RANLIB=$(MUSL_RANLIB) \
	  CFLAGS="-Os -fno-pie" LDFLAGS="-static -no-pie" \
	  ../binutils-$(BINUTILS_VERSION)/configure \
	    --build=$$build_triplet \
	    --host=x86_64-linux-musl \
	    --target=x86_64-linux-musl \
	    --with-sysroot=/host/sysroot \
	    --with-lib-path=/host/sysroot/lib \
	    --disable-nls \
	    --disable-werror \
	    --disable-shared \
	    --enable-static \
	    --disable-gdb \
	    --disable-gdbserver \
	    --disable-gprofng \
	    --disable-gold \
	    --disable-sim \
	    --disable-libdecnumber \
	    --disable-readline \
	    --disable-plugins \
	    --without-zstd \
	    --without-debuginfod \
	    --without-babeltrace \
	    --without-libiconv >/dev/null
	@touch $@

$(BUILT): $(CONFIGURED)
	@echo "🔨 Building GNU binutils $(BINUTILS_VERSION)..."
	$(MAKE) -C $(BUILD_DIR) -j$(NPROC) \
	  LDFLAGS="-static -no-pie" \
	  all-binutils all-gas all-ld >/dev/null
	@# Top-level make propagates LDFLAGS into still-unconfigured subprojects,
	@# where libtool's -all-static is not a compiler flag. Configure/build once
	@# with -static, then relink the three already-configured program trees with
	@# -all-static so the final executables have no PT_INTERP.
	$(MAKE) -C $(BUILD_DIR)/binutils clean >/dev/null
	$(MAKE) -C $(BUILD_DIR)/gas clean >/dev/null
	$(MAKE) -C $(BUILD_DIR)/ld clean >/dev/null
	$(MAKE) -C $(BUILD_DIR) -j$(NPROC) \
	  LDFLAGS="-all-static -no-pie" \
	  all-binutils all-gas all-ld >/dev/null
	@touch $@

$(OUTPUT_DIR):
	mkdir -p $@

$(OUTPUT_DIR)/addr2line: $(BUILT) | $(OUTPUT_DIR)
	cp $(BUILD_DIR)/binutils/addr2line $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@

$(OUTPUT_DIR)/ar: $(BUILT) | $(OUTPUT_DIR)
	cp $(BUILD_DIR)/binutils/ar $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@

$(OUTPUT_DIR)/as: $(BUILT) | $(OUTPUT_DIR)
	cp $(BUILD_DIR)/gas/as-new $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@

$(OUTPUT_DIR)/c++filt: $(BUILT) | $(OUTPUT_DIR)
	cp $(BUILD_DIR)/binutils/cxxfilt $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@

$(OUTPUT_DIR)/elfedit: $(BUILT) | $(OUTPUT_DIR)
	cp $(BUILD_DIR)/binutils/elfedit $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@

$(OUTPUT_DIR)/ld: $(BUILT) | $(OUTPUT_DIR)
	cp $(BUILD_DIR)/ld/ld-new $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@

$(OUTPUT_DIR)/nm: $(BUILT) | $(OUTPUT_DIR)
	cp $(BUILD_DIR)/binutils/nm-new $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@

$(OUTPUT_DIR)/objcopy: $(BUILT) | $(OUTPUT_DIR)
	cp $(BUILD_DIR)/binutils/objcopy $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@

$(OUTPUT_DIR)/objdump: $(BUILT) | $(OUTPUT_DIR)
	cp $(BUILD_DIR)/binutils/objdump $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@

$(OUTPUT_DIR)/ranlib: $(BUILT) | $(OUTPUT_DIR)
	cp $(BUILD_DIR)/binutils/ranlib $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@

$(OUTPUT_DIR)/readelf: $(BUILT) | $(OUTPUT_DIR)
	cp $(BUILD_DIR)/binutils/readelf $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@

$(OUTPUT_DIR)/size: $(BUILT) | $(OUTPUT_DIR)
	cp $(BUILD_DIR)/binutils/size $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@

$(OUTPUT_DIR)/strings: $(BUILT) | $(OUTPUT_DIR)
	cp $(BUILD_DIR)/binutils/strings $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@

$(OUTPUT_DIR)/strip: $(BUILT) | $(OUTPUT_DIR)
	cp $(BUILD_DIR)/binutils/strip-new $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@

validate: $(OUTPUTS)
	@for binary in $(OUTPUTS); do \
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
	rm -rf $(BUILD_DIR) $(OUTPUT_DIR)

distclean:
	rm -rf $(OUT_DIR)
