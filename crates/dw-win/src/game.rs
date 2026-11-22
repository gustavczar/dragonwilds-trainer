//! Finds the local player and applies the enabled features. Names are
//! resolved once (`init`); a background pass finds the player's components
//! and attributes (`player_pass`); `tick` applies the features on the game
//! thread 4x a second.
//!
//! Health, stamina, special charge and damage multipliers are `FloatAttribute`
//! objects owned by the player character. The game reports an attribute's
//! effective value through `DominionAttributesComponent.BP_TryGetFloatAttributeValue`;
//! resources are topped up by raising the attribute's `BaseValue`.

use crate::{engine, log, mem::InProcess, settings};
use dw_core::features::{self, SlotState};
use dw_core::{FunctionInfo, MemoryExt, Reflection, Toggles};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

static MEM: InProcess = InProcess;
static GAME: OnceLock<Game> = OnceLock::new();
static PAUSED: AtomicBool = AtomicBool::new(false);
/// Shown in the panel: the player's components were found.
pub static PLAYER_FOUND: AtomicBool = AtomicBool::new(false);

/// A spell's last-cast time is written this far in the past (float seconds).
const LONG_AGO: f32 = -1.0e9;
/// Element of TMap<UObject*, float>: key 8 + value 4 + hash links 8, aligned to 8.
const CAST_MAP_STRIDE: u64 = 24;
const CAST_MAP_VALUE: u64 = 8;
/// The backpack: the controller's paged inventory (hotbar, backpack, rune and ammo pouches).
const BACKPACK_CLASS: &str = "PagedInventoryWithLogComponent";
const ALL_STYLES_DAMAGE: &str = "DamageMultiplierAllCombatStylesAttribute";
/// Attributes logged when a player is found, to compare base and effective values.
const LOGGED_ATTRS: [&str; 7] = [
    "HealthAttribute",
    "MaxHealthAttribute",
    "StaminaAttribute",
    "MaxStaminaAttribute",
    "SpecialChargeAttribute",
    "MaxSpecialChargeAttribute",
    ALL_STYLES_DAMAGE,
];

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
    item_slots: u64,
    cast_times: u64,
    carry_weight: u64,
    base_value: u64,
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
    update_encumbrance: Option<FunctionInfo>,
    get_attribute: Option<FunctionInfo>,
    state: Mutex<State>,
}

/// The player's objects, found by the background pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Player {
    pc: u64,
    pawn: u64,
    health: Option<u64>,
    magic: Option<u64>,
    encumbrance: Option<u64>,
    attributes: Option<u64>,
    backpack: Option<u64>,
    /// FloatAttribute objects of the pawn, by class name.
    attrs: HashMap<String, u64>,
}

#[derive(Default)]
struct State {
    player: Option<Player>,
    /// Health component we made immortal, if any.
    immortal: Option<u64>,
    /// (attribute, original base) of the damage multiplier we raised.
    one_hit: Option<(u64, f32)>,
    /// Original weight of every ItemData we zeroed.
    weights: HashMap<u64, f32>,
    /// One-time log lines already written for the current player.
    noted: HashSet<String>,
}

fn note(st: &mut State, key: &str, msg: impl FnOnce() -> String) {
    if st.noted.insert(key.to_string()) {
        log::line(&msg());
    }
}

