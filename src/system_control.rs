//! Persistent system preferences and the ring-3 control syscall.
//!
//! This module owns requested preferences and persistence. Effective visual
//! state remains owned by the window manager/theme and desktop window.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use core::mem;
use core::sync::atomic::{AtomicU8, Ordering};

use crate::arch::x86_64::preemption_guard::PreemptionMutex;
use crate::arch::x86_64::syscall::SyscallArgs;
use crate::fs::file_handle::{File, FileError};
use crate::fs::filesystem::FilesystemError;
use crate::userland::abi::{EFAULT, EINVAL, EIO, ENOENT, ERANGE};
use crate::window::renderer::RendererKind;
use crate::window::theme::{ThemeKind, ThemeRequest};

const SETTINGS_DIR: &str = "/data/agenticos";
const SETTINGS_PATH: &str = "/data/agenticos/settings.conf";
const SETTINGS_TEMP_PATH: &str = "/data/agenticos/.settings.new";
const MAX_SETTINGS_BYTES: usize = 4096;
const MAX_WALLPAPER_PATH_BYTES: usize = 1024;

pub const COMMAND_GET_SNAPSHOT: u64 = 0;
pub const COMMAND_GET_WALLPAPER_PATH: u64 = 1;
pub const COMMAND_SET_THEME: u64 = 2;
pub const COMMAND_SET_WALLPAPER_PATH: u64 = 3;
pub const COMMAND_RESET_WALLPAPER: u64 = 4;

pub const THEME_AVAILABLE_CLASSIC: u32 = 1 << 0;
pub const THEME_AVAILABLE_AERO: u32 = 1 << 1;
pub const THEME_AVAILABLE_FUTURISM: u32 = 1 << 2;
pub const PERSISTENCE_AVAILABLE: u32 = 1 << 0;
pub const BOOT_THEME_OVERRIDE: u32 = 1 << 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ThemePreference {
    Auto = 0,
    Classic = 1,
    Aero = 2,
    Futurism = 3,
}

impl ThemePreference {
    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "auto" => Some(Self::Auto),
            "classic" => Some(Self::Classic),
            "aero" => Some(Self::Aero),
            "futurism" => Some(Self::Futurism),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Classic => "classic",
            Self::Aero => "aero",
            Self::Futurism => "futurism",
        }
    }

    pub const fn request(self) -> ThemeRequest {
        match self {
            Self::Auto => ThemeRequest::Auto,
            Self::Classic => ThemeRequest::Classic,
            Self::Aero => ThemeRequest::Aero,
            Self::Futurism => ThemeRequest::Futurism,
        }
    }

    fn from_u64(value: u64) -> Option<Self> {
        match value {
            0 => Some(Self::Auto),
            1 => Some(Self::Classic),
            2 => Some(Self::Aero),
            3 => Some(Self::Futurism),
            _ => None,
        }
    }

    const fn for_kind(kind: ThemeKind) -> Self {
        match kind {
            ThemeKind::Classic => Self::Classic,
            ThemeKind::Aero => Self::Aero,
            ThemeKind::Futurism => Self::Futurism,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SystemControlSnapshotV1 {
    pub version: u32,
    pub byte_len: u32,
    pub theme_preference: u32,
    pub active_theme: u32,
    pub theme_available_mask: u32,
    pub renderer_kind: u32,
    pub boot_flags: u32,
    pub wallpaper_state: u32,
    pub persistence_flags: u32,
    pub display_width: u32,
    pub display_height: u32,
    pub reserved: [u32; 5],
}

const _: [(); 64] = [(); mem::size_of::<SystemControlSnapshotV1>()];

#[derive(Clone)]
struct SettingsState {
    theme: ThemePreference,
    wallpaper_path: Option<String>,
    wallpaper_fallback: bool,
    persistence_available: bool,
    boot_theme_override: bool,
}

impl SettingsState {
    const fn defaults() -> Self {
        Self {
            theme: ThemePreference::Auto,
            wallpaper_path: None,
            wallpaper_fallback: false,
            persistence_available: false,
            boot_theme_override: false,
        }
    }
}

static SETTINGS: PreemptionMutex<SettingsState> = PreemptionMutex::new(SettingsState::defaults());
static PENDING_THEME_PUBLICATION: AtomicU8 = AtomicU8::new(0);

pub fn init() {
    let persistent = ensure_settings_dir();
    let mut loaded = SettingsState::defaults();
    loaded.persistence_available = persistent;

    if let Ok(file) = File::open_read(SETTINGS_PATH) {
        let size = file.size() as usize;
        if size <= MAX_SETTINGS_BYTES {
            if let Ok(bytes) = file.read_to_vec() {
                if let Ok(text) = core::str::from_utf8(&bytes) {
                    parse_config_into(text, &mut loaded);
                } else {
                    crate::debug_warn!("system settings ignored invalid UTF-8");
                }
            }
        } else {
            crate::debug_warn!("system settings ignored oversized file: {} bytes", size);
        }
    }

    crate::debug_info!(
        "system settings loaded theme={} wallpaper={} persistent={}",
        loaded.theme.as_str(),
        loaded.wallpaper_path.as_deref().unwrap_or("default"),
        loaded.persistence_available,
    );
    *SETTINGS.lock() = loaded;
}

fn parse_config_into(text: &str, state: &mut SettingsState) {
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "theme" => {
                if let Some(theme) = ThemePreference::parse(value) {
                    state.theme = theme;
                }
            }
            "wallpaper" => {
                let value = value.trim();
                if value == "default" {
                    state.wallpaper_path = None;
                } else if valid_absolute_path(value) {
                    state.wallpaper_path = Some(crate::userland::path::normalize_path("/", value));
                }
            }
            _ => {}
        }
    }
}

