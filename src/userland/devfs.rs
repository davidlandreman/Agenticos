//! Minimal synthetic device namespace used by Linux-compatible userland.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceNode {
    Directory,
    Urandom,
    /// Discard sink / empty source. Required by git's `sanitize_stdfds`,
    /// which opens `/dev/null` O_RDWR unconditionally at startup, and by
    /// ordinary shell `> /dev/null` redirection.
    Null,
}

pub fn classify(path: &str) -> Option<DeviceNode> {
    match path {
        "/dev" | "/dev/" => Some(DeviceNode::Directory),
        "/dev/urandom" => Some(DeviceNode::Urandom),
        "/dev/null" => Some(DeviceNode::Null),
        _ => None,
    }
}

pub fn is_dev_path(path: &str) -> bool {
    path == "/dev" || path == "/dev/" || path.starts_with("/dev/")
}