/// Resolves the engine object, the player pointer chain, field offsets and game functions.
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
        item_slots: prop("InventoryComponent", "ItemSlots")?,
        cast_times: prop("PlayerUtilityMagicComponent", "LastCastTimeBySpellData")?,
        carry_weight: prop("PlayerEncumbranceComponent", "CurrentCarryWeight")?,
        base_value: prop("FloatAttribute", "BaseValue")?,
    };
    let game = Game {
        item_class: r.find_type("Item").ok_or("class Item not found")?,
        item_data_class: r.find_type("ItemData").ok_or("class ItemData not found")?,
        set_immortal: func("HealthComponent", "SetIsImmortal"),
        set_can_die: func("HealthComponent", "SetCanDie"),
        set_stack: func("Item", "SetStackSize"),
        update_encumbrance: func("PlayerEncumbranceComponent", "UpdateEncumberedStatus"),
        get_attribute: func("DominionAttributesComponent", "BP_TryGetFloatAttributeValue"),
        r,
        engine,
        off,
        state: Mutex::new(State::default()),
    };
    for (name, f) in [
        ("SetIsImmortal", &game.set_immortal),
        ("SetCanDie", &game.set_can_die),
        ("SetStackSize", &game.set_stack),
        ("UpdateEncumberedStatus", &game.update_encumbrance),
        ("BP_TryGetFloatAttributeValue", &game.get_attribute),
    ] {
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

    /// Engine -> viewport -> world -> game instance -> LocalPlayers[0] -> (controller, pawn).
    fn controller_and_pawn(&self) -> Option<(u64, u64)> {
        let o = &self.off;
        let world = self.ptr(self.ptr(self.engine, o.viewport)?, o.world)?;
        let gi = self.ptr(world, o.game_instance)?;
        let players = MEM.read_u64(gi + o.local_players)?;
        let lp = self.ptr(players, 0)?;
        let pc = self.ptr(lp, o.player_controller)?;
        Some((pc, self.ptr(pc, o.pawn)?))
    }

    /// Components and attributes are subobjects of their actor: walk the object
    /// array once and keep the ones whose outer is the pawn or the controller.
    fn find_player(&self, pc: u64, pawn: u64) -> Player {
        let mut p = Player { pc, pawn, ..Player::default() };
        let outer_at = self.r.layout.uobject_outer;
        let slot = |found: &mut Option<u64>, o: u64, class: &str| {
            if found.is_none() && self.r.is_a(o, class) {
                *found = Some(o);
                true
            } else {
                false
            }
        };
        for i in 0..self.r.object_count().unwrap_or(0) {
            let Some(o) = self.r.object_at(i) else { continue };
            let Some(outer) = MEM.read_u64(o + outer_at) else { continue };
            if outer == pawn {
                let _ = slot(&mut p.health, o, "HealthComponent")
                    || slot(&mut p.magic, o, "PlayerUtilityMagicComponent")
                    || slot(&mut p.encumbrance, o, "PlayerEncumbranceComponent")
                    || slot(&mut p.attributes, o, "DominionAttributesComponent")
                    || (self.r.is_a(o, "FloatAttribute")
                        && self.r.class_of(o).and_then(|c| self.r.object_name(c)).map(|n| p.attrs.insert(n, o)).is_some());
            } else if outer == pc {
                slot(&mut p.backpack, o, BACKPACK_CLASS);
            }
        }
        p
    }

    fn label(&self, o: Option<u64>) -> String {
        o.and_then(|o| self.r.class_of(o)).and_then(|c| self.r.object_name(c)).unwrap_or_else(|| "NOT FOUND".into())
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

    fn base(&self, attr: u64) -> Option<f32> {
        MEM.read_f32(attr + self.off.base_value)
    }

    fn set_base(&self, attr: u64, v: f32) -> bool {
        crate::mem::write(attr + self.off.base_value, v)
    }

    /// Effective value of `attr` as the game computes it (base + modifiers).
    /// Game thread only. Falls back to the base value.
    fn value(&self, p: &Player, attr: u64) -> Option<f32> {
        let (Some(comp), Some(f)) = (p.attributes, &self.get_attribute) else { return self.base(attr) };
        let class = self.r.class_of(attr)?;
        let param = |n: &str| f.params.iter().find(|x| x.name == n).map(|x| x.offset as usize);
        let (Some(c), Some(v), Some(found)) = (param("FloatAttribute"), param("OutAttributeValue"), param("OutbFound")) else {
            return self.base(attr);
        };
        let mut buf = vec![0u8; engine::params_size(f)];
        buf[c..c + 8].copy_from_slice(&class.to_le_bytes());
        engine::call(comp, f, &mut buf);
        (buf[found] != 0).then(|| f32::from_le_bytes(buf[v..v + 4].try_into().unwrap()))
    }
}

/// Calls a game function, putting `value` in its first parameter of type `kind`
/// (or no parameter when `kind` is empty).
fn call_with(obj: u64, f: &FunctionInfo, kind: &str, value: &[u8]) {
    let mut buf = vec![0u8; engine::params_size(f)];
    if !kind.is_empty() {
        let Some(p) = f.params.iter().find(|p| p.type_name == kind) else { return };
        let at = p.offset as usize;
        if at + value.len() > buf.len() {
            return;
        }
        buf[at..at + value.len()].copy_from_slice(value);
    }
    engine::call(obj, f, &mut buf);
}

/// Game thread, 4x a second.
pub fn tick() {
    let Some(g) = GAME.get() else { return };
    if PAUSED.load(Ordering::Relaxed) {
        return;
    }
    let t = settings::toggles();
    let Ok(mut st) = g.state.lock() else { return };
    let Some(p) = st.player.clone() else { return };
    // The pass may be up to 2 s old: drop it if the player changed meanwhile.
    if g.controller_and_pawn() != Some((p.pc, p.pawn)) {
        return;
    }
    let live = |o: Option<u64>| o.filter(|&o| g.r.is_object(o));

    if !st.noted.contains("values") {
        log_values(g, &mut st, &p);
    }
    if let Some(hc) = live(p.health) {
        immortality(g, &mut st, &t, hc);
    }
    if t.infinite_health {
        top_up(g, &mut st, &p, "HealthAttribute", "MaxHealthAttribute");
    }
    if t.infinite_stamina {
        top_up(g, &mut st, &p, "StaminaAttribute", "MaxStaminaAttribute");
    }
    if t.infinite_special {
        top_up(g, &mut st, &p, "SpecialChargeAttribute", "MaxSpecialChargeAttribute");
    }
    one_hit(g, &mut st, &t, &p);
    if let (true, Some(magic)) = (t.no_cooldown, live(p.magic)) {
        cooldowns(g, magic);
    }
    if let (true, Some(enc)) = (t.zero_weight, live(p.encumbrance)) {
        carry_weight(g, enc);
    }
    if let (true, Some(bag)) = (t.fill_slots || t.free_runes, live(p.backpack)) {
        refill(g, &t, bag);
    }
}

/// Base and effective value of the attributes the features use, once per player.
fn log_values(g: &Game, st: &mut State, p: &Player) {
    st.noted.insert("values".into());
    for name in LOGGED_ATTRS {
        match p.attrs.get(name) {
            Some(&a) => log::line(&format!("attr {name}: base={:?} value={:?}", g.base(a), g.value(p, a))),
            None => log::line(&format!("attr {name}: NOT FOUND")),
        }
    }
}

/// Raises the base of `cur` so its effective value reaches the effective value of `max`.
fn top_up(g: &Game, st: &mut State, p: &Player, cur: &str, max: &str) {
    let (Some(&c), Some(&m)) = (p.attrs.get(cur), p.attrs.get(max)) else {
        note(st, &format!("missing {cur}"), || format!("{cur} or {max} not found; cannot top up"));
        return;
    };
    let (Some(now), Some(full), Some(base)) = (g.value(p, c), g.value(p, m), g.base(c)) else { return };
    if full <= 0.0 || now >= full {
        return;
    }
    if g.set_base(c, base + (full - now)) {
        let first = !st.noted.contains(cur);
        if first {
            let after = g.value(p, c);
            note(st, cur, || format!("top up {cur}: {now} -> {after:?} (max {full}, base {base} -> {})", base + full - now));
        }
    }
}

/// `SetIsImmortal(bValue, LockerObj) -> bool`: the game counts immortality per
/// locker object, so the request must name one and the release must use the
/// same one. The health component itself is our locker.
fn set_immortal(hc: u64, f: &FunctionInfo, on: bool) -> Option<bool> {
    let mut buf = vec![0u8; engine::params_size(f)];
    let param = |name: &str| f.params.iter().find(|p| p.name == name).map(|p| p.offset as usize);
    buf[param("bValue")?] = on as u8;
    let locker = param("LockerObj")?;
    buf[locker..locker + 8].copy_from_slice(&hc.to_le_bytes());
    engine::call(hc, f, &mut buf);
    param("ReturnValue").map(|r| buf[r] != 0)
}

/// Guards against a single blow bigger than the whole bar landing between two ticks.
fn immortality(g: &Game, st: &mut State, t: &Toggles, hc: u64) {
    let (Some(immortal), Some(can_die)) = (&g.set_immortal, &g.set_can_die) else { return };
    // Only undo what we did: never clear an immortality the game set itself.
    match (t.infinite_health, st.immortal == Some(hc)) {
        (true, false) => {
            let accepted = set_immortal(hc, immortal, true);
            call_with(hc, can_die, "BoolProperty", &[0]);
            st.immortal = Some(hc);
            log::line(&format!("infinite health on (immortal accepted: {accepted:?})"));
        }
        (false, true) => {
            let accepted = set_immortal(hc, immortal, false);
            call_with(hc, can_die, "BoolProperty", &[1]);
            st.immortal = None;
            log::line(&format!("infinite health off (release accepted: {accepted:?})"));
        }
        _ => {}
    }
}

/// Multiplies the damage of every combat style (melee, ranged, magic); restores it when off.
fn one_hit(g: &Game, st: &mut State, t: &Toggles, p: &Player) {
    let Some(&attr) = p.attrs.get(ALL_STYLES_DAMAGE) else { return };
    match (t.one_hit, st.one_hit) {
        (true, None) => {
            let Some(orig) = g.base(attr) else { return };
            let before = g.value(p, attr);
            if g.set_base(attr, features::ONE_HIT_MULTIPLIER) {
                st.one_hit = Some((attr, orig));
                log::line(&format!("one hit on: damage x{before:?} -> x{:?} (base {orig})", g.value(p, attr)));
            }
        }
        (true, Some((a, _))) if g.base(a) != Some(features::ONE_HIT_MULTIPLIER) => {
            // The game rewrote it (e.g. equipment change): raise it again.
            g.set_base(a, features::ONE_HIT_MULTIPLIER);
        }
        (false, Some((a, orig))) if g.set_base(a, orig) => {
            st.one_hit = None;
            log::line(&format!("one hit off: base restored to {orig}"));
        }
        _ => {}
    }
}

fn cooldowns(g: &Game, magic: u64) {
    let map = magic + g.off.cast_times;
    let (Some(data), Some(num)) = (MEM.read_u64(map), MEM.read_i32(map + 8)) else { return };
    for i in 0..num.clamp(0, 512) as u64 {
        let e = data + i * CAST_MAP_STRIDE;
        // Free sparse-array slots hold list links, never a live object: skip them.
        if MEM.read_u64(e).is_some_and(|k| g.r.is_object(k)) {
            crate::mem::write(e + CAST_MAP_VALUE, LONG_AGO);
        }
    }
}

/// The game caches the carried total; zero it and let the game re-check the penalty.
fn carry_weight(g: &Game, enc: u64) {
    let at = enc + g.off.carry_weight;
    if MEM.read_f32(at).is_some_and(|w| w > 0.0) && crate::mem::write(at, 0.0f32) {
        if let Some(update) = &g.update_encumbrance {
            call_with(enc, update, "", &[]);
        }
    }
}

fn refill(g: &Game, t: &Toggles, bag: u64) {
    let Some(set_stack) = &g.set_stack else { return };
    let o = &g.off;
    let slots = bag + o.item_slots;
    let (Some(data), Some(num)) = (MEM.read_u64(slots), MEM.read_i32(slots + 8)) else { return };
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
            log::line(&format!("refilled slot {} ({} -> {})", f.slot, f.count, f.target));
        }
    }
}

