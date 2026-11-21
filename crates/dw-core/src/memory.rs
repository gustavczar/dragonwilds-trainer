//! Read access to the game's address space.

/// Read-only view of the target address space.
///
/// Implementations must return `false` for any unreadable range and must never
/// panic or fault: the reflection walker feeds it addresses taken from game
/// memory, which may be stale or garbage.
pub trait Memory {
    /// Copy `out.len()` bytes starting at `addr` into `out`.
    fn read(&self, addr: u64, out: &mut [u8]) -> bool;
}

/// Typed little-endian reads on top of [`Memory`].
pub trait MemoryExt: Memory {
    fn read_u8(&self, addr: u64) -> Option<u8> {
        let mut b = [0u8; 1];
        self.read(addr, &mut b).then_some(b[0])
    }
    fn read_u16(&self, addr: u64) -> Option<u16> {
        let mut b = [0u8; 2];
        self.read(addr, &mut b).then(|| u16::from_le_bytes(b))
    }
    fn read_u32(&self, addr: u64) -> Option<u32> {
        let mut b = [0u8; 4];
        self.read(addr, &mut b).then(|| u32::from_le_bytes(b))
    }
    fn read_i32(&self, addr: u64) -> Option<i32> {
        self.read_u32(addr).map(|v| v as i32)
    }
    fn read_u64(&self, addr: u64) -> Option<u64> {
        let mut b = [0u8; 8];
        self.read(addr, &mut b).then(|| u64::from_le_bytes(b))
    }
    fn read_f32(&self, addr: u64) -> Option<f32> {
        self.read_u32(addr).map(f32::from_bits)
    }
}

impl<M: Memory + ?Sized> MemoryExt for M {}
