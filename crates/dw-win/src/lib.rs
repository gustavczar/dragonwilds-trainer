//! dragonwilds-trainer, Windows side. Ships as `version.dll` in the game's
//! `Binaries/Win64` folder; every real version.dll export is forwarded (build.rs).

#![cfg(windows)]

mod bootstrap;
mod engine;
mod game;
mod log;
mod mem;
mod proxy;
mod settings;
mod ui;

use dw_core::{Layout, Reflection};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HMODULE, TRUE};
use windows_sys::Win32::System::LibraryLoader::{DisableThreadLibraryCalls, GetModuleFileNameW, GetModuleHandleW};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

static SELF_MODULE: AtomicUsize = AtomicUsize::new(0);

#[no_mangle]
extern "system" fn DllMain(module: HMODULE, reason: u32, _reserved: *mut core::ffi::c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        if !proxy::init() {
            return 0; // without the real version.dll the game cannot run; fail the load
        }
        SELF_MODULE.store(module as usize, Ordering::Relaxed);
        // SAFETY: `module` is our own handle, given by the loader.
        unsafe { DisableThreadLibraryCalls(module) };
        // Only start a thread here: nothing heavy may run under the loader lock.
        std::thread::spawn(|| {
            if std::panic::catch_unwind(run).is_err() {
                log::line("panic caught; trainer stopped, game untouched");
            }
        });
    }
    TRUE
}

/// `<game>/Binaries/Win64/dw-trainer/`, next to this DLL.
pub(crate) fn data_dir() -> PathBuf {
    let mut buf = [0u16; 1024];
    // SAFETY: valid buffer and our own module handle.
    let n = unsafe { GetModuleFileNameW(SELF_MODULE.load(Ordering::Relaxed) as HMODULE, buf.as_mut_ptr(), buf.len() as u32) };
    let dll = PathBuf::from(String::from_utf16_lossy(&buf[..n as usize]));
    dll.parent().map(|p| p.join("dw-trainer")).unwrap_or_else(|| PathBuf::from("dw-trainer"))
}

fn run() {
    let _ = std::fs::create_dir_all(data_dir());
    std::panic::set_hook(Box::new(|info| log::line(&format!("panic: {info}"))));
    log::line(concat!("dragonwilds-trainer ", env!("CARGO_PKG_VERSION"), " loaded"));

    // SAFETY: a null name returns the game executable's own module.
    let base = unsafe { GetModuleHandleW(core::ptr::null()) } as u64;
    let mem = mem::InProcess;
    let layout = Layout::default();

    // The proxy loads before the engine starts: wait for its tables.
    let started = Instant::now();
    let (objects, names) = loop {
        if let Some(found) = bootstrap::find(&mem, &layout, base) {
            break found;
        }
        if started.elapsed() > Duration::from_secs(600) {
            log::line("engine tables not found after 10 min; trainer idle");
            return;
        }
        std::thread::sleep(Duration::from_secs(2));
    };
    log::line(&format!(
        "engine found after {:.1}s: objects=+{:#x} names=+{:#x}",
        started.elapsed().as_secs_f32(),
        objects - base,
        names - base
    ));

    // Let the main menu finish loading before reading every class.
    std::thread::sleep(Duration::from_secs(20));
    let r = Reflection::new(&mem, layout, objects, names);
    report(&r);
    if !data_dir().join("reflection-dump.json").exists() {
        dump(&r, base);
    }

    // Read-only mode: with this file present the trainer only reads and logs.
    if data_dir().join("READONLY").exists() {
        log::line("READONLY present: no hook, no panel");
        return;
    }
    start(&r, objects, names, base);
}

