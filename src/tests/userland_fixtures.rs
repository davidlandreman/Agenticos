#![allow(dead_code)]
//! Hand-rolled ELF64 fixtures for the U6 loader tests.
//!
//! We build minimal `Vec<u8>` ELF binaries directly here rather than
//! `include_bytes!` from the userland sibling project (U8). The plan's
//! F2 finding flags U8 as a soft circular dependency for U6 tests, and the
//! happy-path is small enough that hand-rolling keeps the loader honest.
//!
//! Layout (ET_EXEC, EM_X86_64, ELFCLASS64):
//!
//! ```
//! +------------- 0
//! | Elf64Ehdr (64 bytes)
//! +------------- 64
//! | Elf64Phdr * phnum (56 bytes each)
//! +------------- after phdrs
//! | (optional) section headers + dynsym/strtab/rela
//! +------------- p_offset
//! | PT_LOAD payload (instructions, data, bss tail)
//! +-------------
//! ```
//!
//! `p_offset == p_vaddr` so the alignment invariant `p_offset % 0x1000 ==
//! p_vaddr % 0x1000` holds for any page-aligned `p_vaddr`. We keep
//! `p_offset` small (just past the headers) and the segment payload short;
//! the page mapping covers exactly the bytes used.

use alloc::vec;
use alloc::vec::Vec;

pub const EI_NIDENT: usize = 16;
pub const EHDR_SIZE: u64 = 64;
pub const PHDR_SIZE: u64 = 56;
pub const SHDR_SIZE: u64 = 64;
pub const RELA_SIZE: u64 = 24;
pub const SYM_SIZE: u64 = 24;

pub const ELFMAG: [u8; 4] = [0x7F, b'E', b'L', b'F'];
pub const ELFCLASS64: u8 = 2;
pub const ELFDATA2LSB: u8 = 1;
pub const EV_CURRENT: u8 = 1;

pub const ET_REL: u16 = 1;
pub const ET_EXEC: u16 = 2;
pub const EM_X86_64: u16 = 62;
pub const EM_AARCH64: u16 = 183;

pub const PT_LOAD: u32 = 1;
pub const PT_INTERP: u32 = 3;
pub const PT_TLS: u32 = 7;

pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

pub const SHT_PROGBITS: u32 = 1;
pub const SHT_RELA: u32 = 4;
pub const SHT_DYNSYM: u32 = 11;
pub const SHT_STRTAB: u32 = 3;

pub const R_X86_64_GLOB_DAT: u32 = 6;
pub const R_X86_64_TPOFF64: u32 = 18;