fn ensure_settings_dir() -> bool {
    if !crate::fs::vfs::vfs_is_writable("/data") {
        return false;
    }
    match crate::fs::vfs::vfs_mkdir(SETTINGS_DIR) {
        Ok(()) | Err(FilesystemError::AlreadyExists) => true,
        Err(error) => {
            crate::debug_warn!("system settings storage unavailable: {:?}", error);
            false
        }
    }
}

fn serialize(state: &SettingsState) -> String {
    format!(
        "theme={}\nwallpaper={}\n",
        state.theme.as_str(),
        state.wallpaper_path.as_deref().unwrap_or("default"),
    )
}

fn persist_current() -> bool {
    let snapshot = SETTINGS.lock().clone();
    if !ensure_settings_dir() {
        SETTINGS.lock().persistence_available = false;
        return false;
    }
    let contents = serialize(&snapshot);
    let result = (|| {
        let file = File::create(SETTINGS_TEMP_PATH)?;
        if file.write(contents.as_bytes())? != contents.len() {
            return Err(FileError::IoError);
        }
        file.sync(false)?;
        file.close()?;
        crate::fs::vfs::vfs_rename(SETTINGS_TEMP_PATH, SETTINGS_PATH)
            .map_err(FileError::FilesystemError)?;
        crate::fs::vfs::vfs_sync_all().map_err(FileError::FilesystemError)?;
        Ok::<(), FileError>(())
    })();
    let ok = result.is_ok();
    SETTINGS.lock().persistence_available = ok;
    if let Err(error) = result {
        crate::debug_warn!("system settings save failed: {:?}", error);
    }
    ok
}

pub fn theme_preference() -> ThemePreference {
    SETTINGS.lock().theme
}

pub fn record_boot_theme_override(explicit: bool) {
    SETTINGS.lock().boot_theme_override = explicit;
}

pub fn queue_theme_publication(kind: ThemeKind) {
    PENDING_THEME_PUBLICATION.store((kind as u8) + 1, Ordering::Release);
}

/// Flush renderer-fallback notifications after the compositor releases the
/// window-manager lock.
pub fn drain_pending_notifications() {
    let pending = PENDING_THEME_PUBLICATION.swap(0, Ordering::AcqRel);
    let Some(kind) = pending.checked_sub(1).and_then(ThemeKind::from_u8) else {
        return;
    };
    crate::userland::etc::publish_theme(kind);
    crate::userland::gui::broadcast_theme_changed(kind, theme_preference().request());
}

pub fn configured_wallpaper_path() -> String {
    SETTINGS
        .lock()
        .wallpaper_path
        .clone()
        .unwrap_or_else(|| crate::window::DEFAULT_WALLPAPER_PATH.to_string())
}

pub fn load_configured_wallpaper() -> Option<alloc::vec::Vec<u8>> {
    let custom = SETTINGS.lock().wallpaper_path.clone();
    if let Some(path) = custom {
        if let Some(bytes) = crate::window::load_wallpaper(&path) {
            SETTINGS.lock().wallpaper_fallback = false;
            return Some(bytes);
        }
        crate::debug_warn!(
            "configured wallpaper unavailable: {}; fallback=default",
            path
        );
        SETTINGS.lock().wallpaper_fallback = true;
    } else {
        SETTINGS.lock().wallpaper_fallback = false;
    }
    crate::window::load_default_wallpaper()
}

