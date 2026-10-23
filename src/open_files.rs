//! Receives the files macOS asks the app to open (Finder double-click, Open With, `open -a`).
//!
//! macOS doesn't put them on the command line. AppKit calls `application:openURLs:` on the
//! application delegate instead, at launch and while running. winit 0.30 owns that delegate
//! and doesn't implement the method, so it's added to winit's delegate class. This is what
//! Slint's own editor does until Slint has an API for it (slint-ui/slint#13283).

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use objc2::MainThreadMarker;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::sel;
use objc2_app_kit::NSApplication;
use objc2_foundation::{NSArray, NSURL};

type OpenUrls = extern "C-unwind" fn(&AnyObject, Sel, &NSApplication, &NSArray<NSURL>);

static ON_OPEN: OnceLock<fn(PathBuf)> = OnceLock::new();

/// Calls `on_open` with the first file of every open request.
///
/// Call after the Slint window exists (that creates winit's delegate) and before the event
/// loop runs, or the file that launched the app is missed.
pub fn install(on_open: fn(PathBuf)) {
    let mtm = MainThreadMarker::new().expect("called on the main thread");
    let Some(delegate) = NSApplication::sharedApplication(mtm).delegate() else {
        eprintln!("viewer: no application delegate; files opened from Finder are ignored");
        return;
    };
    let delegate: &AnyObject = delegate.as_ref();
    let class: *const AnyClass = delegate.class();
    // SAFETY: `open_urls` matches the `v@:@@` encoding of `application:openURLs:`.
    // Adding a method to a registered class is allowed and never replaces an existing one.
    let added = unsafe {
        objc2::ffi::class_addMethod(
            class.cast_mut(),
            sel!(application:openURLs:),
            std::mem::transmute::<OpenUrls, Imp>(open_urls),
            c"v@:@@".as_ptr(),
        )
    };
    if !added.as_bool() {
        eprintln!("viewer: the application delegate already handles application:openURLs:");
        return;
    }
    let _ = ON_OPEN.set(on_open);
}

extern "C-unwind" fn open_urls(
    _delegate: &AnyObject,
    _cmd: Sel,
    _application: &NSApplication,
    urls: &NSArray<NSURL>,
) {
    let Some(path) = urls.iter().find_map(|url| url.to_file_path()) else {
        return;
    };
    // At launch AppKit calls this from inside `finishLaunching`, before winit has started.
    slint::Timer::single_shot(Duration::ZERO, move || {
        if let Some(on_open) = ON_OPEN.get() {
            on_open(path);
        }
    });
}