/// Background thread, every 2 s: (re)finds the player's components and
/// attributes when the player changes (world load, respawn), and logs them.
pub fn player_pass() {
    let Some(g) = GAME.get() else { return };
    let Some((pc, pawn)) = g.controller_and_pawn() else {
        PLAYER_FOUND.store(false, Ordering::Relaxed);
        return;
    };
    let known = g.state.lock().ok().and_then(|st| st.player.as_ref().map(|p| (p.pc, p.pawn)));
    if known == Some((pc, pawn)) {
        return;
    }
    let p = g.find_player(pc, pawn);
    log::line(&format!(
        "player: pawn={} health={} magic={} encumbrance={} attributes={} ({} attrs) backpack={}",
        g.label(Some(pawn)),
        g.label(p.health),
        g.label(p.magic),
        g.label(p.encumbrance),
        g.label(p.attributes),
        p.attrs.len(),
        g.label(p.backpack)
    ));
    PLAYER_FOUND.store(p.health.is_some() || p.backpack.is_some(), Ordering::Relaxed);
    if let Ok(mut st) = g.state.lock() {
        // A new pawn has new attribute objects: forget what we did to the old ones.
        st.one_hit = None;
        st.noted.clear();
        st.player = Some(p);
    }
}

/// Background thread, every 20 s: zeroes the weight of every ItemData while the
/// feature is on (so tooltips and new pickups weigh 0), restores the originals
/// when it is switched off.
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