fn snapshot() -> SystemControlSnapshotV1 {
    let state = SETTINGS.lock().clone();
    let (renderer, width, height) = crate::window::with_window_manager(|wm| {
        let selection = wm.renderer_selection();
        let (width, height) = wm.screen_dimensions();
        (selection.selected, width, height)
    })
    .unwrap_or((RendererKind::Legacy, 0, 0));
    let active = crate::window::theme::active();
    let available = THEME_AVAILABLE_CLASSIC
        | if renderer == RendererKind::Legacy {
            0
        } else {
            THEME_AVAILABLE_AERO | THEME_AVAILABLE_FUTURISM
        };
    SystemControlSnapshotV1 {
        version: 1,
        byte_len: mem::size_of::<SystemControlSnapshotV1>() as u32,
        theme_preference: state.theme as u32,
        active_theme: ThemePreference::for_kind(active) as u32,
        theme_available_mask: available,
        renderer_kind: match renderer {
            RendererKind::Legacy => 0,
            RendererKind::RetainedCpu => 1,
            RendererKind::Virgl => 2,
        },
        boot_flags: if state.boot_theme_override {
            BOOT_THEME_OVERRIDE
        } else {
            0
        },
        wallpaper_state: if state.wallpaper_fallback {
            2
        } else if state.wallpaper_path.is_some() {
            1
        } else {
            0
        },
        persistence_flags: if state.persistence_available {
            PERSISTENCE_AVAILABLE
        } else {
            0
        },
        display_width: width,
        display_height: height,
        reserved: [0; 5],
    }
}

fn valid_absolute_path(path: &str) -> bool {
    !path.is_empty()
        && path.starts_with('/')
        && path.len() <= MAX_WALLPAPER_PATH_BYTES
        && !path.chars().any(char::is_control)
}

fn set_theme(preference: ThemePreference) -> i64 {
    let request = preference.request();
    let prior_preference = theme_preference();
    let result = crate::window::with_window_manager(|wm| wm.apply_theme_request(request));
    let (selection, changed) = match result {
        Some(Ok(value)) => value,
        Some(Err(error)) => return error,
        None => return EIO,
    };
    SETTINGS.lock().theme = preference;
    let persisted = persist_current();
    crate::userland::etc::publish_theme(selection.selected);
    if changed || prior_preference != preference {
        crate::userland::gui::broadcast_theme_changed(selection.selected, request);
    }
    crate::debug_info!(
        "system theme applied request={} effective={} persistent={}",
        request.as_str(),
        selection.selected.as_str(),
        persisted,
    );
    if persisted {
        0
    } else {
        1
    }
}

fn set_wallpaper_path(path: &str) -> i64 {
    if !valid_absolute_path(path) {
        return EINVAL;
    }
    let normalized = crate::userland::path::normalize_path("/", path);
    let Some(bytes) = crate::window::load_wallpaper(&normalized) else {
        return ENOENT;
    };
    let applied = crate::window::with_window_manager(|wm| wm.set_desktop_wallpaper(Some(bytes)))
        .unwrap_or(false);
    if !applied {
        return EIO;
    }
    {
        let mut state = SETTINGS.lock();
        state.wallpaper_path = Some(normalized.clone());
        state.wallpaper_fallback = false;
    }
    let persisted = persist_current();
    crate::userland::gui::broadcast_settings_changed();
    crate::debug_info!(
        "system wallpaper applied path={} persistent={}",
        normalized,
        persisted,
    );
    if persisted {
        0
    } else {
        1
    }
}

fn reset_wallpaper() -> i64 {
    let bytes = crate::window::load_default_wallpaper();
    let applied =
        crate::window::with_window_manager(|wm| wm.set_desktop_wallpaper(bytes)).unwrap_or(false);
    if !applied {
        return EIO;
    }
    {
        let mut state = SETTINGS.lock();
        state.wallpaper_path = None;
        state.wallpaper_fallback = false;
    }
    let persisted = persist_current();
    crate::userland::gui::broadcast_settings_changed();
    if persisted {
        0
    } else {
        1
    }
}

