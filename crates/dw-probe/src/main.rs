//! dw-probe: read-only inspection of a running RuneScape: Dragonwilds.
//!
//! Opens the game with PROCESS_VM_READ only, so it cannot change anything.
//! The two table offsets come from the trainer log line
//! "engine found ...: objects=+0x... names=+0x...".
//!
//! Usage:
//!   dw-probe <objects> <names> player              local player chain and its object fields
//!   dw-probe <objects> <names> fields <hex addr>   object fields of any object
//!   dw-probe <objects> <names> slots <hex addr> [from to]   ItemSlots of an inventory component
//!   dw-probe <objects> <names> class <Name>        properties and functions of a class

use dw_core::{Layout, Memory, MemoryExt, Reflection};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Module32FirstW, Process32FirstW, Process32NextW, MODULEENTRY32W, PROCESSENTRY32W,
    TH32CS_SNAPMODULE, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ};

const EXE: &str = "RSDragonwilds-Win64-Shipping.exe";

struct External(HANDLE);

impl Memory for External {
    fn read(&self, addr: u64, out: &mut [u8]) -> bool {
        let mut done = 0usize;
        // SAFETY: the handle only has VM_READ; the call validates the remote range.
        let ok = unsafe { ReadProcessMemory(self.0, addr as _, out.as_mut_ptr() as _, out.len(), &mut done) };
        ok != 0 && done == out.len()
    }
}

fn wide(s: &[u16]) -> String {
    String::from_utf16_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())])
}

/// (pid, base address of the game executable)
fn find_game() -> Option<(u32, u64)> {
    // SAFETY: Toolhelp snapshots with properly sized entry structs.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut pid = None;
        let mut ok = Process32FirstW(snap, &mut e);
        while ok != 0 {
            if wide(&e.szExeFile).eq_ignore_ascii_case(EXE) {
                pid = Some(e.th32ProcessID);
                break;
            }
            ok = Process32NextW(snap, &mut e);
        }
        CloseHandle(snap);
        let Some(pid) = pid else {
            eprintln!("{EXE} not found in the process list");
            return None;
        };
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid);
        if snap == INVALID_HANDLE_VALUE {
            eprintln!("module snapshot of pid {pid} failed: {}", std::io::Error::last_os_error());
            return None;
        }
        let mut m: MODULEENTRY32W = std::mem::zeroed();
        m.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
        let base = (Module32FirstW(snap, &mut m) != 0).then(|| m.modBaseAddr as u64);
        CloseHandle(snap);
        Some((pid, base?))
    }
}

fn hex(s: &str) -> u64 {
    u64::from_str_radix(s.trim_start_matches("+").trim_start_matches("0x"), 16).expect("hex number")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: dw-probe <objects +0x..> <names +0x..> player|fields <addr>|slots <addr> [from to]|class <Name>");
        std::process::exit(2);
    }
    let (pid, base) = find_game().expect("game not running");
    // SAFETY: read-only access rights.
    let handle = unsafe { OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    assert!(!handle.is_null(), "OpenProcess failed");
    let mem = External(handle);
    let r = Reflection::new(&mem, Layout::default(), base + hex(&args[1]), base + hex(&args[2]));
    let p = Probe { r: &r, mem: &mem };
    match args[3].as_str() {
        "player" => p.player(),
        "fields" => p.fields(hex(&args[4])),
        "slots" => {
            let from = args.get(5).map_or(0, |v| v.parse().unwrap());
            let to = args.get(6).map_or(200, |v| v.parse().unwrap());
            p.slots(hex(&args[4]), from, to)
        }
        "class" => p.class(&args[4]),
        other => eprintln!("unknown command {other}"),
    }
}

struct Probe<'a> {
    r: &'a Reflection<'a, External>,
    mem: &'a External,
}

