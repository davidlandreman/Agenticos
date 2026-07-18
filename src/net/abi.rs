#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SockAddrV4 {
    pub address: [u8; 4],
    pub port: u16,
}

impl SockAddrV4 {
    pub const UNSPECIFIED: Self = Self {
        address: [0; 4],
        port: 0,
    };

    pub fn is_unspecified(&self) -> bool {
        self.address == [0; 4]
    }
}