pub fn syscall_handler(args: &mut SyscallArgs) -> i64 {
    if args.r8 != 0 {
        return EINVAL;
    }
    match args.rdi {
        COMMAND_GET_SNAPSHOT => {
            if args.rsi != 0 || args.r10 < mem::size_of::<SystemControlSnapshotV1>() as u64 {
                return EINVAL;
            }
            let value = snapshot();
            match crate::userland::usercopy::write_unaligned(args.rdx, &value) {
                Ok(()) => mem::size_of::<SystemControlSnapshotV1>() as i64,
                Err(_) => EFAULT,
            }
        }
        COMMAND_GET_WALLPAPER_PATH => {
            if args.rsi != 0 {
                return EINVAL;
            }
            let path = configured_wallpaper_path();
            if args.r10 < path.len() as u64 {
                return ERANGE;
            }
            match crate::userland::usercopy::copy_to_user(args.rdx, path.as_bytes()) {
                Ok(()) => path.len() as i64,
                Err(_) => EFAULT,
            }
        }
        COMMAND_SET_THEME => {
            if args.rdx != 0 || args.r10 != 0 {
                return EINVAL;
            }
            match ThemePreference::from_u64(args.rsi) {
                Some(preference) => set_theme(preference),
                None => EINVAL,
            }
        }
        COMMAND_SET_WALLPAPER_PATH => {
            if args.rsi != 0 || args.r10 == 0 || args.r10 as usize > MAX_WALLPAPER_PATH_BYTES {
                return EINVAL;
            }
            let mut bytes = vec![0u8; args.r10 as usize];
            if crate::userland::usercopy::copy_from_user(&mut bytes, args.rdx).is_err() {
                return EFAULT;
            }
            match String::from_utf8(bytes) {
                Ok(path) => set_wallpaper_path(&path),
                Err(_) => EINVAL,
            }
        }
        COMMAND_RESET_WALLPAPER => {
            if args.rsi != 0 || args.rdx != 0 || args.r10 != 0 {
                EINVAL
            } else {
                reset_wallpaper()
            }
        }
        _ => EINVAL,
    }
}

#[cfg(feature = "test")]
mod tests {
    use super::*;

    fn test_config_parser_is_forward_compatible() {
        let mut state = SettingsState::defaults();
        parse_config_into(
            "unknown=value\ntheme=classic\nwallpaper=/data/wall.bmp\n",
            &mut state,
        );
        assert_eq!(state.theme, ThemePreference::Classic);
        assert_eq!(state.wallpaper_path.as_deref(), Some("/data/wall.bmp"));
    }

    fn test_bad_individual_values_keep_defaults() {
        let mut state = SettingsState::defaults();
        parse_config_into("theme=purple\nwallpaper=relative.bmp\n", &mut state);
        assert_eq!(state.theme, ThemePreference::Auto);
        assert!(state.wallpaper_path.is_none());
    }

    fn test_snapshot_layout_is_stable() {
        assert_eq!(mem::size_of::<SystemControlSnapshotV1>(), 64);
    }

    fn test_futurism_preference_round_trip() {
        let mut state = SettingsState::defaults();
        parse_config_into("theme=futurism\n", &mut state);
        assert_eq!(state.theme, ThemePreference::Futurism);
        assert_eq!(serialize(&state), "theme=futurism\nwallpaper=default\n");
        assert_eq!(
            ThemePreference::from_u64(3),
            Some(ThemePreference::Futurism)
        );
        assert_eq!(ThemePreference::from_u64(4), None);
        assert_eq!(ThemePreference::Futurism.request(), ThemeRequest::Futurism);
        assert_eq!(
            ThemePreference::for_kind(ThemeKind::Futurism),
            ThemePreference::Futurism
        );
    }

    fn test_snapshot_syscall_writes_versioned_payload() {
        let mut snapshot = SystemControlSnapshotV1::default();
        let pointer = &mut snapshot as *mut _ as u64;
        crate::userland::abi::set_user_va_bounds(crate::userland::abi::UserVaBounds {
            start: pointer,
            end: pointer + mem::size_of::<SystemControlSnapshotV1>() as u64,
        });
        let mut args = SyscallArgs::default();
        args.rdi = COMMAND_GET_SNAPSHOT;
        args.rdx = pointer;
        args.r10 = mem::size_of::<SystemControlSnapshotV1>() as u64;
        assert_eq!(syscall_handler(&mut args), 64);
        crate::userland::abi::clear_user_va_bounds();
        assert_eq!(snapshot.version, 1);
        assert_eq!(snapshot.byte_len, 64);
        assert_eq!(snapshot.reserved, [0; 5]);
    }

    pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
        &[
            &test_config_parser_is_forward_compatible,
            &test_bad_individual_values_keep_defaults,
            &test_snapshot_layout_is_stable,
            &test_futurism_preference_round_trip,
            &test_snapshot_syscall_writes_versioned_payload,
        ]
    }
}

#[cfg(feature = "test")]
pub use tests::get_tests as system_control_tests;
