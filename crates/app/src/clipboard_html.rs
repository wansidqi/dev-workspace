//! Adds an HTML flavour to whatever the clipboard already holds, so a
//! paste into Google Sheets or Excel keeps borders and bold text while a
//! paste into a plain editor still gets the text.
//!
//! GPUI's clipboard only carries plain text, so on Windows this calls the
//! clipboard API directly (declared by hand: no extra crate for four calls).
//! Other platforms keep plain text only.

/// Adds `html` (a fragment) as the "HTML Format" entry of the clipboard.
/// Call after the plain text was written. Returns whether it was added.
#[cfg(windows)]
pub fn add_html(html: &str) -> bool {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "user32")]
    extern "system" {
        fn OpenClipboard(owner: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn RegisterClipboardFormatW(name: *const u16) -> u32;
        fn SetClipboardData(format: u32, mem: *mut c_void) -> *mut c_void;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
        fn GlobalLock(mem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(mem: *mut c_void) -> i32;
        fn GlobalFree(mem: *mut c_void) -> *mut c_void;
    }
    const GMEM_MOVEABLE: u32 = 0x0002;

    let mut bytes = devspace_core::cf_html(html);
    bytes.push(0);
    let name: Vec<u16> = std::ffi::OsStr::new("HTML Format")
        .encode_wide()
        .chain(Some(0))
        .collect();

    // SAFETY: plain Win32 calls following their documented contracts. The
    // global block is owned by us until SetClipboardData succeeds (then the
    // system owns it) and is freed on every failure path; the clipboard is
    // always closed after a successful open.
    unsafe {
        // Another process can hold the clipboard for a moment.
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(std::ptr::null_mut()) != 0 {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if !opened {
            return false;
        }
        let format = RegisterClipboardFormatW(name.as_ptr());
        let mut added = false;
        if format != 0 {
            let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
            if !mem.is_null() {
                let dst = GlobalLock(mem) as *mut u8;
                if dst.is_null() {
                    GlobalFree(mem);
                } else {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
                    GlobalUnlock(mem);
                    if SetClipboardData(format, mem).is_null() {
                        GlobalFree(mem);
                    } else {
                        added = true;
                    }
                }
            }
        }
        CloseClipboard();
        added
    }
}

#[cfg(not(windows))]
pub fn add_html(_html: &str) -> bool {
    false
}

#[cfg(all(test, windows))]
mod tests {
    /// Touches the real clipboard, so it only runs on request:
    /// `cargo test clipboard_html -- --ignored`.
    #[test]
    #[ignore]
    fn html_flavour_lands_on_the_clipboard() {
        assert!(super::add_html("<table><tr><td>hello</td></tr></table>"));
    }
}
