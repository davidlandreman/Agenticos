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
pub fn run_tests() {
    use crate::{debug_info};
    use crate::lib::test_utils::{Testable, exit_qemu_success};

    debug_info!("=== Running Kernel Tests ===");

    // Collect all tests from modules
    let basic_tests = basic::get_tests();
    let memory_tests = memory::get_tests();
    let display_tests = display::get_tests();
    let interrupts_tests = interrupts::get_tests();
    let heap_tests = heap::get_tests();
    let arc_tests = arc::get_tests();
    let filesystem_tests = filesystem::get_tests();
    let tools_tests = tools::get_tests();
    let userland_tests = userland::get_tests();
    let fonts_tests = fonts::get_tests();
    let window_clipping_tests = window_clipping::get_tests();
    let graphics_device_image_tests = graphics_device_image::get_tests();
    let desktop_window_tests = desktop_window::get_tests();
    let mouse_event_extension_tests = mouse_event_extension_tests::get_tests();
    let layout_tests = layout_tests::get_tests();
    let selection_tests = selection_tests::get_tests();
    let scroll_view_tests = scroll_view_tests::get_tests();
    let trait_delegation_tests = trait_delegation_tests::get_tests();

    let mut total_tests = 0;

    // Run basic tests
    debug_info!("\n[Basic Tests]");
    debug_info!("Running {} tests", basic_tests.len());
    for test in basic_tests {
        test.run();
        total_tests += 1;
    }

    // Run memory tests
    debug_info!("\n[Memory Tests]");
    debug_info!("Running {} tests", memory_tests.len());
    for test in memory_tests {
        test.run();
        total_tests += 1;
    }

    // Run display tests
    debug_info!("\n[Display Tests]");
    debug_info!("Running {} tests", display_tests.len());
    for test in display_tests {
        test.run();
        total_tests += 1;
    }

    // Run interrupt tests
    debug_info!("\n[Interrupt Tests]");
    debug_info!("Running {} tests", interrupts_tests.len());
    for test in interrupts_tests {
        test.run();
        total_tests += 1;
    }

    // Run heap tests
    debug_info!("\n[Heap Tests]");
    debug_info!("Running {} tests", heap_tests.len());
    for test in heap_tests {
        test.run();
        total_tests += 1;
    }

    // Run Arc tests
    debug_info!("\n[Arc Tests]");
    debug_info!("Running {} tests", arc_tests.len());
    for test in arc_tests {
        test.run();
        total_tests += 1;
    }

    // Run Filesystem tests
    debug_info!("\n[Filesystem Tests]");
    debug_info!("Running {} tests", filesystem_tests.len());
    for test in filesystem_tests {
        test.run();
        total_tests += 1;
    }

    // Run Tools (registry) tests
    debug_info!("\n[Tools Tests]");
    debug_info!("Running {} tests", tools_tests.len());
    for test in tools_tests {
        test.run();
        total_tests += 1;
    }

    // Run Userland tests
    debug_info!("\n[Userland Tests]");
    debug_info!("Running {} tests", userland_tests.len());
    for test in userland_tests {
        test.run();
        total_tests += 1;
    }

    // Run Font tests
    debug_info!("\n[Font Tests]");
    debug_info!("Running {} tests", fonts_tests.len());
    for test in fonts_tests {
        test.run();
        total_tests += 1;
    }

    // Run window clipping tests
    debug_info!("\n[Window Clipping Tests]");
    debug_info!("Running {} tests", window_clipping_tests.len());
    for test in window_clipping_tests {
        test.run();
        total_tests += 1;
    }

    // Run GraphicsDevice image tests
    debug_info!("\n[GraphicsDevice Image Tests]");
    debug_info!("Running {} tests", graphics_device_image_tests.len());
    for test in graphics_device_image_tests {
        test.run();
        total_tests += 1;
    }

    // Run DesktopWindow tests
    debug_info!("\n[DesktopWindow Tests]");
    debug_info!("Running {} tests", desktop_window_tests.len());
    for test in desktop_window_tests {
        test.run();
        total_tests += 1;
    }

    // Run MouseEvent extension tests (U16)
    debug_info!("\n[MouseEvent Extension Tests]");
    debug_info!("Running {} tests", mouse_event_extension_tests.len());
    for test in mouse_event_extension_tests {
        test.run();
        total_tests += 1;
    }

    // Run Layout tests (U2)
    debug_info!("\n[Layout Tests]");
    debug_info!("Running {} tests", layout_tests.len());
    for test in layout_tests {
        test.run();
        total_tests += 1;
    }

    // Run Selection tests (U1)
    debug_info!("\n[Selection Tests]");
    debug_info!("Running {} tests", selection_tests.len());
    for test in selection_tests {
        test.run();
        total_tests += 1;
    }

    // Run ScrollView tests (U3)
    debug_info!("\n[ScrollView Tests]");
    debug_info!("Running {} tests", scroll_view_tests.len());
    for test in scroll_view_tests {
        test.run();
        total_tests += 1;
    }

    // Run trait-delegation tests (U5)
    debug_info!("\n[Trait Delegation Tests]");
    debug_info!("Running {} tests", trait_delegation_tests.len());
    for test in trait_delegation_tests {
        test.run();
        total_tests += 1;
    }

    debug_info!("\n=== All {} tests passed! ===", total_tests);
    exit_qemu_success();
}
