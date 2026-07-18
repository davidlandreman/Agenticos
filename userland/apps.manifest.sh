# shellcheck shell=bash
# name | source dir | build kind | staged 8.3 name | ship kind | toolchain |
# build output (relative to userland/) | committed prebuilt (or -)
app_row hello            apps/hello             cargo HELLO.ELF    built-every-run rust-nightly target/x86_64-unknown-none/release/hello            -
app_row guilaunch        apps/guilaunch         cargo GLAUNCH.ELF  built-every-run rust-nightly target/x86_64-unknown-none/release/guilaunch        -
app_row guidemo          apps/guidemo           cargo GUIDEMO.ELF  built-every-run rust-nightly target/x86_64-unknown-none/release/guidemo          -
app_row notepad          apps/notepad           cargo NOTEPAD.ELF  built-every-run rust-nightly target/x86_64-unknown-none/release/notepad          -
app_row taskmgr          apps/taskmgr           cargo TASKMGR.ELF  built-every-run rust-nightly target/x86_64-unknown-none/release/taskmgr          -
app_row calc             apps/calc              cargo CALC.ELF     built-every-run rust-nightly target/x86_64-unknown-none/release/calc             -
app_row glgame           apps/glgame            cargo GLGAME.ELF   built-every-run rust-nightly target/x86_64-unknown-none/release/glgame           -
app_row painting         apps/painting          cargo PAINTING.ELF built-every-run rust-nightly target/x86_64-unknown-none/release/painting         -
app_row fileman          apps/fileman           cargo FILEMAN.ELF  built-every-run rust-nightly target/x86_64-unknown-none/release/fileman          -
app_row control          apps/control           cargo CONTROL.ELF  built-every-run rust-nightly target/x86_64-unknown-none/release/control          -
app_row hello-cpp        apps/hello-cpp         make  HELLOCPP.ELF built-every-run musl-cxx      apps/hello-cpp/build/hello-cpp                    -
app_row zsh              apps/zsh               make  ZSH.ELF      prebuilt-managed musl-cc     apps/zsh/build/zsh                                prebuilt/ZSH.ELF
app_row busybox          apps/busybox           make  BB.ELF       prebuilt-managed musl-cc     apps/busybox/build/busybox                        prebuilt/BB.ELF
app_row tcc              apps/tcc               make  TCC.ELF      prebuilt-managed musl-cc     apps/tcc/build/tcc                                prebuilt/TCC.ELF
app_row links2           apps/links2            make  LINKS.ELF    prebuilt-managed musl-cc     apps/links2/build/links                           prebuilt/LINKS.ELF
app_row binutils-addr2line apps/binutils         make  ADDRLINE.ELF prebuilt-managed musl-cc     apps/binutils/build/out/addr2line                 prebuilt/binutils/ADDRLINE.ELF
app_row binutils-ar       apps/binutils          make  AR.ELF       prebuilt-managed musl-cc     apps/binutils/build/out/ar                        prebuilt/binutils/AR.ELF
app_row binutils-as       apps/binutils          make  AS.ELF       prebuilt-managed musl-cc     apps/binutils/build/out/as                        prebuilt/binutils/AS.ELF
app_row binutils-cxxfilt  apps/binutils          make  CPPFILT.ELF  prebuilt-managed musl-cc     apps/binutils/build/out/c++filt                   prebuilt/binutils/CPPFILT.ELF
app_row binutils-elfedit  apps/binutils          make  ELFEDIT.ELF  prebuilt-managed musl-cc     apps/binutils/build/out/elfedit                   prebuilt/binutils/ELFEDIT.ELF
app_row binutils-ld       apps/binutils          make  LD.ELF       prebuilt-managed musl-cc     apps/binutils/build/out/ld                        prebuilt/binutils/LD.ELF
app_row binutils-nm       apps/binutils          make  NM.ELF       prebuilt-managed musl-cc     apps/binutils/build/out/nm                        prebuilt/binutils/NM.ELF
app_row binutils-objcopy  apps/binutils          make  OBJCOPY.ELF  prebuilt-managed musl-cc     apps/binutils/build/out/objcopy                   prebuilt/binutils/OBJCOPY.ELF
app_row binutils-objdump  apps/binutils          make  OBJDUMP.ELF  prebuilt-managed musl-cc     apps/binutils/build/out/objdump                   prebuilt/binutils/OBJDUMP.ELF
app_row binutils-ranlib   apps/binutils          make  RANLIB.ELF   prebuilt-managed musl-cc     apps/binutils/build/out/ranlib                    prebuilt/binutils/RANLIB.ELF
app_row binutils-readelf  apps/binutils          make  READELF.ELF  prebuilt-managed musl-cc     apps/binutils/build/out/readelf                   prebuilt/binutils/READELF.ELF
app_row binutils-size     apps/binutils          make  SIZE.ELF     prebuilt-managed musl-cc     apps/binutils/build/out/size                      prebuilt/binutils/SIZE.ELF
app_row binutils-strings  apps/binutils          make  STRINGS.ELF  prebuilt-managed musl-cc     apps/binutils/build/out/strings                   prebuilt/binutils/STRINGS.ELF
app_row binutils-strip    apps/binutils          make  STRIP.ELF    prebuilt-managed musl-cc     apps/binutils/build/out/strip                     prebuilt/binutils/STRIP.ELF
app_row compiler-crt     apps/compiler-compat   make  CCCRT.ELF    test-fixture    musl-cc       apps/compiler-compat/build/CCCRT.ELF              prebuilt/compiler-compat/CCCRT.ELF
app_row compiler-libc    apps/compiler-compat   make  CCLIBC.ELF   test-fixture    musl-cc       apps/compiler-compat/build/CCLIBC.ELF             prebuilt/compiler-compat/CCLIBC.ELF
app_row compiler-probe   apps/compiler-compat   make  CCPROBE.ELF  test-fixture    musl-cc       apps/compiler-compat/build/CCPROBE.ELF            prebuilt/compiler-compat/CCPROBE.ELF
app_row network-test     apps/network-test      make  NETTEST.ELF  test-fixture    musl-cc       apps/network-test/build/NETTEST.ELF               prebuilt/network/NETTEST.ELF
