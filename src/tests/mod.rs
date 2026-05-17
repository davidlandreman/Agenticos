#[cfg(feature = "test")]
pub mod filter;

#[cfg(feature = "test")]
pub mod basic;
#[cfg(feature = "test")]
pub mod memory;
#[cfg(feature = "test")]
pub mod display;
#[cfg(feature = "test")]
pub mod interrupts;
#[cfg(feature = "test")]
pub mod heap;
#[cfg(feature = "test")]
pub mod arc;
#[cfg(feature = "test")]
pub mod filesystem;
#[cfg(feature = "test")]
pub mod fat_write;
#[cfg(feature = "test")]
pub mod tools;
#[cfg(feature = "test")]
pub mod userland;
#[cfg(feature = "test")]
pub mod userland_fixtures;
#[cfg(feature = "test")]
pub mod fonts;
#[cfg(feature = "test")]
pub mod window_clipping;
#[cfg(feature = "test")]
pub mod graphics_device_image;
#[cfg(feature = "test")]
pub mod desktop_window;
#[cfg(feature = "test")]
pub mod mouse_event_extension_tests;
#[cfg(feature = "test")]
pub mod layout_tests;
#[cfg(feature = "test")]
pub mod selection_tests;
#[cfg(feature = "test")]
pub mod scroll_view_tests;
#[cfg(feature = "test")]
pub mod trait_delegation_tests;
#[cfg(feature = "test")]
pub mod list_migration_tests;
#[cfg(feature = "test")]
pub mod tree_view_tests;
#[cfg(feature = "test")]
pub mod splitter_tests;
#[cfg(feature = "test")]
pub mod toolbar_status_tests;
#[cfg(feature = "test")]
pub mod path_bar_tests;
#[cfg(feature = "test")]
pub mod icon_view_tests;
#[cfg(feature = "test")]
pub mod progress_bar_tests;
#[cfg(feature = "test")]
pub mod text_editor_migration_tests;
#[cfg(feature = "test")]
pub mod explorer_dir_model_tests;
#[cfg(feature = "test")]
pub mod explorer_dispatch_tests;
#[cfg(feature = "test")]
pub mod compositor;
#[cfg(feature = "test")]
pub mod window_manager_render;
#[cfg(feature = "test")]
pub mod window_buffer;
#[cfg(feature = "test")]
pub mod desktop_backing_store;

#[cfg(feature = "test")]
type GetTestsFn = fn() -> &'static [&'static dyn crate::lib::test_utils::Testable];

/// Registry of test modules. Adding a new module = one line here.
///
/// Names are used as the `<module>` half of `<module>::<fn>` filter matching
/// (see `filter.rs`). Keep them short and lowercase.
#[cfg(feature = "test")]
static MODULES: &[(&str, GetTestsFn)] = &[
    ("basic", basic::get_tests),
    ("memory", memory::get_tests),
    ("display", display::get_tests),
    ("interrupts", interrupts::get_tests),
    ("heap", heap::get_tests),
    ("arc", arc::get_tests),
    ("filesystem", filesystem::get_tests),
    ("fat_lfn", crate::fs::fat::lfn::lfn_tests),
    ("tmpfs", crate::fs::tmpfs::filesystem::tmpfs_tests),
    ("overlay", crate::fs::overlay::filesystem::overlay_tests),
    ("fat_write", fat_write::get_tests),
    ("tools", tools::get_tests),
    ("userland", userland::get_tests),
    ("path", crate::userland::path::path_tests),
    ("bin_namespace", crate::userland::bin_namespace::bin_namespace_tests),
    ("gui_launch_table", crate::commands::gui_launch_table::gui_launch_table_tests),
    ("fonts", fonts::get_tests),
    ("window_clipping", window_clipping::get_tests),
    ("graphics_device_image", graphics_device_image::get_tests),
    ("desktop_window", desktop_window::get_tests),
    ("mouse_event_extension", mouse_event_extension_tests::get_tests),
    ("layout", layout_tests::get_tests),
    ("selection", selection_tests::get_tests),
    ("scroll_view", scroll_view_tests::get_tests),
    ("trait_delegation", trait_delegation_tests::get_tests),
    ("list_migration", list_migration_tests::get_tests),
    ("tree_view", tree_view_tests::get_tests),
    ("splitter", splitter_tests::get_tests),
    ("toolbar_status", toolbar_status_tests::get_tests),
    ("path_bar", path_bar_tests::get_tests),
    ("icon_view", icon_view_tests::get_tests),
    ("progress_bar", progress_bar_tests::get_tests),
    ("text_editor_migration", text_editor_migration_tests::get_tests),
    ("explorer_dir_model", explorer_dir_model_tests::get_tests),
    ("explorer_dispatch", explorer_dispatch_tests::get_tests),
    ("compositor", compositor::get_tests),
    ("window_manager_render", window_manager_render::get_tests),
    ("window_buffer", window_buffer::get_tests),
    ("desktop_backing_store", desktop_backing_store::get_tests),
    ("filter", filter::get_tests),
];

