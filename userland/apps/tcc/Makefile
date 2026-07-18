# TinyCC — first on-target C compiler.
#
# Cross-builds a static musl TinyCC (compiler + assembler + linker in one
# ET_EXEC binary) and assembles the minimal musl sysroot it compiles
# against inside the guest. Both artifacts are prebuilt-managed:
#   build/tcc                 -> committed as userland/prebuilt/TCC.ELF
#   build/tcc-sysroot.tar.gz  -> committed as userland/prebuilt/tcc-sysroot.tar.gz
#
# The sysroot stages to host_share/sysroot/ and appears in the guest at:
#   /host/sysroot/include          pruned musl + linux-uapi headers
#   /host/sysroot/lib              crt1.o crti.o crtn.o libc.a (+ empty
#                                  POSIX compat archives so -lm/-lpthread work)
#   /host/sysroot/lib/tcc          libtcc1.a + TCC's private headers
#   /host/sysroot/examples         smoke-test C sources
#
# TCC's default search paths and an implicit -static are compiled in at
# configure time (the kernel loader rejects PT_INTERP, so dynamic output
# would be unexecutable), so in-guest usage needs no flags:
#   cd /work && tcc -o hello /host/sysroot/examples/hello.c && ./hello
#
# Source pin: TinyCC mob branch snapshot (musl config + current x86-64
# codegen; the 0.9.27 release predates --config-musl). Bumping the commit
# bumps the SHA in lockstep.
#
# Cross-build wrinkles, both handled with upstream mechanisms (no patches):
#   - c2str.exe is a build-host helper the Makefile compiles with the
#     target CC; we pre-generate it (and tccdefs_.h) with the host cc.
#   - libtcc1.a is normally compiled by the freshly built tcc, which is a
#     Linux binary the macOS host cannot run; the upstream-supported
#     `x86_64-libtcc1-usegcc=yes` switch builds it with the cross gcc.

MUSL_CC      ?= x86_64-linux-musl-gcc
MUSL_AR      ?= $(patsubst %-gcc,%-ar,$(MUSL_CC))
MUSL_STRIP   ?= $(patsubst %-gcc,%-strip,$(MUSL_CC))
CROSS_PREFIX := $(patsubst %gcc,%,$(MUSL_CC))
HOST_CC      ?= cc

TCC_COMMIT := d9d02c56401e43be43760b63f7d82f771a7ed1f6
TCC_SHA256 := b25f68a46930549553f6d679106d99d5a00b4d8f39a178baaeae6a50018e37ae
TCC_URL    := https://repo.or.cz/tinycc.git/snapshot/$(TCC_COMMIT).tar.gz

OUT_DIR      := build
TARBALLS_DIR := $(OUT_DIR)/tarballs
TCC_TARBALL  := $(TARBALLS_DIR)/tinycc-$(TCC_COMMIT).tar.gz
# repo.or.cz snapshots extract to tinycc-<7-char short hash>.
TCC_SRC       := $(OUT_DIR)/tinycc-$(shell printf '%.7s' $(TCC_COMMIT))
TCC_EXTRACTED := $(TCC_SRC)/.agenticos-extracted
TCC_BUILT     := $(TCC_SRC)/tcc
BIN           := $(OUT_DIR)/tcc

SYSROOT     := $(OUT_DIR)/sysroot
SYSROOT_TAR := $(OUT_DIR)/tcc-sysroot.tar.gz
TOOLCHAIN_SYSROOT := $(shell $(MUSL_CC) -print-sysroot 2>/dev/null)

# Header trees that no C-only guest compile needs: C++ runtime headers and
# rarely used kernel-uapi subtrees. Pruning keeps the vvfat file count and
# FAT-PIO read surface down.
INCLUDE_PRUNE := c++ drm sound rdma scsi

# Empty POSIX compat archives: musl folds their contents into libc.a but
# ships the stubs so `-lm -lpthread ...` link lines keep working.
COMPAT_LIBS := libm.a libpthread.a libdl.a libcrypt.a librt.a libutil.a libxnet.a libresolv.a

# Guest paths compiled into the tcc binary.
GUEST_SYSROOT := /host/sysroot
GUEST_TCCDIR  := $(GUEST_SYSROOT)/lib/tcc

NPROC := $(shell sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)

.PHONY: all clean distclean fetch
.DELETE_ON_ERROR:

all: $(BIN) $(SYSROOT_TAR)

# ---------------- fetch + extract ----------------

fetch: $(TCC_TARBALL)

$(TARBALLS_DIR):
	mkdir -p $@

$(TCC_TARBALL): | $(TARBALLS_DIR)
	@echo "📥 Fetching tinycc mob snapshot $(TCC_COMMIT)..."
	curl -fsSL --max-time 180 -o $@.tmp $(TCC_URL)
	@actual=$$(shasum -a 256 $@.tmp | awk '{print $$1}'); \
	  if [ "$$actual" != "$(TCC_SHA256)" ]; then \
	    echo "❌ tinycc SHA256 mismatch:"; echo "   expected $(TCC_SHA256)"; echo "   got      $$actual"; \
	    rm -f $@.tmp; exit 1; \
	  fi
	mv $@.tmp $@

