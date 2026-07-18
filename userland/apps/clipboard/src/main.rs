//! PBCLIP.ELF — `pbcopy` and `pbpaste` multicall commands.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::vec;
use alloc::vec::Vec;
use runtime::{clipboard_copy, clipboard_paste, execve, exit, read, startup_from_stack, write};

const MAX_TEXT_BYTES: usize = 1024 * 1024;
// Kernel argv strings currently share PATH_MAX's bound.
const MAX_EXEC_COMMAND_BYTES: usize = 4095;

const PBCOPY_HELP: &[u8] = br#"Usage: pbcopy [OPTIONS]
Copy UTF-8 text to the host clipboard. Standard input is used by default.

Options:
  -t, --text TEXT       copy TEXT instead of reading standard input
  -a, --append          append input to the current clipboard
  -p, --prepend         prepend input to the current clipboard
      --trim            remove leading and trailing Unicode whitespace
  -n, --no-newline      remove one trailing LF (and an optional CR)
  -c, --clear           clear the host clipboard without reading input
  -v, --verbose         report the copied byte count on standard error
  -h, --help            show this help

The result must be valid UTF-8 and no larger than 1 MiB. Transformations are
applied to the new input before --append or --prepend merges it.

Examples:
  echo "hello" | pbcopy --no-newline
  pbcopy --text "hello from AgenticOS"
  printf '\nnext line' | pbcopy --append
"#;

const PBPASTE_HELP: &[u8] = br#"Usage: pbpaste [OPTIONS]
Read UTF-8 text from the host clipboard and write it to standard output.

Options:
  -l, --length          print the UTF-8 byte count
  -m, --chars           print the Unicode character count
  -L, --lines           print the logical line count
  -t, --trim            remove leading and trailing Unicode whitespace
  -q, --shell-quote     print a POSIX-shell single-quoted representation
  -n, --ensure-newline  add an LF only when the text does not end in one
  -x, --exec            execute the clipboard as `zsh -c COMMAND`
  -h, --help            show this help

The default output is byte-exact and adds no newline. --exec passes the
clipboard directly to the guest shell. Executed commands are currently limited
to 4095 UTF-8 bytes.

Examples:
  pbpaste --ensure-newline
  pbpaste --trim --shell-quote
  pbpaste --exec
"#;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PasteMode {
    Print,
    Length,
    Chars,
    Lines,
    ShellQuote,
    Exec,
}

struct CopyOptions {
    text: Option<&'static [u8]>,
    append: bool,
    prepend: bool,
    trim: bool,
    no_newline: bool,
    clear: bool,
    verbose: bool,
}

struct PasteOptions {
    mode: PasteMode,
    trim: bool,
    ensure_newline: bool,
}

#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "mov rdi, rsp",
        "and rsp, -16",
        "call {}",
        "ud2",
        sym clipboard_main,
    );
}

unsafe extern "C" fn clipboard_main(stack_top: *const u64) -> ! {
    let startup = startup_from_stack(stack_top);
    let Some(&name_pointer) = startup.argv.first() else {
        fail_code(b"clipboard: invoke as pbcopy or pbpaste\n", 2);
    };
    let name = argument_bytes(name_pointer)
        .unwrap_or_else(|| fail_code(b"clipboard: invalid argv[0]\n", 2));
    if name.ends_with(b"pbcopy") {
        copy_main(startup.argv);
    }
    if name.ends_with(b"pbpaste") {
        paste_main(startup.argv, startup.envp);
    }
    fail_code(b"clipboard: invoke as pbcopy or pbpaste\n", 2);
}

