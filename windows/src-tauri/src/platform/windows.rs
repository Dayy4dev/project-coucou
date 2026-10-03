// Windows: Win32 for the island window and the cursor, %APPDATA% for files.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

use ::windows::core::{BOOL, PWSTR};
use ::windows::Win32::Foundation::{
    CloseHandle, LocalFree, HANDLE, HLOCAL, HWND, LPARAM, POINT, WPARAM,
};
use ::windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use ::windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use ::windows::Win32::System::Ole::RevokeDragDrop;
use ::windows::Win32::System::SystemInformation::GetLocalTime;
use ::windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThreadId, OpenProcessToken,
};
use ::windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL,
    MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, VK_LBUTTON, VK_SPACE,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumChildWindows, GetClassNameW, GetCursorPos, GetMessageW,
    GetWindowLongPtrW, PostThreadMessageW, SetWindowLongPtrW, TranslateMessage, GWL_EXSTYLE, MSG,
    WM_APP, WM_HOTKEY, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

use super::LocalTime;
use crate::island::WINDOW_LABEL;

/// File name of the Claude Code relay.
pub const HOOK_EXE: &str = "coucou-hook.exe";

/// Environment variable holding the home directory.
pub const HOME_VAR: &str = "USERPROFILE";

/// Keeps spawned helpers from flashing a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ── Files ─────────────────────────────────────────────────────────────────────

/// %APPDATA%\Coucou — preferences.
pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Coucou")
}

/// %LOCALAPPDATA%\Coucou — where coucou-hook.exe, the inbox and the log live.
pub fn local_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Coucou")
}

/// %APPDATA% and %LOCALAPPDATA% are already private to the user.
pub fn ensure_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// Nothing to set up before the webview starts.
pub fn prepare_environment() {}

pub fn local_time() -> LocalTime {
    let t = unsafe { GetLocalTime() };
    LocalTime {
        year: t.wYear.into(),
        month: t.wMonth.into(),
        day: t.wDay.into(),
        hour: t.wHour.into(),
        minute: t.wMinute.into(),
        second: t.wSecond.into(),
    }
}

// ── Processes ─────────────────────────────────────────────────────────────────

/// Spawned helpers must never flash a console window.
pub fn no_console(cmd: &mut Command) -> &mut Command {
    cmd.creation_flags(CREATE_NO_WINDOW)
}

pub fn open_url(url: &str) {
    let _ =
        no_console(Command::new("rundll32.exe").args(["url.dll,FileProtocolHandler", url])).spawn();
}

pub fn reveal_folder(path: &str) {
    let _ = Command::new("explorer").arg(path).spawn();
}

/// Our own `where`: walks %PATH% against %PATHEXT%, no shell involved.
/// Rust quotes arguments correctly for `.cmd`/`.bat` targets since 1.77, so
/// spawning `code.cmd` directly is safe.
pub fn find_on_path(stem: &str) -> Option<PathBuf> {
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let dirs = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&dirs) {
        for ext in exts.split(';').filter(|e| !e.is_empty()) {
            let candidate = dir.join(format!("{stem}{}", ext.to_lowercase()));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

// ── Who we are ────────────────────────────────────────────────────────────────
//
// Named pipes share one machine-wide namespace, so the SID in the name is what
// keeps two accounts on the same machine from ever meeting on `coucou-*`.
// coucou-hook computes the same string (hook/src/win.rs) and additionally checks
// that the process serving the pipe really is us.

/// The SID of the account this process runs as, as `S-1-5-21-…`.
pub fn current_user_sid() -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;

        // First call sizes the buffer, second fills it.
        let mut needed = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
        if needed == 0 {
            let _ = CloseHandle(token);
            return None;
        }
        let mut buf = vec![0u8; needed as usize];
        let ok = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
        .is_ok();
        let _ = CloseHandle(token);
        if !ok {
            return None;
        }

        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text).ok()?;
        let sid = text.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(text.0 as *mut _)));
        sid
    }
}

// ── Cursor ────────────────────────────────────────────────────────────────────

/// The 60 Hz poll reads the cursor and flips click-through from it.
pub const CURSOR_POLL: bool = true;

/// Cursor position in physical screen pixels.
pub fn cursor_physical() -> Option<(f64, f64)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p).ok()? };
    Some((p.x as f64, p.y as f64))
}

