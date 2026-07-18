#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NetworkConfig {
    pub configured: bool,
    pub address: [u8; 4],
    pub prefix_len: u8,
    pub router: Option<[u8; 4]>,
    pub dns_servers: [[u8; 4]; 3],
    pub dns_server_count: u8,
}
