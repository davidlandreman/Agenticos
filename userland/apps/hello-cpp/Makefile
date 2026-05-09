# HELLOCPP — first C++ user app.
#
# Builds a statically-linked ET_EXEC binary against musl + libstdc++ via
# the host's x86_64-linux-musl cross-compiler. The kernel ELF loader
# accepts only ET_EXEC (-static -no-pie); the readelf check in build.sh
# asserts that on every build so a toolchain that defaults to ET_DYN
# fails loud rather than silently at load time.
#
# Override the toolchain by setting MUSL_GXX:
#   make MUSL_GXX=x86_64-linux-musl-g++-13

MUSL_GXX ?= x86_64-linux-musl-g++

# `-static -no-pie` is the kernel-mandated link mode (the loader rejects
# ET_DYN). `-fno-pie -no-pie` defends against distros whose CRT defaults
# to PIE even when -no-pie is passed only at link time.
CXXFLAGS = -O2 -Wall -Wextra -fno-pie
LDFLAGS = -static -no-pie

OUT_DIR = build
BIN = $(OUT_DIR)/hello-cpp

.PHONY: all clean

all: $(BIN)

$(BIN): src/main.cpp | $(OUT_DIR)
	$(MUSL_GXX) $(CXXFLAGS) $< -o $@ $(LDFLAGS)

$(OUT_DIR):
	mkdir -p $(OUT_DIR)

clean:
	rm -rf $(OUT_DIR)
