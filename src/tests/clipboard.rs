use crate::arch::x86_64::syscall::SyscallArgs;
use crate::lib::test_utils::Testable;
use crate::userland::abi::{self, EINVAL, EIO, EMSGSIZE, EOPNOTSUPP};

fn test_request_header_contract() {
    let header = crate::clipboard::request_header(crate::clipboard::OP_COPY, 0x1234)
        .expect("bounded length");
    assert_eq!(&header[..4], b"ACCB");
    assert_eq!(header[4], 1);
    assert_eq!(header[5], crate::clipboard::OP_COPY);
    assert_eq!(
        u32::from_le_bytes(header[6..10].try_into().unwrap()),
        0x1234
    );
}

fn test_response_header_contract() {
    let mut header = [0u8; 10];
    header[..4].copy_from_slice(b"ACBR");
    header[4] = 1;
    header[5] = 0;
    header[6..10].copy_from_slice(&37u32.to_le_bytes());
    let parsed = crate::clipboard::parse_response_header(&header).expect("valid response");
    assert_eq!(parsed.status, 0);
    assert_eq!(parsed.payload_len, 37);

    header[0] = b'X';
    assert_eq!(crate::clipboard::parse_response_header(&header), Err(EIO));
}

fn test_status_errno_mapping() {
    assert_eq!(crate::clipboard::status_errno(1), EINVAL);
    assert_eq!(crate::clipboard::status_errno(2), EOPNOTSUPP);
    assert_eq!(crate::clipboard::status_errno(4), EMSGSIZE);
    assert_eq!(crate::clipboard::status_errno(255), EIO);
}

fn test_syscall_rejects_invalid_operation_and_oversize() {
    let mut args = SyscallArgs {
        rdi: 257,
        ..SyscallArgs::default()
    };
    assert_eq!(crate::clipboard::syscall_handler(&args), EINVAL);

    args.rdi = crate::clipboard::OP_COPY as u64;
    args.rdx = crate::clipboard::MAX_TEXT_BYTES as u64 + 1;
    assert_eq!(crate::clipboard::syscall_handler(&args), EMSGSIZE);
}

fn test_syscall_rejects_non_utf8_copy_before_transport() {
    let bytes = [0xffu8];
    let pointer = bytes.as_ptr() as u64;
    abi::set_user_va_bounds(abi::UserVaBounds {
        start: pointer,
        end: pointer + bytes.len() as u64,
    });
    let args = SyscallArgs {
        rdi: crate::clipboard::OP_COPY as u64,
        rsi: pointer,
        rdx: bytes.len() as u64,
        ..SyscallArgs::default()
    };
    assert_eq!(crate::clipboard::syscall_handler(&args), EINVAL);
    abi::clear_user_va_bounds();
}

fn test_exec_resets_stale_task_userspace_pointers() {
    const TID: u32 = 0xC11B_0001;
    crate::userland::lifecycle::set_clear_child_tid(TID, 0x500000);
    crate::userland::lifecycle::set_robust_list(TID, 0x600000, 24);
    assert_eq!(
        crate::userland::lifecycle::clear_child_tid(TID),
        Some(0x500000)
    );
    assert_eq!(
        crate::userland::lifecycle::robust_list(TID),
        Some((0x600000, 24))
    );

    crate::userland::lifecycle::reset_task_exec_metadata(TID);

    assert_eq!(crate::userland::lifecycle::clear_child_tid(TID), None);
    assert_eq!(crate::userland::lifecycle::robust_list(TID), None);
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_request_header_contract,
        &test_response_header_contract,
        &test_status_errno_mapping,
        &test_syscall_rejects_invalid_operation_and_oversize,
        &test_syscall_rejects_non_utf8_copy_before_transport,
        &test_exec_resets_stale_task_userspace_pointers,
    ]
}
