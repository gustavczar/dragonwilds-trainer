//! The one hook the trainer installs: `UObject::ProcessEvent`, the engine's
//! entry point for every reflected function call. It gives us (1) a regular
//! callback on the game thread, where touching game objects is safe, and (2)
//! the original function, to call game functions such as `SetIsImmortal`.

use crate::bootstrap::Section;
use dw_core::{FunctionInfo, Memory, MemoryExt};
use hudhook::mh::{MhHook, MH_ApplyQueued, MH_Initialize, MH_STATUS};
use std::cell::Cell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowThreadProcessId};

type ProcessEvent = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void);

static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static GAME_THREAD: AtomicU32 = AtomicU32::new(0);
static LAST_TICK: AtomicU64 = AtomicU64::new(0);
/// Game-thread calls seen by the detour; proves the hook is live.
pub static CALLS: AtomicU64 = AtomicU64::new(0);
const TICK_MS: u64 = 250;

thread_local! {
    static IN_TICK: Cell<bool> = const { Cell::new(false) };
}

/// True when `code` tests FUNC_Native (0x400) on UFunction::FunctionFlags at
/// `flags`: `test dword [r+flags], 0x400`, `test byte [r+flags+1], 4` or
/// `bt dword [r+flags], 10`. ProcessEvent is the first UObject virtual that does.
fn tests_native_flag(code: &[u8], flags: u32) -> bool {
    let d = flags.to_le_bytes();
    let d1 = (flags + 1).to_le_bytes();
    code.windows(10).any(|w| w[0] == 0xF7 && w[2..6] == d && w[6..10] == [0, 4, 0, 0])
        || code.windows(7).any(|w| w[0] == 0xF6 && w[2..6] == d1 && w[6] == 0x04)
        || code.windows(8).any(|w| w[0] == 0x0F && w[1] == 0xBA && w[3..7] == d && w[7] == 0x0A)
}

/// (vtable index, address) of ProcessEvent, found from any UObject's vtable.
pub fn find_process_event(mem: &impl Memory, object: u64, code: &[Section], flags: u64) -> Option<(usize, u64)> {
    let vtable = mem.read_u64(object)?;
    let in_code = |a: u64| code.iter().any(|s| s.executable && a >= s.start && a < s.start + s.size);
    (0..0x180usize).find_map(|i| {
        let f = mem.read_u64(vtable + i as u64 * 8)?;
        if !in_code(f) {
            return None;
        }
        let mut body = [0u8; 0x400];
        (mem.read(f, &mut body) && tests_native_flag(&body, flags as u32) && body.windows(2).any(|w| w == [0x0F, 0x85]))
            .then_some((i, f))
    })
}

/// Hooks ProcessEvent at `target`. The game keeps working exactly as before:
/// the detour always calls the original first.
pub fn install(target: u64) -> Result<(), MH_STATUS> {
    // SAFETY: MinHook patches `target`, a function we located in the game's code;
    // the detour has the exact ProcessEvent signature.
    unsafe {
        let st = MH_Initialize();
        if !matches!(st, MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED) {
            return Err(st);
        }
        let hook = MhHook::new(target as *mut c_void, detour as *mut c_void)?;
        ORIGINAL.store(hook.trampoline() as usize, Ordering::Release);
        hook.queue_enable()?;
        MH_ApplyQueued().ok_context("MH_ApplyQueued")?;
        // `hook` has no Drop: the patch stays installed for the life of the process.
    }
    Ok(())
}

unsafe extern "system" fn detour(obj: *mut c_void, func: *mut c_void, params: *mut c_void) {
    // SAFETY: ORIGINAL is set before the hook is enabled.
    let original: ProcessEvent = unsafe { std::mem::transmute(ORIGINAL.load(Ordering::Acquire)) };
    unsafe { original(obj, func, params) };

    // SAFETY: plain Win32 getters.
    if unsafe { GetCurrentThreadId() } != GAME_THREAD.load(Ordering::Relaxed) {
        return;
    }
    CALLS.fetch_add(1, Ordering::Relaxed);
    let now = unsafe { GetTickCount64() };
    if now.saturating_sub(LAST_TICK.load(Ordering::Relaxed)) < TICK_MS || IN_TICK.get() {
        return;
    }
    LAST_TICK.store(now, Ordering::Relaxed);
    IN_TICK.set(true);
    if std::panic::catch_unwind(crate::game::tick).is_err() {
        crate::log::line("panic in tick; features paused");
        crate::game::pause();
    }
    IN_TICK.set(false);
}

/// Calls game function `f` on `obj` through the original ProcessEvent.
/// Only call from the game thread (i.e. from `game::tick`).
pub fn call(obj: u64, f: &FunctionInfo, params: &mut [u8]) {
    let original = ORIGINAL.load(Ordering::Acquire);
    if original == 0 || obj == 0 || params.len() < params_size(f) {
        return;
    }
    // SAFETY: `obj` and `f.object` were validated through reflection, and the
    // buffer is at least as large as the function's parameter block.
    unsafe {
        let pe: ProcessEvent = std::mem::transmute(original);
        pe(obj as *mut c_void, f.object as *mut c_void, params.as_mut_ptr() as *mut c_void);
    }
}

/// Size of `f`'s parameter block, rounded up to 16.
pub fn params_size(f: &FunctionInfo) -> usize {
    f.params.iter().map(|p| (p.offset.max(0) + p.size.max(0)) as usize).max().unwrap_or(0).max(1).next_multiple_of(16)
}

/// The game thread is the one that owns the game window (class "UnrealWindow").
pub fn find_game_thread() -> bool {
    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> i32 {
        let mut pid = 0u32;
        // SAFETY: valid window handle from EnumWindows and valid out pointers.
        let tid = unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        let mut class = [0u16; 32];
        let n = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) } as usize;
        if pid == unsafe { GetCurrentProcessId() } && String::from_utf16_lossy(&class[..n]) == "UnrealWindow" {
            unsafe { *(found as *mut u32) = tid };
            return 0; // stop
        }
        1
    }
    let mut tid = 0u32;
    // SAFETY: the callback only writes to `tid`, which outlives the call.
    unsafe { EnumWindows(Some(each), &mut tid as *mut u32 as LPARAM) };
    GAME_THREAD.store(tid, Ordering::Relaxed);
    tid != 0
}

#[cfg(test)]
mod tests {
    use super::tests_native_flag;

    #[test]
    fn recognises_the_three_encodings_of_the_native_flag_test() {
        // test dword ptr [rcx+0B0h], 400h
        assert!(tests_native_flag(&[0x90, 0xF7, 0x81, 0xB0, 0, 0, 0, 0, 4, 0, 0], 0xB0));
        // test byte ptr [rbx+0B1h], 4
        assert!(tests_native_flag(&[0xF6, 0x83, 0xB1, 0, 0, 0, 0x04], 0xB0));
        // bt dword ptr [rdi+0B0h], 0Ah
        assert!(tests_native_flag(&[0x0F, 0xBA, 0xA7, 0xB0, 0, 0, 0, 0x0A], 0xB0));
        // same instruction on another field, or another bit: no match
        assert!(!tests_native_flag(&[0xF7, 0x81, 0xB4, 0, 0, 0, 0, 4, 0, 0], 0xB0));
        assert!(!tests_native_flag(&[0xF7, 0x81, 0xB0, 0, 0, 0, 0, 8, 0, 0], 0xB0));
    }
}