unsafe fn copy_main(argv: &[*const u8]) -> ! {
    let options = parse_copy_options(argv);
    let mut incoming = if options.clear {
        Vec::new()
    } else if let Some(text) = options.text {
        text.to_vec()
    } else {
        read_stdin()
    };

    validate_utf8(&incoming, b"pbcopy: input is not UTF-8 text\n");
    if options.trim {
        trim_text(&mut incoming);
    }
    if options.no_newline {
        remove_one_newline(&mut incoming);
    }

    let result = if options.append || options.prepend {
        let current = read_host_clipboard(b"pbcopy: host clipboard is unavailable or too large\n");
        merge_text(current, incoming, options.prepend)
    } else {
        incoming
    };

    if clipboard_copy(&result) < 0 {
        fail(b"pbcopy: host clipboard is unavailable\n");
    }
    if options.verbose {
        let message = format!("pbcopy: copied {} bytes\n", result.len());
        let _ = write_all(2, message.as_bytes());
    }
    exit(0);
}

unsafe fn paste_main(argv: &[*const u8], inherited_envp: &[*const u8]) -> ! {
    let options = parse_paste_options(argv);
    let mut text = read_host_clipboard(b"pbpaste: host clipboard is unavailable or too large\n");
    if options.trim {
        trim_text(&mut text);
    }

    match options.mode {
        PasteMode::Print => {
            if write_all(1, &text).is_err() {
                fail(b"pbpaste: could not write standard output\n");
            }
            if options.ensure_newline && !text.ends_with(b"\n") && write_all(1, b"\n").is_err() {
                fail(b"pbpaste: could not write standard output\n");
            }
            exit(0);
        }
        PasteMode::Length => write_count_and_exit(text.len()),
        PasteMode::Chars => {
            let count = core::str::from_utf8(&text).map_or(0, |value| value.chars().count());
            write_count_and_exit(count);
        }
        PasteMode::Lines => {
            let count = core::str::from_utf8(&text).map_or(0, |value| value.lines().count());
            write_count_and_exit(count);
        }
        PasteMode::ShellQuote => {
            let quoted = shell_quote(&text);
            if write_all(1, &quoted).is_err() || write_all(1, b"\n").is_err() {
                fail(b"pbpaste: could not write standard output\n");
            }
            exit(0);
        }
        PasteMode::Exec => exec_clipboard(text, inherited_envp),
    }
}

unsafe fn parse_copy_options(argv: &[*const u8]) -> CopyOptions {
    let mut options = CopyOptions {
        text: None,
        append: false,
        prepend: false,
        trim: false,
        no_newline: false,
        clear: false,
        verbose: false,
    };
    let mut index = 1usize;
    while index < argv.len() {
        let argument = argument_bytes(argv[index])
            .unwrap_or_else(|| fail_code(b"pbcopy: invalid argument\n", 2));
        match argument {
            b"-h" | b"--help" => print_help_and_exit(PBCOPY_HELP),
            b"-a" | b"--append" => options.append = true,
            b"-p" | b"--prepend" => options.prepend = true,
            b"--trim" => options.trim = true,
            b"-n" | b"--no-newline" => options.no_newline = true,
            b"-c" | b"--clear" => options.clear = true,
            b"-v" | b"--verbose" => options.verbose = true,
            b"-t" | b"--text" => {
                index += 1;
                if index >= argv.len() {
                    usage_fail(b"pbcopy", b"--text requires an argument");
                }
                if options.text.is_some() {
                    usage_fail(b"pbcopy", b"--text may only be specified once");
                }
                options.text = argument_bytes(argv[index]);
                if options.text.is_none() {
                    usage_fail(b"pbcopy", b"invalid --text argument");
                }
            }
            _ if argument.starts_with(b"--text=") => {
                if options.text.is_some() {
                    usage_fail(b"pbcopy", b"--text may only be specified once");
                }
                options.text = Some(&argument[b"--text=".len()..]);
            }
            _ => unknown_option(b"pbcopy", argument),
        }
        index += 1;
    }

    if options.append && options.prepend {
        usage_fail(b"pbcopy", b"--append and --prepend are mutually exclusive");
    }
    if options.clear && (options.text.is_some() || options.append || options.prepend) {
        usage_fail(
            b"pbcopy",
            b"--clear cannot be combined with --text, --append, or --prepend",
        );
    }
    options
}