/// Installs the ProcessEvent hook, the weight pass and the in-game panel.
fn start(r: &Reflection<'_, mem::InProcess>, objects: u64, names: u64, base: u64) {
    if let Err(e) = game::init(objects, names) {
        log::line(&format!("init failed: {e}; trainer idle"));
        return;
    }
    let code = bootstrap::sections(&mem::InProcess, base);
    // Object 0 is a UPackage, which does not override ProcessEvent, so its vtable
    // slot is UObject::ProcessEvent itself.
    let Some((index, pe)) = r.object_at(0).and_then(|o| engine::find_process_event(&mem::InProcess, o, &code, r.layout.ufunction_flags))
    else {
        log::line("ProcessEvent not found; trainer idle");
        return;
    };
    log::line(&format!("ProcessEvent: vtable[{index:#x}] = +{:#x}", pe - base));
    while !engine::find_game_thread() {
        std::thread::sleep(Duration::from_secs(1));
    }
    if let Err(e) = engine::install(pe) {
        log::line(&format!("hook failed: {e:?}; trainer idle"));
        return;
    }
    log::line("hook installed");
    std::thread::sleep(Duration::from_secs(5));
    log::line(&format!("game-thread ProcessEvent calls in 5s: {}", engine::CALLS.load(Ordering::Relaxed)));

    std::thread::spawn(|| {
        for n in 0u64.. {
            let _ = std::panic::catch_unwind(game::player_pass);
            if n % 10 == 0 {
                let _ = std::panic::catch_unwind(game::weights_pass);
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });

    let module = hudhook::windows::Win32::Foundation::HINSTANCE(SELF_MODULE.load(Ordering::Relaxed) as *mut _);
    // DX12 only: the game's default RHI. The DX11 and DX12 hooks both patch
    // IDXGISwapChain::Present, so they cannot be installed together.
    let overlay = hudhook::Hudhook::builder()
        .with::<hudhook::hooks::dx12::ImguiDx12Hooks>(ui::Panel::default())
        .with_hmodule(module)
        .build()
        .apply();
    match overlay {
        Ok(()) => log::line("panel ready: press Esc in game"),
        Err(e) => log::line(&format!("panel failed: {e:?}; features still run with saved settings")),
    }
}

/// The classes and members the features rely on, so one log line tells whether
/// a patch renamed or moved anything.
fn report(r: &Reflection<'_, mem::InProcess>) {
    const WANTED: [(&str, &[&str], &[&str]); 7] = [
        ("HealthComponent", &["MaxHealth", "AuthoritativeHealth", "bCanDie"], &["SetIsImmortal", "SetCanDie", "SetHealth"]),
        ("DamageComponent", &["bCanTakeDamage"], &["SetIsInvulnerable", "SetCanTakeDamage"]),
        ("PlayerEncumbranceComponent", &[], &["GetCurrentCarryWeight", "GetMaxCarryWeight", "IsEncumbered"]),
        ("PlayerUtilityMagicComponent", &["LastCastTimeBySpellData"], &[]),
        ("Item", &["Count", "ItemData"], &["SetStackSize"]),
        ("ItemData", &["MaxStackSize", "Weight"], &[]),
        ("InventoryComponent", &["ItemSlots"], &[]),
    ];
    log::line(&format!("objects: {}", r.object_count().unwrap_or(-1)));
    for (class, props, funcs) in WANTED {
        let Some(c) = r.find_type(class) else {
            log::line(&format!("MISSING class {class}"));
            continue;
        };
        for p in props {
            match r.find_property(c, p) {
                Some(i) => log::line(&format!("  {class}.{p} {} @{:#x}", i.type_name, i.offset)),
                None => log::line(&format!("  MISSING {class}.{p}")),
            }
        }
        for f in funcs {
            let ok = r.find_function(c, f).is_some();
            log::line(&format!("  {}{class}::{f}", if ok { "" } else { "MISSING " }));
        }
    }
}

fn dump(r: &Reflection<'_, mem::InProcess>, base: u64) {
    let types = r.dump(&|_| true, base);
    let path = data_dir().join("reflection-dump.json");
    let json = serde_json::json!({ "engine": "5.6", "classes": types });
    match std::fs::write(&path, serde_json::to_vec(&json).unwrap_or_default()) {
        Ok(()) => log::line(&format!("dump: {} types -> {}", types.len(), path.display())),
        Err(e) => log::line(&format!("dump failed: {e}")),
    }
}
