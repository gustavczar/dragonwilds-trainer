//! Finds the local player and applies the enabled features. Everything is
//! resolved by name once (`init`); `tick` runs on the game thread 4x a second.

use crate::{engine, log, mem::InProcess, settings};
use dw_core::features::{self, SlotState};
use dw_core::{FunctionInfo, MemoryExt, Reflection, Toggles};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

static MEM: InProcess = InProcess;
static GAME: OnceLock<Game> = OnceLock::new();
static PAUSED: AtomicBool = AtomicBool::new(false);
/// Shown in the panel: the local player was found on the last tick.
pub static PLAYER_FOUND: AtomicBool = AtomicBool::new(false);

/// A spell's last-cast time is written this far in the past (float seconds).
const LONG_AGO: f32 = -1.0e9;
/// Element of TMap<UObject*, float>: key 8 + value 4 + hash links 8, aligned to 8.
const CAST_MAP_STRIDE: u64 = 24;
const CAST_MAP_VALUE: u64 = 8;

struct Offsets {
    viewport: u64,
    world: u64,
    game_instance: u64,
    local_players: u64,
    player_controller: u64,
    pawn: u64,
    item_count: u64,
    item_data: u64,
    max_stack: u64,
    weight: u64,
}

struct Game {
    r: Reflection<'static, InProcess>,
    engine: u64,
    off: Offsets,
    item_class: u64,
    item_data_class: u64,
    set_immortal: Option<FunctionInfo>,
    set_can_die: Option<FunctionInfo>,
    set_stack: Option<FunctionInfo>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Health component we made immortal, if any.
    immortal: Option<u64>,
    /// Original weight of every ItemData we zeroed.
    weights: HashMap<u64, f32>,
    logged: HashSet<String>,
}

fn once(st: &mut State, msg: &str) {
    if st.logged.insert(msg.to_string()) {
        log::line(msg);
    }
}

/// Resolves the engine object, the player pointer chain and the game functions.
pub fn init(objects: u64, names: u64) -> Result<(), String> {
    let r = Reflection::new(&MEM, dw_core::Layout::default(), objects, names);
    let prop = |class: &str, name: &str| -> Result<u64, String> {
        let c = r.find_type(class).ok_or(format!("class {class} not found"))?;
        let p = r.find_property(c, name).ok_or(format!("{class}.{name} not found"))?;
        Ok(p.offset as u64)
    };
    let func = |class: &str, name: &str| r.find_type(class).and_then(|c| r.find_function(c, name));
    let engine = *r.instances_of("GameEngine").first().ok_or("GameEngine instance not found")?;
    let off = Offsets {
        viewport: prop("Engine", "GameViewport")?,
        world: prop("GameViewportClient", "World")?,
        game_instance: prop("World", "OwningGameInstance")?,
        local_players: prop("GameInstance", "LocalPlayers")?,
        player_controller: prop("Player", "PlayerController")?,
        pawn: prop("Controller", "Pawn")?,
        item_count: prop("Item", "Count")?,
        item_data: prop("Item", "ItemData")?,
        max_stack: prop("ItemData", "MaxStackSize")?,
        weight: prop("ItemData", "Weight")?,
    };
    let game = Game {
        item_class: r.find_type("Item").ok_or("class Item not found")?,
        item_data_class: r.find_type("ItemData").ok_or("class ItemData not found")?,
        set_immortal: func("HealthComponent", "SetIsImmortal"),
        set_can_die: func("HealthComponent", "SetCanDie"),
        set_stack: func("Item", "SetStackSize"),
        r,
        engine,
        off,
        state: Mutex::new(State::default()),
    };
    for (name, f) in [("SetIsImmortal", &game.set_immortal), ("SetCanDie", &game.set_can_die), ("SetStackSize", &game.set_stack)] {
        if f.is_none() {
            log::line(&format!("function {name} not found; its feature stays off"));
        }
    }
    GAME.set(game).map_err(|_| "init ran twice".to_string())
}