unsafe fn parse_paste_options(argv: &[*const u8]) -> PasteOptions {
    let mut options = PasteOptions {
        mode: PasteMode::Print,
        trim: false,
        ensure_newline: false,
    };
    for &pointer in argv.iter().skip(1) {
        let argument =
            argument_bytes(pointer).unwrap_or_else(|| fail_code(b"pbpaste: invalid argument\n", 2));
        match argument {
            b"-h" | b"--help" => print_help_and_exit(PBPASTE_HELP),
            b"-l" | b"--length" => set_paste_mode(&mut options, PasteMode::Length),
            b"-m" | b"--chars" => set_paste_mode(&mut options, PasteMode::Chars),
            b"-L" | b"--lines" => set_paste_mode(&mut options, PasteMode::Lines),
            b"-t" | b"--trim" => options.trim = true,
            b"-q" | b"--shell-quote" => set_paste_mode(&mut options, PasteMode::ShellQuote),
            b"-n" | b"--ensure-newline" => options.ensure_newline = true,
            b"-x" | b"--exec" => set_paste_mode(&mut options, PasteMode::Exec),
            _ => unknown_option(b"pbpaste", argument),
        }
    }
    if options.ensure_newline && options.mode != PasteMode::Print {
        usage_fail(
            b"pbpaste",
            b"--ensure-newline only applies to normal clipboard output",
        );
    }
    options
}

fn set_paste_mode(options: &mut PasteOptions, mode: PasteMode) {
    if options.mode != PasteMode::Print {
        unsafe { usage_fail(b"pbpaste", b"output modes are mutually exclusive") }
    }
    options.mode = mode;
}

fn read_stdin() -> Vec<u8> {
    let mut target = vec![0u8; MAX_TEXT_BYTES];
    let mut used = 0usize;
    while used < target.len() {
        let count = read(0, &mut target[used..]);
        if count < 0 {
            unsafe { fail(b"pbcopy: could not read standard input\n") }
        }
        if count == 0 {
            break;
        }
        used += count as usize;
    }
    if used == target.len() {
        let mut extra = [0u8; 1];
        if read(0, &mut extra) != 0 {
            unsafe { fail(b"pbcopy: text exceeds the 1 MiB limit\n") }
        }
    }
    target.truncate(used);
    target
}

fn read_host_clipboard(error_message: &[u8]) -> Vec<u8> {
    let mut target = vec![0u8; MAX_TEXT_BYTES];
    let count = clipboard_paste(&mut target);
    if count < 0 {
        unsafe { fail(error_message) }
    }
    target.truncate(count as usize);
    validate_utf8(
        &target,
        b"pbpaste: host clipboard does not contain UTF-8 text\n",
    );
    target
}

fn validate_utf8(text: &[u8], error_message: &[u8]) {
    if core::str::from_utf8(text).is_err() {
        unsafe { fail(error_message) }
    }
}

fn trim_text(text: &mut Vec<u8>) {
    let (start, length) = {
        let value = core::str::from_utf8(text).expect("clipboard text was validated as UTF-8");
        let trimmed = value.trim().as_bytes();
        (
            trimmed.as_ptr() as usize - text.as_ptr() as usize,
            trimmed.len(),
        )
    };
    text.copy_within(start..start + length, 0);
    text.truncate(length);
}

fn remove_one_newline(text: &mut Vec<u8>) {
    if text.last() == Some(&b'\n') {
        text.pop();
        if text.last() == Some(&b'\r') {
            text.pop();
        }
    }
}

