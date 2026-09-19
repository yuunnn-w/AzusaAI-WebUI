//! 剪贴板（§7.1「复制 Base URL」）。
//!
//! 只做一件小事：把一段文本以 `CF_UNICODETEXT` 放进剪贴板。失败（被其它程序占用）⇒ `false`，
//! 由调用方给可见反馈（不静默失败）。

use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

/// `CF_UNICODETEXT = 13`。windows crate 把它放在 `Win32_System_Ole`（`CLIPBOARD_FORMAT(13)`）——
/// 为这一个常量开整个 Ole feature 面不划算（多引 DLL 面孔），故此处用等值常量 + 本注释说明来源。
const CF_UNICODETEXT: u32 = 13;

/// 写文本到剪贴板；成功 ⇒ `true`（此后内存归系统所有，**不可**再释放）。
pub fn set_text(hwnd: HWND, text: &str) -> bool {
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return false;
        }
        let result = copy_locked(text);
        let _ = CloseClipboard();
        result
    }
}

unsafe fn copy_locked(text: &str) -> bool {
    unsafe {
        if EmptyClipboard().is_err() {
            return false;
        }
        let mut units: Vec<u16> = text.encode_utf16().collect();
        units.push(0);
        let bytes = units.len() * std::mem::size_of::<u16>();
        let memory = match GlobalAlloc(GMEM_MOVEABLE, bytes) {
            Ok(memory) => memory,
            Err(_) => return false,
        };
        let target = GlobalLock(memory) as *mut u16;
        if target.is_null() {
            // 锁不到 ⇒ 系统不会接管这块内存，必须自己收（审查 P2-9：原来这路径会泄漏）
            let _ = GlobalFree(Some(memory));
            return false;
        }
        std::ptr::copy_nonoverlapping(units.as_ptr(), target, units.len());
        let _ = GlobalUnlock(memory);
        if SetClipboardData(CF_UNICODETEXT, Some(HANDLE(memory.0))).is_ok() {
            true // 成功 ⇒ 内存所有权归系统（**不可**再释放）
        } else {
            // 未接管 ⇒ 自己释放（审查 P2-9）
            let _ = GlobalFree(Some(memory));
            false
        }
    }
}
