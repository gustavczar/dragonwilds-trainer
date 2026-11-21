//! `Memory` over the trainer's own process.

use dw_core::Memory;
use windows_sys::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// Reads go through ReadProcessMemory on our own process: an unmapped or
/// protected address returns `false` instead of crashing the game.
pub struct InProcess;

impl Memory for InProcess {
    fn read(&self, addr: u64, out: &mut [u8]) -> bool {
        if out.is_empty() || addr < 0x10000 {
            return false;
        }
        let mut done = 0usize;
        // SAFETY: ReadProcessMemory validates the source range itself; `out` is a
        // valid, exclusively borrowed buffer of the given length.
        let ok = unsafe {
            ReadProcessMemory(GetCurrentProcess(), addr as _, out.as_mut_ptr() as _, out.len(), &mut done)
        };
        ok != 0 && done == out.len()
    }
}

/// Writes `value` at `addr` in our own process; `false` if the page is not writable.
pub fn write<T: Copy>(addr: u64, value: T) -> bool {
    let mut done = 0usize;
    // SAFETY: WriteProcessMemory validates the destination range; the source is a
    // local value of exactly size_of::<T>() bytes.
    let ok = unsafe {
        WriteProcessMemory(GetCurrentProcess(), addr as _, &value as *const T as _, std::mem::size_of::<T>(), &mut done)
    };
    ok != 0 && done == std::mem::size_of::<T>()
}