/// True while the left mouse button is held — the only signal we get that a
/// drag might be in flight before it reaches the window.
pub fn left_button_down() -> bool {
    unsafe { (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0 }
}

// ── Island window ─────────────────────────────────────────────────────────────

fn hwnd_of(win: &WebviewWindow) -> Option<HWND> {
    let raw = win.hwnd().ok()?.0 as isize;
    if raw == 0 {
        return None;
    }
    Some(HWND(raw as *mut _))
}

/// Lets dropped files reach the app again.
///
/// wry installs its drop target by walking the webview's child windows **once**,
/// when the webview is created. WebView2 creates `Chrome_RenderWidgetHostHWND`
/// later and registers its own target on it; being the innermost window, that one
/// wins, and since the page has no HTML5 drop handler it refuses everything — the
/// "no drop" cursor, with nothing reaching Tauri. Revoking it makes OLE fall
/// through to the target wry registered on the parent widget, which is the one
/// that feeds Tauri's drag events.
///
/// Cheap and idempotent, so it is simply re-run whenever a drag might be starting.
pub fn unblock_webview_drops(app: &AppHandle) {
    for label in [WINDOW_LABEL, "settings"] {
        let Some(win) = app.get_webview_window(label) else {
            continue;
        };
        let Some(hwnd) = hwnd_of(&win) else { continue };
        unsafe {
            let _ = EnumChildWindows(Some(hwnd), Some(revoke_render_widget), LPARAM(0));
        }
    }
}

unsafe extern "system" fn revoke_render_widget(hwnd: HWND, _: LPARAM) -> BOOL {
    let mut name = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, &mut name) };
    if len > 0 {
        let class = String::from_utf16_lossy(&name[..len as usize]);
        if class == "Chrome_RenderWidgetHostHWND" {
            let _ = unsafe { RevokeDragDrop(hwnd) };
        }
    }
    true.into()
}

/// WS_EX_NOACTIVATE keeps clicks from stealing focus; WS_EX_TOOLWINDOW keeps the
/// island out of Alt-Tab.
pub fn make_non_activating(win: &WebviewWindow) {
    let Some(hwnd) = hwnd_of(win) else { return };
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let want = ex | WS_EX_NOACTIVATE.0 as isize | WS_EX_TOOLWINDOW.0 as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want);
    }
}

/// Temporarily allow activation so a text field inside the island can be typed in.
pub fn set_activating(win: &WebviewWindow, activating: bool) {
    let Some(hwnd) = hwnd_of(win) else { return };
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let want = if activating {
            ex & !(WS_EX_NOACTIVATE.0 as isize)
        } else {
            ex | WS_EX_NOACTIVATE.0 as isize
        };
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want);
    }
}

/// Click-through here is the poll's WS_EX_TRANSPARENT toggle, not a region.
pub fn set_input_region(_win: &WebviewWindow, _rect: Option<(f64, f64, f64, f64)>) {}

/// Bring an external window (e.g. Hermes Desktop) to the front.
pub fn focus_external_window(title_substring: &str) -> bool {
    use ::windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible, SetForegroundWindow,
        ShowWindow, SW_RESTORE,
    };

    struct FindState {
        needle: String,
        found_hwnd: Option<HWND>,
    }

    let mut state = FindState {
        needle: title_substring.to_lowercase(),
        found_hwnd: None,
    };

    unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = &mut *(lparam.0 as *mut FindState);
        if !IsWindowVisible(hwnd).as_bool() {
            return true.into();
        }
        let len = GetWindowTextLengthW(hwnd);
        if len > 0 {
            let mut buf = vec![0u16; (len + 1) as usize];
            let read = GetWindowTextW(hwnd, &mut buf);
            if read > 0 {
                let title = String::from_utf16_lossy(&buf[..read as usize]).to_lowercase();
                if title.contains(&state.needle) {
                    state.found_hwnd = Some(hwnd);
                    return false.into(); // stop search
                }
            }
        }
        true.into()
    }

    unsafe {
        let _ = EnumWindows(Some(enum_cb), LPARAM(&mut state as *mut FindState as isize));
        if let Some(hwnd) = state.found_hwnd {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
            return true;
        }
    }
    false
}

// ── Global Hotkey ─────────────────────────────────────────────────────────────

use std::sync::Mutex as StdMutex;

/// The live hotkey thread, so a preference change can be applied without a
/// restart: only one thread may own the registration at a time.
struct HotkeyState {
    tx: std::sync::mpsc::Sender<(bool, String)>,
    thread_id: u32,
}