#[derive(Clone, Copy)]
pub struct PhdrSpec {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

/// Settings for the happy-path (and most negative-path) ELF.
pub struct Fixture {
    pub e_type: u16,
    pub e_machine: u16,
    pub ei_class: u8,
    pub ei_data: u8,
    pub e_entry: u64,
    pub phdrs: Vec<PhdrSpec>,
    /// File-resident bytes per PT_LOAD. The loader copies `p_filesz` bytes
    /// from `bytes[p_offset..]` into the user VA. We append each PT_LOAD's
    /// payload at its `p_offset`. Zero-padding between regions is fine.
    pub payloads: Vec<(u64 /* p_offset */, Vec<u8>)>,
    /// Truncate the assembled output at this length. None = no truncation.
    pub truncate_to: Option<usize>,
}

impl Fixture {
    pub fn build(self) -> Vec<u8> {
        let phnum = self.phdrs.len() as u16;

        // Compute total length: header + phdrs + every payload range.
        let mut total = EHDR_SIZE + phnum as u64 * PHDR_SIZE;
        for (off, payload) in &self.payloads {
            let end = off + payload.len() as u64;
            if end > total {
                total = end;
            }
        }

        let mut out: Vec<u8> = vec![0u8; total as usize];

        // Ehdr.
        out[0..4].copy_from_slice(&ELFMAG);
        out[4] = self.ei_class;
        out[5] = self.ei_data;
        out[6] = EV_CURRENT;
        // e_ident[7..16] left zero (OSABI=0 etc).
        write_u16(&mut out, 16, self.e_type);
        write_u16(&mut out, 18, self.e_machine);
        write_u32(&mut out, 20, EV_CURRENT as u32);
        write_u64(&mut out, 24, self.e_entry);
        write_u64(&mut out, 32, EHDR_SIZE); // e_phoff
        write_u64(&mut out, 40, 0); // e_shoff
        write_u32(&mut out, 48, 0); // e_flags
        write_u16(&mut out, 52, EHDR_SIZE as u16);
        write_u16(&mut out, 54, PHDR_SIZE as u16);
        write_u16(&mut out, 56, phnum);
        write_u16(&mut out, 58, SHDR_SIZE as u16);
        write_u16(&mut out, 60, 0); // e_shnum
        write_u16(&mut out, 62, 0); // e_shstrndx

        // Phdrs.
        for (i, ph) in self.phdrs.iter().enumerate() {
            let off = (EHDR_SIZE + i as u64 * PHDR_SIZE) as usize;
            write_u32(&mut out, off, ph.p_type);
            write_u32(&mut out, off + 4, ph.p_flags);
            write_u64(&mut out, off + 8, ph.p_offset);
            write_u64(&mut out, off + 16, ph.p_vaddr);
            write_u64(&mut out, off + 24, ph.p_vaddr); // p_paddr = p_vaddr
            write_u64(&mut out, off + 32, ph.p_filesz);
            write_u64(&mut out, off + 40, ph.p_memsz);
            write_u64(&mut out, off + 48, ph.p_align);
        }

        // Payloads.
        for (offset, payload) in &self.payloads {
            let off = *offset as usize;
            out[off..off + payload.len()].copy_from_slice(payload);
        }

        if let Some(n) = self.truncate_to {
            out.truncate(n);
        }
        out
    }
}

/// Smallest valid happy-path ELF: one PT_LOAD covering a single page at
/// `0x40_0000` (USER_LOAD_BASE) with a tiny RX payload. e_entry points to
/// the start of the segment. `p_filesz < p_memsz` so the loader's bss
/// zero-fill code is exercised.
pub fn happy_path_elf() -> Vec<u8> {
    // Choose p_offset = 0x1000 so the file layout is:
    //   [0..64) ehdr
    //   [64..120) phdr
    //   [0x1000..0x1000+0x10) payload (16 bytes of "code")
    let payload: Vec<u8> = (0..16u8).collect();
    let p_offset = 0x1000u64;
    let p_vaddr = 0x40_0000u64;
    let p_filesz = payload.len() as u64;
    let p_memsz = 0x100u64; // bss tail of (0x100 - 0x10) zero bytes
    let phdr = PhdrSpec {
        p_type: PT_LOAD,
        p_flags: PF_R | PF_X,
        p_offset,
        p_vaddr,
        p_filesz,
        p_memsz,
        p_align: 0x1000,
    };
    Fixture {
        e_type: ET_EXEC,
        e_machine: EM_X86_64,
        ei_class: ELFCLASS64,
        ei_data: ELFDATA2LSB,
        e_entry: p_vaddr,
        phdrs: vec![phdr],
        payloads: vec![(p_offset, payload)],
        truncate_to: None,
    }
    .build()
}

// ---------- runnable fixtures (U7) ----------

/// Build a single-PT_LOAD ELF whose `_start` (at 0x40_0000) is the given byte
/// stream. The page is mapped R+X (PF_R | PF_X). `p_memsz` is rounded up to a
/// page so the loader maps exactly one 4 KiB page covering `p_vaddr ..
/// p_vaddr+0x1000`. Useful for hand-crafted "do one thing" test apps.
pub fn runnable_elf_rx(code: &[u8]) -> Vec<u8> {
    let payload = code.to_vec();
    let p_offset = 0x1000u64;
    let p_vaddr = 0x40_0000u64;
    let p_filesz = payload.len() as u64;
    let p_memsz = 0x1000u64; // full page
    let phdr = PhdrSpec {
        p_type: PT_LOAD,
        p_flags: PF_R | PF_X,
        p_offset,
        p_vaddr,
        p_filesz,
        p_memsz,
        p_align: 0x1000,
    };
    Fixture {
        e_type: ET_EXEC,
        e_machine: EM_X86_64,
        ei_class: ELFCLASS64,
        ei_data: ELFDATA2LSB,
        e_entry: p_vaddr,
        phdrs: vec![phdr],
        payloads: vec![(p_offset, payload)],
        truncate_to: None,
    }
    .build()
}

/// Hand-assembled "write(stdout, msg) + exit_group(0)" app using the
/// Linux x86-64 ABI (`syscall` instruction, Linux syscall numbers).
///
/// Layout in the single R+X page:
///
/// ```
/// 0x40_0000:  mov eax, 1            ; B8 01 00 00 00          (write)
/// 0x40_0005:  mov edi, 1            ; BF 01 00 00 00          (fd = stdout)
/// 0x40_000A:  mov rsi, 0x400080     ; 48 BE 80 00 40 00 00 00 00 00  (buf)
/// 0x40_0014:  mov edx, len          ; BA <len32>              (count)
/// 0x40_0019:  syscall               ; 0F 05
/// 0x40_001B:  mov eax, 231          ; B8 E7 00 00 00          (exit_group)
/// 0x40_0020:  mov edi, exit_code    ; BF <code32>
/// 0x40_0025:  syscall               ; 0F 05
/// 0x40_0027:  hlt                   ; F4                      (safety)
/// ...
/// 0x40_0080:  "hello\n"
/// ```
///
/// Issuing `syscall` directly with the Linux number baked into EAX lets
/// us exercise the full SYSCALL fast path (entry stub, dispatcher, IRETQ)
/// without depending on relocations or any linker tooling.
pub fn hello_exit0_elf() -> Vec<u8> {
    build_print_exit_elf(b"hello\n", 0)
}

/// Same shape as `hello_exit0_elf` but with a custom message and exit code.
pub fn build_print_exit_elf(msg: &[u8], exit_code: u32) -> Vec<u8> {
    let mut code: Vec<u8> = Vec::new();
    // mov eax, 1   (NR_WRITE)
    code.extend_from_slice(&[0xB8, 0x01, 0x00, 0x00, 0x00]);
    // mov edi, 1   (fd = stdout)
    code.extend_from_slice(&[0xBF, 0x01, 0x00, 0x00, 0x00]);
    // mov rsi, 0x40_0080  (buf addr)  — REX.W mov + 0xBE+rd
    code.extend_from_slice(&[0x48, 0xBE, 0x80, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00]);
    // mov edx, msg.len()  (count)
    let len = msg.len() as u32;
    code.push(0xBA);
    code.extend_from_slice(&len.to_le_bytes());
    // syscall      (0F 05)
    code.extend_from_slice(&[0x0F, 0x05]);
    // mov eax, 231 (NR_EXIT_GROUP)
    code.extend_from_slice(&[0xB8, 0xE7, 0x00, 0x00, 0x00]);
    // mov edi, exit_code
    code.push(0xBF);
    code.extend_from_slice(&exit_code.to_le_bytes());
    // syscall
    code.extend_from_slice(&[0x0F, 0x05]);
    // hlt (safety; reached only if exit_group returns, which it does not)
    code.push(0xF4);

    // Pad to 0x80 then append message.
    while code.len() < 0x80 {
        code.push(0x90);
    }
    code.extend_from_slice(msg);
    runnable_elf_rx(&code)
}

/// Fixture D — unhandled-syscall trap.
///
/// Issues `syscall RAX=999`. The kernel dispatcher's default arm should
/// log the number and terminate the process via the existing fault-cleanup
/// path, recording `ExitKind::UnimplementedSyscall { nr: 999 }`. No
/// kernel panic, no hang, no silent `-ENOSYS` return.
pub fn syscall_999_elf() -> Vec<u8> {
    let mut code: Vec<u8> = Vec::new();
    // mov eax, 999
    code.extend_from_slice(&[0xB8, 0xE7, 0x03, 0x00, 0x00]);
    // syscall
    code.extend_from_slice(&[0x0F, 0x05]);
    // hlt — only reached if the dispatcher returned cleanly, which would
    // be a bug.
    code.push(0xF4);
    runnable_elf_rx(&code)
}

/// Fixture A — the Risks-table mandated minimal SYSCALL transition smoke test.
///
/// Single instruction: `syscall` with `RAX=231 (exit_group), RDI=42`. Verifies
/// the SYSCALL fast path end-to-end (entry stub stack switch, dispatcher,
/// long-jump-via-cooperative_exit) by recording exit code 42 in
/// `LAST_EXIT_CODE`. This is the smallest possible live test; if it passes,
/// the rest of the syscall surface can be debugged on solid foundations.
pub fn syscall_exit42_elf() -> Vec<u8> {
    let mut code: Vec<u8> = Vec::new();
    // mov eax, 231  (NR_EXIT_GROUP)
    code.extend_from_slice(&[0xB8, 0xE7, 0x00, 0x00, 0x00]);
    // mov edi, 42
    code.extend_from_slice(&[0xBF, 0x2A, 0x00, 0x00, 0x00]);
    // syscall
    code.extend_from_slice(&[0x0F, 0x05]);
    // hlt (safety)
    code.push(0xF4);
    runnable_elf_rx(&code)
}

/// Fixture B — Linux initial-stack contract.
///
/// Verifies the kernel built `argc/argv/envp/auxv` correctly. The binary:
///
/// 1. Reads `argc` from `[rsp]` — must be 1.
/// 2. Reads `argv[0]` from `[rsp+8]` — must be non-NULL.
/// 3. Reads `argv[1]` from `[rsp+16]` — must be NULL (argv terminator).
/// 4. Reads `envp[0]` from `[rsp+24]` — must be NULL.
/// 5. Walks the auxv (starting at `[rsp+32]`) looking for `AT_RANDOM`.
///    Verifies the auxv terminates at `AT_NULL` and `AT_RANDOM` is
///    present with a non-zero value pointer.
/// 6. Calls `exit_group(0)` if all checks pass; `exit_group(1..6)` for
///    the specific check that failed.
///
/// Hand-assembled x86-64 to keep the test independent of a host
/// toolchain.
pub fn auxv_walker_elf() -> Vec<u8> {
    // Linux x86-64 auxv constants.
    const AT_NULL: u8 = 0;
    const AT_RANDOM: u8 = 25;

    // We hand-assemble a small program. Using rax/rcx/rdi/rsi as scratch.
    // System V calling convention is irrelevant here — only `_start`,
    // which receives no args by ABI.
    let mut code: Vec<u8> = Vec::new();

    // 0: cmp qword ptr [rsp], 1     ; argc == 1?
    code.extend_from_slice(&[0x48, 0x83, 0x3C, 0x24, 0x01]);
    // 5: jne fail_argc                ; -> mov edi,1; exit_group
    // We'll resolve forward jumps by patching after we know offsets.
    // Use placeholder 0xCC so we can spot un-patched jumps.
    let mut patches: Vec<(usize, usize)> = Vec::new(); // (instr_addr, target_label_id)

    // jne rel8 — opcode 75 imm8.
    code.extend_from_slice(&[0x75, 0x00]); // patched to fail_argc offset
    let jne_argc_at = code.len() - 1;

    // 7: mov rcx, [rsp + 8]            ; argv[0]
    code.extend_from_slice(&[0x48, 0x8B, 0x4C, 0x24, 0x08]);
    // 12: test rcx, rcx
    code.extend_from_slice(&[0x48, 0x85, 0xC9]);
    // 15: je fail_argv0
    code.extend_from_slice(&[0x74, 0x00]);
    let je_argv0_at = code.len() - 1;

    // 17: cmp qword ptr [rsp + 16], 0  ; argv[1] == NULL?
    code.extend_from_slice(&[0x48, 0x83, 0x7C, 0x24, 0x10, 0x00]);
    // 23: jne fail_argv1
    code.extend_from_slice(&[0x75, 0x00]);
    let jne_argv1_at = code.len() - 1;

    // 25: cmp qword ptr [rsp + 24], 0  ; envp[0] == NULL?
    code.extend_from_slice(&[0x48, 0x83, 0x7C, 0x24, 0x18, 0x00]);
    // 31: jne fail_envp0
    code.extend_from_slice(&[0x75, 0x00]);
    let jne_envp0_at = code.len() - 1;

    // Walk auxv at [rsp+32]. Use rsi as cursor.
    // 33: lea rsi, [rsp + 32]
    code.extend_from_slice(&[0x48, 0x8D, 0x74, 0x24, 0x20]);

    // walk_loop:
    let walk_loop_target = code.len();
    // 38: mov rax, [rsi]              ; a_type
    code.extend_from_slice(&[0x48, 0x8B, 0x06]);
    // 41: cmp rax, AT_NULL
    code.extend_from_slice(&[0x48, 0x83, 0xF8, AT_NULL]);
    // 45: je fail_no_random           ; reached terminator without finding AT_RANDOM
    code.extend_from_slice(&[0x74, 0x00]);
    let je_no_random_at = code.len() - 1;
    // 47: cmp rax, AT_RANDOM
    code.extend_from_slice(&[0x48, 0x83, 0xF8, AT_RANDOM]);
    // 51: je found_random
    code.extend_from_slice(&[0x74, 0x00]);
    let je_found_random_at = code.len() - 1;
    // 53: add rsi, 16
    code.extend_from_slice(&[0x48, 0x83, 0xC6, 0x10]);
    // 57: jmp walk_loop  (rel8)
    let jmp_back_at = code.len() + 1;
    let walk_back_offset = walk_loop_target as i32 - (jmp_back_at + 1) as i32;
    code.extend_from_slice(&[0xEB, walk_back_offset as i8 as u8]);

    // found_random:
    let found_random_target = code.len();
    // mov rcx, [rsi + 8]              ; AT_RANDOM value (ptr)
    code.extend_from_slice(&[0x48, 0x8B, 0x4E, 0x08]);
    // test rcx, rcx
    code.extend_from_slice(&[0x48, 0x85, 0xC9]);
    // je fail_random_null
    code.extend_from_slice(&[0x74, 0x00]);
    let je_random_null_at = code.len() - 1;

    // success: mov edi, 0; mov eax, 231 (NR_EXIT_GROUP); syscall
    code.extend_from_slice(&[0xBF, 0x00, 0x00, 0x00, 0x00]);
    code.extend_from_slice(&[0xB8, 0xE7, 0x00, 0x00, 0x00]);
    code.extend_from_slice(&[0x0F, 0x05]);
    code.push(0xF4);

    // Failure exits — each one sets edi to its code, then exit_group.
    // Fail codes: argc=1, argv0=2, argv1=3, envp0=4, no_random=5, random_null=6.
    let exit_block = |exit_code: u8, code: &mut Vec<u8>| -> usize {
        let target = code.len();
        code.extend_from_slice(&[0xBF, exit_code, 0x00, 0x00, 0x00]); // mov edi, code
        code.extend_from_slice(&[0xB8, 0xE7, 0x00, 0x00, 0x00]);       // mov eax, 231
        code.extend_from_slice(&[0x0F, 0x05]);                          // syscall
        code.push(0xF4);                                                // hlt safety
        target
    };

    let fail_argc = exit_block(1, &mut code);
    let fail_argv0 = exit_block(2, &mut code);
    let fail_argv1 = exit_block(3, &mut code);
    let fail_envp0 = exit_block(4, &mut code);
    let fail_no_random = exit_block(5, &mut code);
    let fail_random_null = exit_block(6, &mut code);

    // Patch all the rel8 forward jumps.
    let patch = |code: &mut Vec<u8>, instr_at: usize, target: usize, patches: &mut Vec<(usize, usize)>| {
        let next = instr_at + 1;
        let off = target as i32 - next as i32;
        assert!(off >= -128 && off <= 127, "rel8 out of range: {}", off);
        code[instr_at] = off as i8 as u8;
        patches.push((instr_at, target));
    };
    patch(&mut code, jne_argc_at, fail_argc, &mut patches);
    patch(&mut code, je_argv0_at, fail_argv0, &mut patches);
    patch(&mut code, jne_argv1_at, fail_argv1, &mut patches);
    patch(&mut code, jne_envp0_at, fail_envp0, &mut patches);
    patch(&mut code, je_no_random_at, fail_no_random, &mut patches);
    patch(&mut code, je_found_random_at, found_random_target, &mut patches);
    patch(&mut code, je_random_null_at, fail_random_null, &mut patches);

    runnable_elf_rx(&code)
}

/// Fixture C — PT_TLS smoke test.
///
/// One PT_LOAD with `syscall RAX=231 (exit_group), RDI=0` plus one PT_TLS
/// segment carrying 4 bytes of `0x55` tdata followed by zero tbss padding.
/// The binary itself doesn't access TLS — Fixture C only verifies that the
/// loader accepts PT_TLS, copies tdata correctly, and initializes the TCB
/// self-pointer. End-to-end TLS access via `arch_prctl(ARCH_SET_FS)` lands
/// in U9.
pub fn tls_smoke_elf() -> Vec<u8> {
    let mut code: Vec<u8> = Vec::new();
    // mov eax, 231  (NR_EXIT_GROUP)
    code.extend_from_slice(&[0xB8, 0xE7, 0x00, 0x00, 0x00]);
    // mov edi, 0
    code.extend_from_slice(&[0xBF, 0x00, 0x00, 0x00, 0x00]);
    // syscall
    code.extend_from_slice(&[0x0F, 0x05]);
    code.push(0xF4); // hlt safety

    // Fixture builder layout:
    //   p_offset 0x1000: PT_LOAD payload (the code above, padded to a page)
    //   p_offset 0x2000: PT_TLS payload (4 bytes of 0x55 + zero tbss tail)
    let mut load_payload = code;
    while load_payload.len() < 0x100 {
        load_payload.push(0x90);
    }
    let tls_payload = vec![0x55u8, 0x55, 0x55, 0x55];

    let phdr_load = PhdrSpec {
        p_type: PT_LOAD,
        p_flags: PF_R | PF_X,
        p_offset: 0x1000,
        p_vaddr: 0x40_0000,
        p_filesz: load_payload.len() as u64,
        p_memsz: 0x1000,
        p_align: 0x1000,
    };
    let phdr_tls = PhdrSpec {
        p_type: PT_TLS,
        p_flags: PF_R,
        p_offset: 0x2000,
        p_vaddr: 0,
        p_filesz: tls_payload.len() as u64,
        // 0x100 bytes total = 4 bytes of tdata + 252 bytes of tbss.
        p_memsz: 0x100,
        // Word alignment is enough for this fixture.
        p_align: 8,
    };

    Fixture {
        e_type: ET_EXEC,
        e_machine: EM_X86_64,
        ei_class: ELFCLASS64,
        ei_data: ELFDATA2LSB,
        e_entry: 0x40_0000,
        phdrs: vec![phdr_load, phdr_tls],
        payloads: vec![(0x1000, load_payload), (0x2000, tls_payload)],
        truncate_to: None,
    }
    .build()
}

/// Fixture: first instruction is UD2 (`0F 0B`). Triggers #UD on entry.
pub fn fault_ud_elf() -> Vec<u8> {
    runnable_elf_rx(&[0x0F, 0x0B])
}

/// Fixture: dereferences `0x10_0000_0000` (canonical, unmapped) for a read.
/// Triggers #PF (a non-canonical address would trigger #GP instead).
pub fn fault_pf_elf() -> Vec<u8> {
    let mut code: Vec<u8> = Vec::new();
    // mov rax, 0x10_0000_0000 ; 48 B8 imm64
    code.extend_from_slice(&[0x48, 0xB8]);
    code.extend_from_slice(&0x10_0000_0000u64.to_le_bytes());
    // mov rax, [rax]            ; 48 8B 00
    code.extend_from_slice(&[0x48, 0x8B, 0x00]);
    // hlt (unreachable — should fault first)
    code.push(0xF4);
    runnable_elf_rx(&code)
}

/// Fixture: executes `cli` (0xFA), a privileged instruction. Triggers #GP.
pub fn fault_gp_elf() -> Vec<u8> {
    runnable_elf_rx(&[0xFA, 0xF4])
}

/// Fixture: calls `write(stdout, kernel_ptr, 5)`. The syscall returns
/// EFAULT (negative); the app then does `exit_group(rax)`. Demonstrates
/// pointer-validation defense without crashing the kernel.
pub fn print_kernel_ptr_then_exit_elf() -> Vec<u8> {
    let mut code: Vec<u8> = Vec::new();
    // mov eax, 1   (NR_WRITE)
    code.extend_from_slice(&[0xB8, 0x01, 0x00, 0x00, 0x00]);
    // mov edi, 1   (fd = stdout)
    code.extend_from_slice(&[0xBF, 0x01, 0x00, 0x00, 0x00]);
    // mov rsi, 0xFFFF_8000_0000_0000  (kernel-range buf)
    code.extend_from_slice(&[0x48, 0xBE]);
    code.extend_from_slice(&0xFFFF_8000_0000_0000u64.to_le_bytes());
    // mov edx, 5  (count)
    code.extend_from_slice(&[0xBA, 0x05, 0x00, 0x00, 0x00]);
    // syscall (returns -EFAULT in RAX)
    code.extend_from_slice(&[0x0F, 0x05]);
    // mov rdi, rax  (exit code = whatever write returned)
    code.extend_from_slice(&[0x48, 0x89, 0xC7]);
    // mov eax, 231  (NR_EXIT_GROUP)
    code.extend_from_slice(&[0xB8, 0xE7, 0x00, 0x00, 0x00]);
    // syscall
    code.extend_from_slice(&[0x0F, 0x05]);
    code.push(0xF4);
    runnable_elf_rx(&code)
}

// ---------- low-level writers ----------

pub fn write_u16(buf: &mut [u8], at: usize, v: u16) {
    buf[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
pub fn write_u32(buf: &mut [u8], at: usize, v: u32) {
    buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
pub fn write_u64(buf: &mut [u8], at: usize, v: u64) {
    buf[at..at + 8].copy_from_slice(&v.to_le_bytes());
}
pub fn write_i64(buf: &mut [u8], at: usize, v: i64) {
    buf[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

// ---------- ELF with section headers + relocations ----------

/// Build an ELF that includes section headers, a dynsym, a strtab, and one
/// SHT_RELA section with a single Rela entry of the requested type referencing
/// the requested symbol name. `rela_offset` is the `r_offset` field of the
/// rela; for the happy "patch a GOT slot" case point it inside the writable
/// segment; for the BadRelocOffset test point it outside.
///
/// The binary has two PT_LOADs: an R-X "text" page at 0x40_0000 and an R-W
/// "data" page at 0x40_1000 where the GOT slot lives (so the relocation walk
/// has somewhere legal to write).
pub fn elf_with_one_reloc(
    sym_name: &str,
    rela_type: u32,
    rela_offset: u64,
) -> Vec<u8> {
    // Layout (file offsets):
    //   ehdr 0
    //   phdr 64
    //   phdr 64+56=120
    //   shdrs 0x800       (4 entries: NULL, .dynsym, .dynstr, .rela.dyn)
    //   dynsym 0xA00      (2 entries: STN_UNDEF, our import)
    //   dynstr 0xB00      ("\0<name>\0")
    //   rela 0xC00        (1 entry)
    //   text payload 0x1000 (16 bytes at p_vaddr=0x40_0000)
    //   data payload 0x2000 (8 bytes at p_vaddr=0x40_1000 — the GOT slot)
    //
    // Sizes are generous so we never have to reflow.

    let text_payload: Vec<u8> = (0..16u8).collect();
    let data_payload: Vec<u8> = vec![0u8; 8];

    let dynsym_off: u64 = 0xA00;
    let dynstr_off: u64 = 0xB00;
    let rela_off: u64 = 0xC00;
    let shoff: u64 = 0x800;

    let text_p_offset = 0x1000u64;
    let text_p_vaddr = 0x40_0000u64;
    let data_p_offset = 0x2000u64;
    let data_p_vaddr = 0x40_1000u64;

    let phdrs = vec![
        PhdrSpec {
            p_type: PT_LOAD,
            p_flags: PF_R | PF_X,
            p_offset: text_p_offset,
            p_vaddr: text_p_vaddr,
            p_filesz: text_payload.len() as u64,
            p_memsz: 0x100,
            p_align: 0x1000,
        },
        PhdrSpec {
            p_type: PT_LOAD,
            p_flags: PF_R | PF_W,
            p_offset: data_p_offset,
            p_vaddr: data_p_vaddr,
            p_filesz: data_payload.len() as u64,
            p_memsz: 0x100,
            p_align: 0x1000,
        },
    ];

    // Build dynstr: starts with "\0" then the symbol name + "\0".
    let mut dynstr = Vec::new();
    dynstr.push(0u8); // STN_UNDEF
    let name_off = dynstr.len() as u32;
    dynstr.extend_from_slice(sym_name.as_bytes());
    dynstr.push(0);

    // Build dynsym (2 entries: undef + the import).
    let mut dynsym = Vec::new();
    // Entry 0: STN_UNDEF, all zeros.
    dynsym.extend_from_slice(&[0u8; SYM_SIZE as usize]);
    // Entry 1: st_name = name_off, st_info=0x10 (STB_GLOBAL, STT_NOTYPE),
    // st_shndx=0 (SHN_UNDEF), st_value=0, st_size=0.
    let mut sym1 = vec![0u8; SYM_SIZE as usize];
    write_u32(&mut sym1, 0, name_off);
    sym1[4] = 0x10;
    dynsym.extend_from_slice(&sym1);

    // Build the single rela.
    let mut rela = vec![0u8; RELA_SIZE as usize];
    write_u64(&mut rela, 0, rela_offset);
    let r_sym: u64 = 1; // index into dynsym for our import
    let r_info: u64 = (r_sym << 32) | rela_type as u64;
    write_u64(&mut rela, 8, r_info);
    write_i64(&mut rela, 16, 0); // r_addend

    // Build section headers.
    // Indexes: 0 = NULL, 1 = .dynsym, 2 = .dynstr, 3 = .rela.dyn.
    let mut shdrs = vec![0u8; (4 * SHDR_SIZE) as usize];
    // Section 1: .dynsym
    let s1 = SHDR_SIZE as usize;
    write_u32(&mut shdrs, s1, 0); // sh_name
    write_u32(&mut shdrs, s1 + 4, SHT_DYNSYM);
    write_u64(&mut shdrs, s1 + 16, 0); // sh_addr
    write_u64(&mut shdrs, s1 + 24, dynsym_off);
    write_u64(&mut shdrs, s1 + 32, dynsym.len() as u64);
    write_u32(&mut shdrs, s1 + 40, 2); // sh_link -> .dynstr
    write_u64(&mut shdrs, s1 + 56, SYM_SIZE);
    // Section 2: .dynstr
    let s2 = (2 * SHDR_SIZE) as usize;
    write_u32(&mut shdrs, s2 + 4, SHT_STRTAB);
    write_u64(&mut shdrs, s2 + 24, dynstr_off);
    write_u64(&mut shdrs, s2 + 32, dynstr.len() as u64);
    // Section 3: .rela.dyn
    let s3 = (3 * SHDR_SIZE) as usize;
    write_u32(&mut shdrs, s3 + 4, SHT_RELA);
    write_u64(&mut shdrs, s3 + 24, rela_off);
    write_u64(&mut shdrs, s3 + 32, RELA_SIZE);
    write_u32(&mut shdrs, s3 + 40, 1); // sh_link -> .dynsym
    write_u64(&mut shdrs, s3 + 56, RELA_SIZE);

    // Compute total file size: bytes up through the last payload.
    let total = (data_p_offset + data_payload.len() as u64) as usize;
    let mut out = vec![0u8; total];

    // Ehdr.
    out[0..4].copy_from_slice(&ELFMAG);
    out[4] = ELFCLASS64;
    out[5] = ELFDATA2LSB;
    out[6] = EV_CURRENT;
    write_u16(&mut out, 16, ET_EXEC);
    write_u16(&mut out, 18, EM_X86_64);
    write_u32(&mut out, 20, EV_CURRENT as u32);
    write_u64(&mut out, 24, text_p_vaddr); // entry
    write_u64(&mut out, 32, EHDR_SIZE); // e_phoff
    write_u64(&mut out, 40, shoff); // e_shoff
    write_u16(&mut out, 52, EHDR_SIZE as u16);
    write_u16(&mut out, 54, PHDR_SIZE as u16);
    write_u16(&mut out, 56, phdrs.len() as u16);
    write_u16(&mut out, 58, SHDR_SIZE as u16);
    write_u16(&mut out, 60, 4); // e_shnum
    write_u16(&mut out, 62, 0); // e_shstrndx

    // Phdrs.
    for (i, ph) in phdrs.iter().enumerate() {
        let off = (EHDR_SIZE + i as u64 * PHDR_SIZE) as usize;
        write_u32(&mut out, off, ph.p_type);
        write_u32(&mut out, off + 4, ph.p_flags);
        write_u64(&mut out, off + 8, ph.p_offset);
        write_u64(&mut out, off + 16, ph.p_vaddr);
        write_u64(&mut out, off + 24, ph.p_vaddr);
        write_u64(&mut out, off + 32, ph.p_filesz);
        write_u64(&mut out, off + 40, ph.p_memsz);
        write_u64(&mut out, off + 48, ph.p_align);
    }

    // Section headers.
    out[shoff as usize..shoff as usize + shdrs.len()].copy_from_slice(&shdrs);
    // dynsym.
    out[dynsym_off as usize..dynsym_off as usize + dynsym.len()].copy_from_slice(&dynsym);
    // dynstr.
    out[dynstr_off as usize..dynstr_off as usize + dynstr.len()].copy_from_slice(&dynstr);
    // rela.
    out[rela_off as usize..rela_off as usize + rela.len()].copy_from_slice(&rela);
    // text.
    out[text_p_offset as usize..text_p_offset as usize + text_payload.len()]
        .copy_from_slice(&text_payload);
    // (data_payload is all zeros — already there)

    out
}
