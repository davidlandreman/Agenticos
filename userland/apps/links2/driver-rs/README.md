# AgenticOS Links driver core

This isolated `no_std` Rust static library owns the AgenticOS window surface,
software raster operations, GUI syscalls, and event translation used by the
thin Links C ABI adapter. It deliberately does not link the AgenticOS Rust
runtime because Links already owns musl's allocator and process startup.
