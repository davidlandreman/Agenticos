//! Booted end-to-end coverage for the GNU binutils userspace port.
//!
//! The tests launch the committed static-musl tools through the production
//! ELF loader, create outputs on `/work`, and run a freshly assembled and
//! linked executable. Missing staged artifacts are hard failures.

use alloc::string::String;
use alloc::vec::Vec;

use crate::lib::test_utils::Testable;
use crate::userland::lifecycle::ExitKind;

const TOOLS: &[(&str, &str)] = &[
    ("addr2line", "/host/ADDRLINE.ELF"),
    ("ar", "/host/AR.ELF"),
    ("as", "/host/AS.ELF"),
    ("c++filt", "/host/CPPFILT.ELF"),
    ("elfedit", "/host/ELFEDIT.ELF"),
    ("ld", "/host/LD.ELF"),
    ("nm", "/host/NM.ELF"),
    ("objcopy", "/host/OBJCOPY.ELF"),
    ("objdump", "/host/OBJDUMP.ELF"),
    ("ranlib", "/host/RANLIB.ELF"),
    ("readelf", "/host/READELF.ELF"),
    ("size", "/host/SIZE.ELF"),
    ("strings", "/host/STRINGS.ELF"),
    ("strip", "/host/STRIP.ELF"),
];

fn run_to_exit(path: &str, argv: &[&str]) -> i64 {
    assert!(crate::fs::exists(path), "missing binary: {}", path);
    let prior_trace = crate::userland::abi::is_trace_mode();
    crate::userland::abi::set_trace_mode(true);
    crate::userland::abi::reset_unknown_syscall_trace();
    let result =
        crate::userland::launcher::launch_user_binary(path, argv, &["PATH=/bin:/host", "LANG=C"]);
    crate::userland::abi::set_trace_mode(prior_trace);
    crate::userland::abi::clear_user_va_bounds();
    let (kind, code) = result.unwrap_or_else(|error| panic!("{} launch failed: {}", path, error));
    assert!(
        matches!(kind, ExitKind::Cooperative),
        "{} exited via {:?} ({})",
        path,
        kind,
        code
    );
    code
}

fn tool(name: &str, args: &[&str]) {
    let path = TOOLS
        .iter()
        .find_map(|(candidate, path)| (*candidate == name).then_some(*path))
        .unwrap_or_else(|| panic!("unknown binutils test tool: {}", name));
    let mut argv = alloc::vec![name];
    argv.extend_from_slice(args);
    let code = run_to_exit(path, &argv);
    assert_eq!(code, 0, "{} {:?} exited with {}", name, args, code);
}

fn tcc(args: &[&str]) {
    let mut argv = alloc::vec!["tcc"];
    argv.extend_from_slice(args);
    let code = run_to_exit("/host/TCC.ELF", &argv);
    assert_eq!(code, 0, "tcc {:?} exited with {}", args, code);
}

fn zsh(command: &str) {
    let code = run_to_exit("/host/ZSH.ELF", &["zsh", "-f", "-c", command]);
    assert_eq!(code, 0, "zsh command failed: {}", command);
}

fn read_file(path: &str) -> Vec<u8> {
    crate::fs::File::open_read(path)
        .unwrap_or_else(|error| panic!("open {} failed: {:?}", path, error))
        .read_to_vec()
        .unwrap_or_else(|error| panic!("read {} failed: {:?}", path, error))
}

fn unlink_if_present(path: &str) {
    if crate::fs::exists(path) {
        crate::fs::vfs::vfs_unlink(path).unwrap_or_else(|error| {
            panic!("cleanup {} failed: {:?}", path, error);
        });
    }
}

fn copy_file(from: &str, to: &str) {
    let bytes = crate::fs::File::open_read(from)
        .unwrap_or_else(|error| panic!("open {} failed: {:?}", from, error))
        .read_to_vec()
        .unwrap_or_else(|error| panic!("read {} failed: {:?}", from, error));
    let output = crate::fs::File::create(to)
        .unwrap_or_else(|error| panic!("create {} failed: {:?}", to, error));
    let written = output
        .write(&bytes)
        .unwrap_or_else(|error| panic!("write {} failed: {:?}", to, error));
    assert_eq!(written, bytes.len());
}

