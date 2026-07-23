//! Kernel-side desktop host for the ring-3 shell (`DESKTOP.ELF`).
//!
//! The desktop shell itself lives entirely in ring 3 (`userland/apps/desktop`).
//! The kernel only has to stand up the compositor's desktop-root window (screen
//! + wallpaper) so the ring-3 panel and `gui_win_create` have a parent to
//! attach to, and then launch the shell process. The former in-kernel
//! `guishell` (taskbar, Start menu, launch policy) was removed once the ring-3
//! shell became the only shell.

use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::window::windows::DesktopWindow;
use crate::window::{self, Rect};

/// Guards against re-creating the desktop root if init is ever called twice.
static DESKTOP_ROOT_READY: AtomicBool = AtomicBool::new(false);

/// Create the GUI screen, desktop-root window, and wallpaper — no taskbar,
/// Start button, or tray (the ring-3 `DESKTOP.ELF` owns all chrome). The root
/// window is required so `gui_win_create` (and the ring-3 panel) have a parent
/// to attach to.
#[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
pub fn init_desktop_root_only() {
    if DESKTOP_ROOT_READY.load(Ordering::Acquire) {
        return;
    }

    // Load the bundled wallpaper outside the GUI lock so the file read can't
    // block window state. Falling back to None yields the solid-color desktop.
    let wallpaper = crate::system_control::load_configured_wallpaper();

    let created = window::with_window_manager(|wm| {
        let (width, height) = wm.screen_dimensions();
        let screen_id = wm.create_screen(window::ScreenMode::Gui);
        wm.switch_screen(screen_id);

        let desktop_id = wm.create_window(None);
        let desktop_bounds = Rect::new(0, 0, width, height);
        let desktop_window: Box<dyn window::Window> = match wallpaper {
            Some(bytes) => Box::new(DesktopWindow::new_with_wallpaper(
                desktop_id,
                desktop_bounds,
                bytes,
            )),
            None => Box::new(DesktopWindow::new(desktop_id, desktop_bounds)),
        };
        wm.set_window_impl(desktop_id, desktop_window);
        if let Some(screen) = wm.get_active_screen_mut() {
            screen.set_root_window(desktop_id);
        }
        if let Some(window) = wm.window_registry.get_mut(&desktop_id) {
            window.invalidate();
        }
        wm.force_full_repaint();
        crate::debug_info!("desktop root initialized for ring-3 shell (desktop={desktop_id:?})");
    });

    if created.is_some() {
        DESKTOP_ROOT_READY.store(true, Ordering::Release);
    }
}

/// Submit the ring-3 desktop shell (`DESKTOP.ELF`). It claims the shell role
/// via `gui_shell_register` on startup and owns the taskbar/Start menu/tray.
pub fn spawn_ring3_desktop_shell() {
    spawn_gui_user_app("/host/DESKTOP.ELF", "desktop");
}

fn spawn_gui_user_app(path: &'static str, name: &'static str) {
    use crate::userland::process_service::{LaunchOutcome, LaunchSpec, DEFAULT_USER_ENV};

    let argv = [name];
    let spec =
        LaunchSpec::new(path, &argv, &DEFAULT_USER_ENV).on_complete(Box::new(move |outcome| {
            match outcome {
                LaunchOutcome::Exited {
                    pid, kind, code, ..
                } => crate::debug_info!(
                    "desktop shell {} pid={} exited: kind={:?}, code={}",
                    path,
                    pid,
                    kind,
                    code
                ),
                LaunchOutcome::Failed { error, .. } => {
                    crate::debug_error!("desktop shell {} failed: {}", path, error);
                    let message = alloc::format!("Could not start {}: {}", name, error);
                    crate::window::dialogs::show_error("Desktop", &message);
                }
            }
        }));
    if let Err(error) = crate::userland::process_service::submit(spec) {
        crate::debug_error!("Could not submit {}: {}", path, error);
        let message = alloc::format!("Could not start {}: {}", name, error);
        crate::window::dialogs::show_error("Desktop", &message);
    }
}
