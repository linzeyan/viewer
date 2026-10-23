//! `viewer.exe --register` / `--unregister`: per-user file associations without an installer.
//!
//! Windows 10 and later don't let a program make itself the default. Registering lists Viewer
//! under "Open with" for every supported extension; the user picks "Always" once.

use windows_registry::CURRENT_USER;
use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};

use crate::folder::EXTENSIONS;

const CLASSES: &str = r"Software\Classes";
const PROG_ID: &str = "Viewer.Image";
const APPLICATION: &str = r"Applications\viewer.exe";

/// Handles the registration flags. Returns the exit code when one was given.
pub fn run_cli() -> Option<i32> {
    let flag = std::env::args().nth(1)?;
    let result = match flag.as_str() {
        "--register" => register(),
        "--unregister" => unregister(),
        _ => return None,
    };
    // A release build has no console of its own; borrow the one it was started from.
    // SAFETY: plain Win32 calls with constant arguments.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
        SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, std::ptr::null(), std::ptr::null());
    }
    Some(match result {
        Ok(()) => {
            println!("viewer: {flag} done");
            0
        }
        Err(e) => {
            eprintln!("viewer: {flag} failed: {e}");
            1
        }
    })
}

fn register() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let command = format!("\"{}\" \"%1\"", exe.display());
    let write = || -> windows_registry::Result<()> {
        let prog = CURRENT_USER.create(format!(r"{CLASSES}\{PROG_ID}"))?;
        prog.set_string("", "Image")?;
        prog.create(r"shell\open\command")?.set_string("", &command)?;
        // Gives the "Open with" entry a name instead of "viewer.exe".
        let app = CURRENT_USER.create(format!(r"{CLASSES}\{APPLICATION}"))?;
        app.set_string("FriendlyAppName", "Viewer")?;
        app.create(r"shell\open\command")?.set_string("", &command)?;
        let supported = app.create("SupportedTypes")?;
        for ext in EXTENSIONS {
            supported.set_string(format!(".{ext}"), "")?;
            CURRENT_USER
                .create(format!(r"{CLASSES}\.{ext}\OpenWithProgids"))?
                .set_string(PROG_ID, "")?;
        }
        Ok(())
    };
    write().map_err(|e| e.to_string())
}

/// Removes everything `register` wrote. Missing keys are fine: the goal is that none remain.
fn unregister() -> Result<(), String> {
    let _ = CURRENT_USER.remove_tree(format!(r"{CLASSES}\{PROG_ID}"));
    let _ = CURRENT_USER.remove_tree(format!(r"{CLASSES}\{APPLICATION}"));
    for ext in EXTENSIONS {
        if let Ok(key) = CURRENT_USER.create(format!(r"{CLASSES}\.{ext}\OpenWithProgids")) {
            let _ = key.remove_value(PROG_ID);
        }
    }
    if CURRENT_USER.open(format!(r"{CLASSES}\{PROG_ID}")).is_ok() {
        return Err(format!(r"HKCU\{CLASSES}\{PROG_ID} is still there"));
    }
    Ok(())
}
