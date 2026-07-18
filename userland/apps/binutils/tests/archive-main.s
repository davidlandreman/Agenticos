.text
.global _start
.extern binutils_probe_add

_start:
    mov $19, %edi
    mov $23, %esi
    call binutils_probe_add
    mov %eax, %edi
    mov $60, %eax
    syscall