pub fn pause() {
    PAUSED.store(true, Ordering::Relaxed);
}

impl Game {
    fn ptr(&self, obj: u64, off: u64) -> Option<u64> {
        let p = MEM.read_u64(obj + off)?;
        self.r.is_object(p).then_some(p)
    }

    /// Engine -> viewport -> world -> game instance -> LocalPlayers[0] -> controller -> pawn.
    fn player(&self) -> Option<(u64, u64)> {
        let o = &self.off;
        let world = self.ptr(self.ptr(self.engine, o.viewport)?, o.world)?;
        let gi = self.ptr(world, o.game_instance)?;
        let players = MEM.read_u64(gi + o.local_players)?;
        let lp = self.ptr(players, 0)?;
        let pc = self.ptr(lp, o.player_controller)?;
        Some((pc, self.ptr(pc, o.pawn)?))
    }

    /// The object held by the first ObjectProperty of `owner` (its class or a
    /// super) that points at an instance of `class`.
    fn component(&self, owner: u64, class: &str) -> Option<u64> {
        let mut c = self.r.class_of(owner);
        for _ in 0..32 {
            let cls = c?;
            for p in self.r.properties(cls) {
                if p.type_name == "ObjectProperty" {
                    if let Some(v) = self.ptr(owner, p.offset as u64) {
                        if self.r.is_a(v, class) {
                            return Some(v);
                        }
                    }
                }
            }
            c = self.r.super_of(cls);
        }
        None
    }

    fn is_instance(&self, obj: u64, class: u64) -> bool {
        let mut c = self.r.class_of(obj);
        for _ in 0..32 {
            match c {
                Some(x) if x == class => return true,
                Some(x) => c = self.r.super_of(x),
                None => return false,
            }
        }
        false
    }
}

/// Calls a game function whose first parameter of `kind` gets `value`.
fn call_with(obj: u64, f: &FunctionInfo, kind: &str, value: &[u8]) {
    let mut buf = vec![0u8; engine::params_size(f)];
    let Some(p) = f.params.iter().find(|p| p.type_name == kind) else { return };
    let at = p.offset as usize;
    if at + value.len() <= buf.len() {
        buf[at..at + value.len()].copy_from_slice(value);
        engine::call(obj, f, &mut buf);
    }
}

pub fn tick() {
    let Some(g) = GAME.get() else { return };
    if PAUSED.load(Ordering::Relaxed) {
        return;
    }
    let t = settings::toggles();
    let Ok(mut st) = g.state.lock() else { return };
    let Some((pc, pawn)) = g.player() else {
        PLAYER_FOUND.store(false, Ordering::Relaxed);
        return;
    };
    PLAYER_FOUND.store(true, Ordering::Relaxed);

    health(g, &mut st, &t, pawn);
    if t.no_cooldown {
        cooldowns(g, &mut st, pawn);
    }
    if t.fill_slots || t.free_runes {
        refill(g, &mut st, &t, pc);
    }
}

fn health(g: &Game, st: &mut State, t: &Toggles, pawn: u64) {
    let (Some(immortal), Some(can_die)) = (&g.set_immortal, &g.set_can_die) else { return };
    let Some(hc) = g.component(pawn, "HealthComponent") else {
        once(st, "HealthComponent not found on the player");
        return;
    };
    // Only undo what we did: never clear an immortality the game set itself.
    match (t.infinite_health, st.immortal == Some(hc)) {
        (true, false) => {
            call_with(hc, immortal, "BoolProperty", &[1]);
            call_with(hc, can_die, "BoolProperty", &[0]);
            st.immortal = Some(hc);
            log::line("infinite health on");
        }
        (false, true) => {
            call_with(hc, immortal, "BoolProperty", &[0]);
            call_with(hc, can_die, "BoolProperty", &[1]);
            st.immortal = None;
            log::line("infinite health off");
        }
        _ => {}
    }
}