static HOTKEY: StdMutex<Option<HotkeyState>> = StdMutex::new(None);

/// Parse a shortcut string like "Ctrl+Shift+C" or "Alt+C" into (HOT_KEY_MODIFIERS, vk).
///
/// A shortcut with no modifier is refused: `RegisterHotKey` would then swallow a
/// bare key system-wide, which is never what somebody wants from a toggle.
fn parse_shortcut(s: &str) -> Option<(HOT_KEY_MODIFIERS, u32)> {
    let mut mods = HOT_KEY_MODIFIERS(MOD_NOREPEAT.0);
    let mut vk: Option<u32> = None;
    let mut has_mod = false;

    for part in s.split('+') {
        let part = part.trim().to_uppercase();
        match part.as_str() {
            "CTRL" | "CONTROL" => {
                mods |= MOD_CONTROL;
                has_mod = true;
            }
            "ALT" | "OPTION" => {
                mods |= MOD_ALT;
                has_mod = true;
            }
            "SHIFT" => {
                mods |= MOD_SHIFT;
                has_mod = true;
            }
            "WIN" | "SUPER" | "META" => {
                mods |= MOD_WIN;
                has_mod = true;
            }
            "SPACE" => vk = Some(VK_SPACE.0 as u32),
            "ESC" | "ESCAPE" => vk = Some(0x1B),
            "ENTER" | "RETURN" => vk = Some(0x0D),
            "TAB" => vk = Some(0x09),
            p if p.len() == 1 => {
                let ch = p.chars().next()?;
                if ch.is_ascii_alphanumeric() {
                    vk = Some(ch as u32);
                }
            }
            p if p.starts_with('F') && p.len() >= 2 => {
                if let Ok(num) = p[1..].parse::<u32>() {
                    if (1..=24).contains(&num) {
                        vk = Some(0x70 + num - 1);
                    }
                }
            }
            _ => {}
        }
    }

    if !has_mod {
        return None;
    }
    vk.map(|k| (mods, k))
}

/// Starts the hotkey thread. Safe to call once at boot; later changes go through
/// `update_global_hotkey`.
pub fn init_global_hotkey(app: AppHandle, enabled: bool, shortcut: String) {
    let (tx, rx) = std::sync::mpsc::channel::<(bool, String)>();
    let thread_id = unsafe { GetCurrentThreadId() };

    std::thread::spawn(move || {
        let mut registered: Option<i32> = None;
        const HOTKEY_ID: i32 = 1010;

        let mut apply = |en: bool, sc: &str| {
            if let Some(id) = registered.take() {
                unsafe {
                    let _ = UnregisterHotKey(None, id);
                }
            }
            if en {
                if let Some((mods, vk)) = parse_shortcut(sc) {
                    match unsafe { RegisterHotKey(None, HOTKEY_ID, mods, vk) } {
                        Ok(()) => registered = Some(HOTKEY_ID),
                        Err(err) => crate::log::line(format!("hotkey {sc} unavailable: {err}")),
                    }
                } else {
                    crate::log::line(format!("hotkey {sc} could not be parsed"));
                }
            }
        };

        apply(enabled, &shortcut);

        loop {
            // Drain pending config changes first; a preference edit arrives as a
            // channel message plus a wake-up post so this thread unblocks at once.
            while let Ok((en, sc)) = rx.try_recv() {
                apply(en, &sc);
            }

            // GetMessageW blocks with nothing to do, so an idle Coucou costs no CPU.
            let mut msg = MSG::default();
            unsafe {
                if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    break;
                }
            }
            if msg.message == WM_HOTKEY {
                // The island page owns what "toggle" means.
                let _ = app.emit_to(WINDOW_LABEL, "hotkey-toggle", ());
                continue;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });

    *HOTKEY.lock().unwrap() = Some(HotkeyState { tx, thread_id });
}

/// Applies a preference change to the already-running hotkey thread.
pub fn update_global_hotkey(enabled: bool, shortcut: &str) {
    let guard = HOTKEY.lock().unwrap();
    let Some(state) = guard.as_ref() else { return };
    let _ = state.tx.send((enabled, shortcut.to_string()));
    // The thread is parked inside GetMessageW; post a message so it wakes up and
    // drains the channel now rather than at some arbitrary later press.
    unsafe {
        let _ = PostThreadMessageW(state.thread_id, WM_APP + 1, WPARAM(0), LPARAM(0));
    }
}