/// Strip the `agenticos::tests::<topic>::` prefix from a test's `type_name`,
/// yielding `<module>::<fn>` (matching the registry's module name).
///
/// Falls back to the original name when the prefix is absent (e.g. tests
/// declared outside `crate::tests`).
#[cfg(feature = "test")]
fn short_name<'a>(module: &str, full: &'a str) -> &'a str {
    // type_name typically returns "agenticos::tests::<topic>::<fn>".
    // Trim down to "<module>::<fn>" — using the registry's `module` name
    // rather than the topic file name keeps filter strings stable even when
    // a file is renamed.
    if let Some(idx) = full.rfind("::") {
        let fn_name = &full[idx + 2..];
        // Caller's module name + fn name, joined with "::". Returning a
        // subslice of `full` would be wrong here because module may differ
        // from the topic; we cheat by storing nothing and asking the matcher
        // to take both pieces. But Testable::name returns &'static str, so
        // we can't allocate. Instead, the matcher gets `module` separately
        // and only needs `<fn>` from us — return that.
        let _ = module;
        return fn_name;
    }
    full
}

#[cfg(feature = "test")]
pub fn run_tests() {
    use crate::debug_info;
    use crate::lib::test_utils::{exit_qemu_failed, exit_qemu_success};

    debug_info!("=== Running Kernel Tests ===");
    if let Some(f) = filter::filter_str() {
        debug_info!("Filter: {:?}", f);
    }

    let mut total_run = 0usize;
    let mut total_skipped = 0usize;
    let mut modules_with_matches = 0usize;

    for (module_name, get) in MODULES {
        let tests = get();

        // Count matches for this module so we can print "N of M".
        let mut matched = 0usize;
        for t in tests.iter() {
            let fn_name = short_name(module_name, t.name());
            if filter_matches(module_name, fn_name) {
                matched += 1;
            }
        }

        if matched == 0 {
            total_skipped += tests.len();
            continue;
        }
        modules_with_matches += 1;

        if matched == tests.len() {
            debug_info!("\n[{}] Running {} tests", module_name, matched);
        } else {
            debug_info!(
                "\n[{}] Running {} of {} tests",
                module_name,
                matched,
                tests.len()
            );
            total_skipped += tests.len() - matched;
        }

        for t in tests.iter() {
            let fn_name = short_name(module_name, t.name());
            if !filter_matches(module_name, fn_name) {
                continue;
            }
            t.run();
            total_run += 1;
        }
    }

    if total_run == 0 {
        crate::debug_error!(
            "No tests matched filter {:?}",
            filter::filter_str().unwrap_or("")
        );
        exit_qemu_failed();
        return;
    }

    if filter::is_empty() {
        debug_info!("\n=== All {} tests passed! ===", total_run);
    } else {
        debug_info!(
            "\n=== {} tests passed across {} module(s) ({} skipped by filter) ===",
            total_run,
            modules_with_matches,
            total_skipped
        );
    }
    exit_qemu_success();
}

/// Build `<module>::<fn>` on the stack (no alloc) and run filter matching
/// against it. `filter::matches` already tries each pattern against both
/// `module` and the joined name.
#[cfg(feature = "test")]
fn filter_matches(module: &str, fn_name: &str) -> bool {
    if filter::is_empty() {
        return true;
    }
    let mut buf = [0u8; 128];
    let m = module.as_bytes();
    let f = fn_name.as_bytes();
    let total = m.len() + 2 + f.len();
    if total > buf.len() {
        // Pathological name — fall back to module-only check.
        return filter::matches(module, module);
    }
    buf[..m.len()].copy_from_slice(m);
    buf[m.len()] = b':';
    buf[m.len() + 1] = b':';
    buf[m.len() + 2..total].copy_from_slice(f);
    let joined = match core::str::from_utf8(&buf[..total]) {
        Ok(s) => s,
        Err(_) => return false,
    };
    filter::matches(module, joined)
}