fn cooldowns(g: &Game, st: &mut State, pawn: u64) {
    let Some(magic) = g.component(pawn, "PlayerUtilityMagicComponent") else {
        once(st, "PlayerUtilityMagicComponent not found on the player");
        return;
    };
    let Some(map) = g.r.class_of(magic).and_then(|c| g.r.find_property(c, "LastCastTimeBySpellData")) else { return };
    let map = magic + map.offset as u64;
    let (Some(data), Some(num)) = (MEM.read_u64(map), MEM.read_i32(map + 8)) else { return };
    for i in 0..num.clamp(0, 512) as u64 {
        let e = data + i * CAST_MAP_STRIDE;
        // Free sparse-array slots hold list links, never a live object: skip them.
        if MEM.read_u64(e).is_some_and(|k| g.r.is_object(k)) {
            crate::mem::write(e + CAST_MAP_VALUE, LONG_AGO);
        }
    }
}

fn refill(g: &Game, st: &mut State, t: &Toggles, pc: u64) {
    let Some(set_stack) = &g.set_stack else { return };
    let Some(inv) = g.component(pc, "InventoryComponent") else {
        once(st, "InventoryComponent not found on the controller");
        return;
    };
    let Some(slots) = g.r.class_of(inv).and_then(|c| g.r.find_property(c, "ItemSlots")) else {
        once(st, "InventoryComponent.ItemSlots not found");
        return;
    };
    let (Some(data), Some(num)) = (MEM.read_u64(inv + slots.offset as u64), MEM.read_i32(inv + slots.offset as u64 + 8)) else {
        return;
    };
    let o = &g.off;
    let wanted = features::BACKPACK_FILL.chain(features::RUNE_POUCH).chain([features::AMMO_SLOT]);
    let mut items = HashMap::new();
    let states: Vec<SlotState> = wanted
        .filter(|&s| (s as i32) < num)
        .filter_map(|slot| {
            let item = MEM.read_u64(data + slot as u64 * 8).filter(|&i| g.r.is_object(i) && g.is_instance(i, g.item_class))?;
            let count = MEM.read_i32(item + o.item_count)?;
            let idata = g.ptr(item, o.item_data)?;
            let max_stack = MEM.read_i32(idata + o.max_stack)?;
            items.insert(slot, item);
            Some(SlotState { slot, count, max_stack })
        })
        .collect();
    for f in features::fills(t, states) {
        if let Some(&item) = items.get(&f.slot) {
            call_with(item, set_stack, "IntProperty", &f.target.to_le_bytes());
            once(st, &format!("refilled slot {} ({} -> {})", f.slot, f.count, f.target));
        }
    }
}

/// Background pass (not the game thread): zeroes the weight of every ItemData
/// while the feature is on, restores the originals when it is switched off.
/// Rerun periodically so items loaded later are covered too.
pub fn weights_pass() {
    let Some(g) = GAME.get() else { return };
    if PAUSED.load(Ordering::Relaxed) {
        return;
    }
    let on = settings::toggles().zero_weight;
    let Ok(mut st) = g.state.lock() else { return };
    if !on {
        for (obj, w) in st.weights.drain() {
            if g.r.is_object(obj) {
                crate::mem::write(obj + g.off.weight, w);
            }
        }
        return;
    }
    drop(st);
    // Walk without holding the lock: this pass takes a while.
    let mut found = Vec::new();
    for i in 0..g.r.object_count().unwrap_or(0) {
        if let Some(o) = g.r.object_at(i) {
            if g.is_instance(o, g.item_data_class) {
                found.push(o);
            }
        }
    }
    let Ok(mut st) = g.state.lock() else { return };
    let before = st.weights.len();
    for o in found {
        let Some(w) = MEM.read_f32(o + g.off.weight) else { continue };
        if w != 0.0 || st.weights.contains_key(&o) {
            st.weights.entry(o).or_insert(w);
            crate::mem::write(o + g.off.weight, 0.0f32);
        }
    }
    if st.weights.len() != before {
        log::line(&format!("zero weight: {} item types", st.weights.len()));
    }
}
