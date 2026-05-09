/* Userland link script (D3, D11).
 *
 * Targets the U6 loader's accepted format:
 *   - static, non-PIE, ET_EXEC
 *   - fixed link base 0x40_0000 (the user load base U4 reserves)
 *   - no PT_INTERP (we are not dynamically linked)
 *   - no PT_TLS (the loader rejects TLS explicitly)
 *
 * NX/WX hygiene: each PT_LOAD carries flags that line up with the page-table
 * permissions the loader applies (PF_X => no NX; !PF_W => not writable).
 *   text   = R + X      (FLAGS 5)
 *   rodata = R          (FLAGS 4)
 *   data   = R + W      (FLAGS 6)
 *
 * `-z noexecstack` and `-z now` are passed via app-level `rustflags`.
 */

ENTRY(_start)

PHDRS {
    text   PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data   PT_LOAD FLAGS(6);
}

SECTIONS {
    . = 0x400000;

    .text : ALIGN(0x1000) {
        *(.text .text.*)
    } :text

    . = ALIGN(0x1000);

    .rodata : ALIGN(0x1000) {
        *(.rodata .rodata.*)
    } :rodata

    . = ALIGN(0x1000);

    .data : ALIGN(0x1000) {
        *(.data .data.*)
    } :data

    .bss : ALIGN(0x10) {
        *(.bss .bss.*)
        *(COMMON)
    } :data

    /DISCARD/ : {
        *(.eh_frame)
        *(.eh_frame_hdr)
        *(.note .note.*)
        *(.comment)
        *(.gnu.hash)
        *(.dynsym)
        *(.dynstr)
        *(.dynamic)
        *(.hash)
        *(.gnu.version*)
        *(.interp)
    }
}