impl Probe<'_> {
    fn label(&self, obj: u64) -> String {
        let class = self.r.class_of(obj).and_then(|c| self.r.object_name(c)).unwrap_or("?".into());
        let name = self.r.object_name(obj).unwrap_or("?".into());
        format!("{obj:#x} {name} ({class})")
    }

    fn prop(&self, class: &str, name: &str) -> u64 {
        let c = self.r.find_type(class).unwrap_or_else(|| panic!("class {class}"));
        self.r.find_property(c, name).unwrap_or_else(|| panic!("{class}.{name}")).offset as u64
    }

    fn ptr(&self, obj: u64, off: u64) -> Option<u64> {
        let v = self.mem.read_u64(obj + off)?;
        self.r.is_object(v).then_some(v)
    }

    fn player(&self) {
        let engine = *self.r.instances_of("GameEngine").first().expect("GameEngine");
        let chain = [
            ("Engine", "GameViewport"),
            ("GameViewportClient", "World"),
            ("World", "OwningGameInstance"),
        ];
        let mut cur = engine;
        println!("engine {}", self.label(cur));
        for (c, f) in chain {
            cur = self.ptr(cur, self.prop(c, f)).expect(f);
            println!("{f:20} {}", self.label(cur));
        }
        let players = self.mem.read_u64(cur + self.prop("GameInstance", "LocalPlayers")).unwrap();
        let lp = self.ptr(players, 0).expect("LocalPlayers[0]");
        let pc = self.ptr(lp, self.prop("Player", "PlayerController")).expect("PlayerController");
        println!("{:20} {}", "PlayerController", self.label(pc));
        match self.ptr(pc, self.prop("Controller", "Pawn")) {
            Some(pawn) => {
                println!("{:20} {}", "Pawn", self.label(pawn));
                println!("\n== pawn fields");
                self.fields(pawn);
            }
            None => println!("no pawn (not in a world?)"),
        }
        println!("\n== controller fields");
        self.fields(pc);
    }

    /// Every ObjectProperty of `obj` (class chain) that holds a live object.
    fn fields(&self, obj: u64) {
        let mut c = self.r.class_of(obj);
        while let Some(cls) = c {
            let cname = self.r.object_name(cls).unwrap_or_default();
            for p in self.r.properties(cls) {
                if p.type_name == "ObjectProperty" {
                    if let Some(v) = self.ptr(obj, p.offset as u64) {
                        println!("  {cname}.{} @{:#x} -> {}", p.name, p.offset, self.label(v));
                    }
                }
            }
            c = self.r.super_of(cls);
        }
    }

    fn slots(&self, inv: u64, from: usize, to: usize) {
        let cls = self.r.class_of(inv).expect("not an object");
        let arr = self.r.find_property(cls, "ItemSlots").expect("no ItemSlots");
        let data = self.mem.read_u64(inv + arr.offset as u64).unwrap();
        let num = self.mem.read_i32(inv + arr.offset as u64 + 8).unwrap();
        println!("{} ItemSlots @{:#x} num={num}", self.label(inv), arr.offset);
        let (count, idata, max) = (self.prop("Item", "Count"), self.prop("Item", "ItemData"), self.prop("ItemData", "MaxStackSize"));
        for i in from..to.min(num.max(0) as usize) {
            let Some(raw) = self.mem.read_u64(data + i as u64 * 8) else { continue };
            if raw == 0 {
                continue;
            }
            if !self.r.is_object(raw) {
                println!("  [{i:3}] {raw:#x} NOT AN OBJECT");
                continue;
            }
            let n = self.mem.read_i32(raw + count).unwrap_or(-1);
            let d = self.ptr(raw, idata);
            let m = d.and_then(|d| self.mem.read_i32(d + max)).unwrap_or(-1);
            let dn = d.and_then(|d| self.r.object_name(d)).unwrap_or("?".into());
            println!("  [{i:3}] {} count={n} max={m} data={dn}", self.label(raw));
        }
    }

    fn class(&self, name: &str) {
        let mut c = self.r.find_type(name);
        while let Some(cls) = c {
            println!("== {}", self.r.object_name(cls).unwrap_or_default());
            for p in self.r.properties(cls) {
                println!("  P {} {} @{:#x}", p.name, p.type_name, p.offset);
            }
            for f in self.r.functions(cls, 0) {
                let ps: Vec<String> = f.params.iter().map(|p| format!("{} {}@{:#x}", p.type_name, p.name, p.offset)).collect();
                println!("  F {}({}) flags={:#x}", f.name, ps.join(", "), f.flags);
            }
            c = self.r.super_of(cls);
        }
    }
}