$(TCC_EXTRACTED): $(TCC_TARBALL)
	rm -rf $(TCC_SRC)
	tar xzf $< -C $(OUT_DIR)
	@touch $@

# ---------------- tcc + libtcc1.a ----------------

$(TCC_BUILT): $(TCC_EXTRACTED)
	@echo "🔨 Configuring tinycc..."
	cd $(TCC_SRC) && \
	  ./configure --cpu=x86_64 --targetos=Linux \
	    --cross-prefix=$(CROSS_PREFIX) --cc=gcc --ar=ar \
	    --config-musl --enable-static \
	    --tccdir=$(GUEST_TCCDIR) \
	    --sysincludepaths="{B}/include:$(GUEST_SYSROOT)/include" \
	    --libpaths="{B}:$(GUEST_SYSROOT)/lib" \
	    --crtprefix=$(GUEST_SYSROOT)/lib \
	    --elfinterp=/lib/ld-musl-x86_64.so.1 \
	    --tcc-switches=-static \
	    --extra-cflags="-O2 -fno-pie" --extra-ldflags="-static -no-pie" >/dev/null
	@echo "🔨 Pre-generating tccdefs_.h with the host cc (c2str must run on the build host)..."
	cd $(TCC_SRC) && \
	  $(HOST_CC) -DC2STR conftest.c -o c2str.exe && \
	  ./c2str.exe include/tccdefs.h tccdefs_.h
	@echo "🔨 Building tinycc + libtcc1.a..."
	$(MAKE) -C $(TCC_SRC) -j$(NPROC) all x86_64-libtcc1-usegcc=yes >/dev/null

$(BIN): $(TCC_BUILT)
	cp $< $@.tmp
	$(MUSL_STRIP) --strip-all $@.tmp
	mv -f $@.tmp $@
	@size=$$(wc -c < $@ | tr -d ' '); echo "✅ Built $@ ($$size bytes, stripped)"

# ---------------- sysroot ----------------

$(SYSROOT_TAR): $(TCC_BUILT) $(wildcard examples/*.c)
	@test -n "$(TOOLCHAIN_SYSROOT)" -a -d "$(TOOLCHAIN_SYSROOT)/include" || { \
	  echo "❌ Cannot locate the musl toolchain sysroot ($(MUSL_CC) -print-sysroot)"; exit 1; }
	@echo "🔨 Assembling guest sysroot..."
	rm -rf $(SYSROOT)
	mkdir -p $(SYSROOT)/lib/tcc/include $(SYSROOT)/examples
	cp -R $(TOOLCHAIN_SYSROOT)/include $(SYSROOT)/include
	@for prune in $(INCLUDE_PRUNE); do rm -rf $(SYSROOT)/include/$$prune; done
	cp $(TOOLCHAIN_SYSROOT)/lib/crt1.o $(TOOLCHAIN_SYSROOT)/lib/crti.o \
	   $(TOOLCHAIN_SYSROOT)/lib/crtn.o $(TOOLCHAIN_SYSROOT)/lib/libc.a $(SYSROOT)/lib/
	@for lib in $(COMPAT_LIBS); do \
	  if [ -f "$(TOOLCHAIN_SYSROOT)/lib/$$lib" ]; then cp "$(TOOLCHAIN_SYSROOT)/lib/$$lib" $(SYSROOT)/lib/; fi; \
	done
	cp $(TCC_SRC)/include/*.h $(SYSROOT)/lib/tcc/include/
	cp $(TCC_SRC)/libtcc1.a $(SYSROOT)/lib/tcc/
	cp examples/*.c $(SYSROOT)/examples/
	$(MUSL_CC) -O2 -static -pthread examples/pthread_tls.c -o $(SYSROOT)/examples/pthread_tls.elf
	$(MUSL_STRIP) --strip-all $(SYSROOT)/examples/pthread_tls.elf
	@echo "🔨 Packing $(SYSROOT_TAR)..."
	@# Normalize mtimes so repacks diff meaningfully; ownership is zeroed
	@# by gtar when available, otherwise left to the host tar.
	find $(SYSROOT) -exec touch -t 202001010000 {} +
	@if command -v gtar >/dev/null 2>&1; then \
	  (cd $(SYSROOT) && gtar --sort=name --owner=0 --group=0 --numeric-owner -czf ../../$(SYSROOT_TAR) .); \
	else \
	  (cd $(SYSROOT) && find . -type f | sort | COPYFILE_DISABLE=1 tar -czf ../../$(SYSROOT_TAR) -T -); \
	fi
	@size=$$(wc -c < $(SYSROOT_TAR) | tr -d ' '); \
	  files=$$(find $(SYSROOT) -type f | wc -l | tr -d ' '); \
	  echo "✅ Packed $(SYSROOT_TAR) ($$size bytes, $$files files)"

# ---------------- maintenance ----------------

clean:
	rm -rf $(TCC_SRC) $(SYSROOT) $(SYSROOT_TAR) $(BIN)

distclean:
	rm -rf $(OUT_DIR)
