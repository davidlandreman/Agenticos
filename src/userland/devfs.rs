//! Minimal synthetic device namespace used by Linux-compatible userland.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceNode {
    Directory,
    Urandom,
}

pub fn classify(path: &str) -> Option<DeviceNode> {
    match path {
        "/dev" | "/dev/" => Some(DeviceNode::Directory),
        "/dev/urandom" => Some(DeviceNode::Urandom),
        _ => None,
    }
}

pub fn is_dev_path(path: &str) -> bool {
    path == "/dev" || path == "/dev/" || path.starts_with("/dev/")
}
