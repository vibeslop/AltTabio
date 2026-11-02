//! Bounded Unicode clipboard transfers on the UI thread, outside every input hook callback.

use windows::Win32::Foundation::{
    ERROR_SUCCESS, GetLastError, HANDLE, HGLOBAL, HWND, SetLastError,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::core::{BOOL, Error, Result};

const MAX_TEXT_BYTES: usize = 128 * 1024;

// These two APIs use zero for a successful release, unlike the generated Result wrappers.
#[link(name = "kernel32")]
unsafe extern "system" {
    #[link_name = "GlobalFree"]
    fn free_global(handle: HGLOBAL) -> HGLOBAL;
    #[link_name = "GlobalUnlock"]
    fn unlock_global(handle: HGLOBAL) -> BOOL;
}

struct Clipboard;
impl Clipboard {
    fn open(owner: HWND) -> Result<Self> {
        // SAFETY: owner is the live overlay HWND, retained until this guard closes the clipboard.
        unsafe {
            OpenClipboard(Some(owner))?;
        }
        Ok(Self)
    }
}
impl Drop for Clipboard {
    fn drop(&mut self) {
        // SAFETY: this UI thread successfully opened the clipboard and closes it exactly once.
        if let Err(error) = unsafe { CloseClipboard() } {
            eprintln!("Could not close the clipboard: {error}");
        }
    }
}

struct Memory(Option<HGLOBAL>);
impl Drop for Memory {
    fn drop(&mut self) {
        if let Some(handle) = self.0 {
            // SAFETY: this guard owns the allocation unless SetClipboardData took ownership.
            if !unsafe { free_global(handle) }.is_invalid() {
                eprintln!(
                    "Could not release clipboard memory: {}",
                    Error::from_thread()
                );
            }
        }
    }
}

struct Locked(HGLOBAL);
impl Drop for Locked {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: reset this thread's last error to distinguish the documented successful
            // final unlock, which returns zero, from a failed unlock that sets an error.
            SetLastError(ERROR_SUCCESS);
        }
        // SAFETY: each guard balances one successful lock before the clipboard or memory is freed.
        let still_locked = unsafe { unlock_global(self.0) };
        // SAFETY: this scalar read immediately follows the unlock on the same thread.
        if !still_locked.as_bool() && unsafe { GetLastError() } != ERROR_SUCCESS {
            eprintln!(
                "Could not unlock clipboard memory: {}",
                Error::from_thread()
            );
        }
    }
}

pub fn read(owner: HWND) -> Result<String> {
    let _clipboard = Clipboard::open(owner)?;
    // SAFETY: querying available formats retains no pointers or memory ownership.
    if unsafe { IsClipboardFormatAvailable(u32::from(CF_UNICODETEXT.0)) }.is_err() {
        return Ok(String::new());
    }
    let handle = unsafe {
        // SAFETY: the clipboard remains open, and this returned global handle is borrowed only.
        GetClipboardData(u32::from(CF_UNICODETEXT.0))?
    };
    let global = HGLOBAL(handle.0);
    let bytes = unsafe {
        // SAFETY: global is the borrowed clipboard allocation retained while the clipboard is open.
        GlobalSize(global)
    };
    let pointer = unsafe {
        // SAFETY: the borrowed allocation remains live until _clipboard drops.
        GlobalLock(global)
    };
    if pointer.is_null() {
        return Err(Error::from_thread());
    }
    let _lock = Locked(global);
    let words = unsafe {
        // SAFETY: GlobalSize bounds this aligned UTF-16 read, further capped for bounded UI work.
        std::slice::from_raw_parts(pointer.cast::<u16>(), bytes.min(MAX_TEXT_BYTES) / 2)
    };
    let end = words
        .iter()
        .position(|word| *word == 0)
        .unwrap_or(words.len());
    Ok(String::from_utf16_lossy(&words[..end]))
}

pub fn write(owner: HWND, text: &str) -> Result<()> {
    let words = text.encode_utf16().chain([0]).collect::<Vec<_>>();
    let handle = unsafe {
        // SAFETY: the bounded query's UTF-16 buffer determines this movable allocation's size.
        GlobalAlloc(GMEM_MOVEABLE, words.len() * 2)?
    };
    let mut memory = Memory(Some(handle));
    let pointer = unsafe {
        // SAFETY: this guard owns the freshly allocated movable block.
        GlobalLock(handle)
    };
    if pointer.is_null() {
        return Err(Error::from_thread());
    }
    let lock = Locked(handle);
    unsafe {
        // SAFETY: the nonoverlapping source and destination both contain words.len() UTF-16 units.
        std::ptr::copy_nonoverlapping(words.as_ptr(), pointer.cast::<u16>(), words.len());
    }
    drop(lock);
    let _clipboard = Clipboard::open(owner)?;
    unsafe {
        // SAFETY: this thread owns the open clipboard. On success the system takes this unlocked
        // movable allocation, and memory's ownership is cleared before any later cleanup.
        EmptyClipboard()?;
        SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(handle.0)))?;
    }
    memory.0 = None;
    Ok(())
}