fn assert_static_exec(path: &str) {
    let bytes = crate::fs::File::open_read(path)
        .expect("open output ELF")
        .read_to_vec()
        .expect("read output ELF");
    assert!(bytes.len() >= 64, "{} is shorter than ELF64 header", path);
    assert_eq!(&bytes[..4], b"\x7fELF");
    assert_eq!(bytes[4], 2, "{} must be ELF64", path);
    assert_eq!(u16::from_le_bytes([bytes[16], bytes[17]]), 2, "ET_EXEC");
    assert_eq!(u16::from_le_bytes([bytes[18], bytes[19]]), 0x3e, "x86-64");

    let phoff = u64::from_le_bytes(bytes[32..40].try_into().unwrap()) as usize;
    let phentsize = u16::from_le_bytes(bytes[54..56].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(bytes[56..58].try_into().unwrap()) as usize;
    for index in 0..phnum {
        let offset = phoff + index * phentsize;
        assert!(offset + 4 <= bytes.len(), "program header out of range");
        let kind = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        assert_ne!(kind, 3, "{} unexpectedly contains PT_INTERP", path);
    }
}

fn test_binutils_artifacts_staged() {
    for (_, path) in TOOLS {
        assert!(crate::fs::exists(path), "staged tool missing: {}", path);
    }
    for path in [
        "/host/BINUTILS/EXIT42.S",
        "/host/BINUTILS/ARCHMAIN.S",
        "/host/BINUTILS/SYMBOLS.C",
        "/host/TCC.ELF",
    ] {
        assert!(crate::fs::exists(path), "fixture missing: {}", path);
    }
}

fn test_binutils_versions_launch() {
    for (name, _) in TOOLS {
        tool(name, &["--version"]);
    }
}

fn test_gas_ld_create_and_run_executable() {
    for path in ["/work/bu-exit.o", "/work/bu-exit"] {
        unlink_if_present(path);
    }
    tool(
        "as",
        &["--64", "-o", "/work/bu-exit.o", "/host/BINUTILS/EXIT42.S"],
    );
    tool("ld", &["-static", "-o", "/work/bu-exit", "/work/bu-exit.o"]);
    assert_static_exec("/work/bu-exit");
    assert_eq!(run_to_exit("/work/bu-exit", &["/work/bu-exit"]), 42);
    tool("readelf", &["-h", "/work/bu-exit"]);
    tool("objdump", &["-f", "/work/bu-exit"]);
    tool("size", &["/work/bu-exit"]);
    tool("addr2line", &["-e", "/work/bu-exit", "0x401000"]);
    unlink_if_present("/work/bu-exit.o");
    unlink_if_present("/work/bu-exit");
}

fn test_archive_and_elf_transform_tools() {
    for path in [
        "/work/bu-archive-main.o",
        "/work/bu-archive-probe",
        "/work/bu-symbols.o",
        "/work/bu-symbols-copy.o",
        "/work/libbuprobe.a",
        "/work/bu-nm.txt",
        "/work/bu-strings.txt",
        "/work/bu-cxxfilt.txt",
    ] {
        unlink_if_present(path);
    }
    // FAT presents staged names in uppercase, so force C mode rather than
    // letting the `.C` suffix select TinyCC's unsupported C++ frontend.
    tcc(&[
        "-x",
        "c",
        "-g",
        "-c",
        "/host/BINUTILS/SYMBOLS.C",
        "-o",
        "/work/bu-symbols.o",
    ]);
    tool(
        "as",
        &[
            "--64",
            "-o",
            "/work/bu-archive-main.o",
            "/host/BINUTILS/ARCHMAIN.S",
        ],
    );
    tool("ar", &["rcs", "/work/libbuprobe.a", "/work/bu-symbols.o"]);
    tool("ar", &["r", "/work/libbuprobe.a", "/work/bu-symbols.o"]);
    tool("ranlib", &["/work/libbuprobe.a"]);
    tool("ar", &["t", "/work/libbuprobe.a"]);
    tool(
        "ld",
        &[
            "-static",
            "-o",
            "/work/bu-archive-probe",
            "/work/bu-archive-main.o",
            "/work/libbuprobe.a",
        ],
    );
    assert_eq!(
        run_to_exit("/work/bu-archive-probe", &["/work/bu-archive-probe"]),
        42
    );

    zsh("nm /work/bu-symbols.o > /work/bu-nm.txt");
    zsh("strings /work/bu-symbols.o > /work/bu-strings.txt");
    zsh("c++filt _Z3foov > /work/bu-cxxfilt.txt");
    assert!(String::from_utf8_lossy(&read_file("/work/bu-nm.txt")).contains("binutils_probe_add"));
    assert!(String::from_utf8_lossy(&read_file("/work/bu-strings.txt"))
        .contains("agenticos-binutils-ok"));
    assert_eq!(
        String::from_utf8_lossy(&read_file("/work/bu-cxxfilt.txt")).trim(),
        "foo()"
    );

    crate::fs::vfs::vfs_set_times("/work/bu-symbols.o", Some(123_456), Some(234_567))
        .expect("set source dates");
    tool(
        "objcopy",
        &[
            "--preserve-dates",
            "/work/bu-symbols.o",
            "/work/bu-symbols-copy.o",
        ],
    );
    let copied = crate::fs::vfs::vfs_unix_metadata("/work/bu-symbols-copy.o")
        .expect("copied object metadata");
    assert_eq!(copied.accessed, 123_456);
    assert_eq!(copied.modified, 234_567);
    let unstripped_size = read_file("/work/bu-symbols-copy.o").len();
    tool("strip", &["--strip-debug", "/work/bu-symbols-copy.o"]);
    assert!(read_file("/work/bu-symbols-copy.o").len() < unstripped_size);
    tool(
        "elfedit",
        &["--output-osabi", "linux", "/work/bu-symbols-copy.o"],
    );
    assert_eq!(read_file("/work/bu-symbols-copy.o")[7], 3, "ELFOSABI_LINUX");

    for path in [
        "/work/bu-archive-main.o",
        "/work/bu-archive-probe",
        "/work/bu-symbols.o",
        "/work/bu-symbols-copy.o",
        "/work/libbuprobe.a",
        "/work/bu-nm.txt",
        "/work/bu-strings.txt",
        "/work/bu-cxxfilt.txt",
    ] {
        unlink_if_present(path);
    }
}

fn test_ld_more_than_fd_table_input_count() {
    for path in ["/work/bu-many-base.o", "/work/bu-many.o"] {
        unlink_if_present(path);
    }
    tool(
        "as",
        &[
            "--64",
            "-o",
            "/work/bu-many-base.o",
            "/host/BINUTILS/EXIT42.S",
        ],
    );

    let mut paths = Vec::new();
    for index in 0..36 {
        let path = alloc::format!("/work/bu-many-{}.o", index);
        copy_file("/work/bu-many-base.o", &path);
        paths.push(path);
    }
    let mut owned = alloc::vec![
        String::from("ld"),
        String::from("-r"),
        String::from("--allow-multiple-definition"),
        String::from("-o"),
        String::from("/work/bu-many.o"),
    ];
    owned.extend(paths.iter().cloned());
    let argv: Vec<&str> = owned.iter().map(String::as_str).collect();
    let code = run_to_exit("/host/LD.ELF", &argv);
    assert_eq!(code, 0, "ld failed the >32-input descriptor-cache test");

    unlink_if_present("/work/bu-many-base.o");
    unlink_if_present("/work/bu-many.o");
    for path in paths {
        unlink_if_present(&path);
    }
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_binutils_artifacts_staged,
        &test_binutils_versions_launch,
        &test_gas_ld_create_and_run_executable,
        &test_archive_and_elf_transform_tools,
        &test_ld_more_than_fd_table_input_count,
    ]
}