fn merge_text(current: Vec<u8>, incoming: Vec<u8>, prepend: bool) -> Vec<u8> {
    let Some(total) = current.len().checked_add(incoming.len()) else {
        unsafe { fail(b"pbcopy: merged text exceeds the 1 MiB limit\n") }
    };
    if total > MAX_TEXT_BYTES {
        unsafe { fail(b"pbcopy: merged text exceeds the 1 MiB limit\n") }
    }
    let mut merged = Vec::with_capacity(total);
    if prepend {
        merged.extend_from_slice(&incoming);
        merged.extend_from_slice(&current);
    } else {
        merged.extend_from_slice(&current);
        merged.extend_from_slice(&incoming);
    }
    merged
}

fn shell_quote(text: &[u8]) -> Vec<u8> {
    let extra = text.iter().filter(|&&byte| byte == b'\'').count() * 3;
    let mut quoted = Vec::with_capacity(text.len().saturating_add(extra).saturating_add(2));
    quoted.push(b'\'');
    for &byte in text {
        if byte == b'\'' {
            quoted.extend_from_slice(b"'\\''");
        } else {
            quoted.push(byte);
        }
    }
    quoted.push(b'\'');
    quoted
}

unsafe fn exec_clipboard(mut command: Vec<u8>, inherited_envp: &[*const u8]) -> ! {
    if command.len() > MAX_EXEC_COMMAND_BYTES {
        fail(b"pbpaste: --exec command exceeds the 4095-byte argv limit\n");
    }
    if command.contains(&0) {
        fail(b"pbpaste: --exec command contains a NUL byte\n");
    }
    command.push(0);
    let argv = [
        b"zsh\0".as_ptr(),
        b"-c\0".as_ptr(),
        command.as_ptr(),
        core::ptr::null(),
    ];
    let mut envp = inherited_envp.to_vec();
    envp.push(core::ptr::null());
    let result = execve(b"/host/ZSH.ELF\0", &argv, &envp);
    if result < 0 {
        fail(b"pbpaste: could not execute clipboard through zsh\n");
    }
    exit(0);
}

unsafe fn argument_bytes(pointer: *const u8) -> Option<&'static [u8]> {
    if pointer.is_null() {
        return None;
    }
    let mut length = 0usize;
    while length < MAX_TEXT_BYTES && core::ptr::read(pointer.add(length)) != 0 {
        length += 1;
    }
    (length < MAX_TEXT_BYTES).then(|| core::slice::from_raw_parts(pointer, length))
}

fn write_count_and_exit(count: usize) -> ! {
    let output = format!("{}\n", count);
    if write_all(1, output.as_bytes()).is_err() {
        unsafe { fail(b"pbpaste: could not write standard output\n") }
    }
    unsafe { exit(0) }
}

fn write_all(fd: i32, mut bytes: &[u8]) -> Result<(), ()> {
    while !bytes.is_empty() {
        let count = write(fd, bytes);
        if count <= 0 {
            return Err(());
        }
        bytes = &bytes[count as usize..];
    }
    Ok(())
}

unsafe fn print_help_and_exit(help: &[u8]) -> ! {
    if write_all(1, help).is_err() {
        exit(1);
    }
    exit(0);
}

unsafe fn unknown_option(command: &[u8], option: &[u8]) -> ! {
    let _ = write_all(2, command);
    let _ = write_all(2, b": unknown option: ");
    let _ = write_all(2, option);
    let _ = write_all(2, b"\nTry '");
    let _ = write_all(2, command);
    let _ = write_all(2, b" --help' for usage.\n");
    exit(2);
}

unsafe fn usage_fail(command: &[u8], message: &[u8]) -> ! {
    let _ = write_all(2, command);
    let _ = write_all(2, b": ");
    let _ = write_all(2, message);
    let _ = write_all(2, b"\nTry '");
    let _ = write_all(2, command);
    let _ = write_all(2, b" --help' for usage.\n");
    exit(2);
}

unsafe fn fail(message: &[u8]) -> ! {
    fail_code(message, 1)
}

unsafe fn fail_code(message: &[u8], code: i64) -> ! {
    let _ = write_all(2, message);
    exit(code)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { exit(127) }
}
