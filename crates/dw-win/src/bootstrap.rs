//! Finds the engine's two global tables (FNamePool and the UObject array) by
//! shape, scanning the game's writable sections. No fixed address and no code
//! signature: the shapes below hold across builds of the same engine version.

use dw_core::{Layout, Memory, MemoryExt, Reflection};

/// One PE section of the loaded game image.
pub struct Section {
    pub start: u64,
    pub size: u64,
    pub writable: bool,
    pub executable: bool,
}

const SCN_EXECUTE: u32 = 0x2000_0000;
const SCN_WRITE: u32 = 0x8000_0000;

/// Sections of the image at `base`, from its PE headers.
pub fn sections(mem: &impl Memory, base: u64) -> Vec<Section> {
    let mut out = Vec::new();
    let Some(nt) = mem.read_u32(base + 0x3c).map(|v| base + v as u64) else { return out };
    if mem.read_u32(nt) != Some(0x4550) {
        return out; // "PE\0\0"
    }
    let count = mem.read_u16(nt + 6).unwrap_or(0) as u64;
    let opt_size = mem.read_u16(nt + 20).unwrap_or(0) as u64;
    let table = nt + 24 + opt_size;
    for i in 0..count.min(96) {
        let h = table + i * 40;
        let (Some(vsize), Some(va), Some(flags)) = (mem.read_u32(h + 8), mem.read_u32(h + 12), mem.read_u32(h + 36))
        else {
            continue;
        };
        out.push(Section {
            start: base + va as u64,
            size: vsize as u64,
            writable: flags & SCN_WRITE != 0,
            executable: flags & SCN_EXECUTE != 0,
        });
    }
    out
}

/// Copy of a section; unreadable pages stay zero.
fn snapshot(mem: &impl Memory, s: &Section) -> Vec<u8> {
    const PAGE: usize = 0x1000;
    let mut buf = vec![0u8; s.size as usize];
    for (i, chunk) in buf.chunks_mut(PAGE).enumerate() {
        let _ = mem.read(s.start + (i * PAGE) as u64, chunk);
    }
    buf
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn plausible_ptr(p: u64) -> bool {
    p > 0x10000 && p < 0x0000_7fff_ffff_0000
}

/// The first two FName entries are always "None" and "ByteProperty".
fn is_name_pool(mem: &impl Memory, l: &Layout, buf: &[u8], off: usize) -> bool {
    let cur_block = u32_at(buf, off + l.names_current_block as usize);
    let block0 = u64_at(buf, off + l.names_blocks as usize);
    if cur_block >= 8192 || !plausible_ptr(block0) {
        return false;
    }
    let entry = |at: u64, text: &str| -> Option<u64> {
        let hdr = mem.read_u16(at)?;
        if hdr & 1 != 0 || (hdr >> 6) as usize != text.len() {
            return None;
        }
        let mut s = vec![0u8; text.len()];
        (mem.read(at + 2, &mut s) && s == text.as_bytes()).then(|| {
            let next = at + 2 + text.len() as u64;
            next.next_multiple_of(l.name_stride)
        })
    };
    entry(block0, "None").and_then(|next| entry(next, "ByteProperty")).is_some()
}

/// Addresses of the FNamePool and of FChunkedFixedUObjectArray, once the engine
/// has registered its first objects (object 0 is the "/Script/CoreUObject" package).
pub fn find(mem: &impl Memory, l: &Layout, base: u64) -> Option<(u64, u64)> {
    let snaps: Vec<(u64, Vec<u8>)> = sections(mem, base)
        .iter()
        .filter(|s| s.writable && !s.executable && s.size >= 0x100)
        .map(|s| (s.start, snapshot(mem, s)))
        .collect();

    let names = snaps.iter().find_map(|(start, buf)| {
        (0..buf.len().saturating_sub(0x100))
            .step_by(8)
            .find(|&o| is_name_pool(mem, l, buf, o))
            .map(|o| start + o as u64)
    })?;

    let objects = snaps.iter().find_map(|(start, buf)| {
        (0..buf.len().saturating_sub(0x40)).step_by(8).find_map(|o| {
            let num = u32_at(buf, o + l.objects_num as usize) as i32;
            let max = u32_at(buf, o + l.objects_max as usize) as i32;
            if num < 1000 || num > max || !plausible_ptr(u64_at(buf, o + l.objects_chunks as usize)) {
                return None;
            }
            let at = start + o as u64;
            let r = Reflection::new(mem, *l, at, names);
            let first = r.object_at(0)?;
            (r.object_count().is_some() && r.object_name(first).as_deref() == Some("/Script/CoreUObject")).then_some(at)
        })
    })?;

    Some((objects, names))
}
