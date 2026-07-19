//! Host-provided run identity, read allocation-free from QEMU fw_cfg.

use core::sync::atomic::{AtomicU64, Ordering};

const FW_CFG_PATH: &str = "opt/agenticos/run_id";
static RUN_LO: AtomicU64 = AtomicU64::new(0);
static RUN_HI: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    let mut encoded = [0u8; 40];
    let Some(length) = crate::drivers::fw_cfg::read_file(FW_CFG_PATH, &mut encoded) else {
        return;
    };
    let length = encoded[..length]
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(length);
    let mut decoded = [0u8; 16];
    let mut source = 0usize;
    let mut destination = 0usize;
    while source < length && destination < decoded.len() {
        if encoded[source] == b'-' {
            source += 1;
            continue;
        }
        if source + 1 >= length {
            return;
        }
        let Some(high) = hex(encoded[source]) else {
            return;
        };
        let Some(low) = hex(encoded[source + 1]) else {
            return;
        };
        decoded[destination] = (high << 4) | low;
        destination += 1;
        source += 2;
    }
    if destination != decoded.len() {
        return;
    }
    RUN_LO.store(
        u64::from_le_bytes(decoded[..8].try_into().unwrap()),
        Ordering::Release,
    );
    RUN_HI.store(
        u64::from_le_bytes(decoded[8..].try_into().unwrap()),
        Ordering::Release,
    );
}

const fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub fn run_id() -> [u8; 16] {
    let mut result = [0u8; 16];
    result[..8].copy_from_slice(&RUN_LO.load(Ordering::Acquire).to_le_bytes());
    result[8..].copy_from_slice(&RUN_HI.load(Ordering::Acquire).to_le_bytes());
    result
}
