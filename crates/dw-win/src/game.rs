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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

static MEM: InProcess = InProcess;
static GAME: OnceLock<Game> = OnceLock::new();
static PAUSED: AtomicBool = AtomicBool::new(false);
/// Shown in the panel: the player's components were found.
pub static PLAYER_FOUND: AtomicBool = AtomicBool::new(false);
/// Players in the current world (GameState.PlayerArray), from the background pass.
pub static PLAYERS: AtomicUsize = AtomicUsize::new(0);

/// A spell's last-cast time is written this far in the past (float seconds).
const LONG_AGO: f32 = -1.0e9;
/// Element of TMap<UObject*, float>: key 8 + value 4 + hash links 8, aligned to 8.
const CAST_MAP_STRIDE: u64 = 24;
const CAST_MAP_VALUE: u64 = 8;
/// The backpack: the controller's paged inventory (hotbar, backpack, rune and ammo pouches).
const BACKPACK_CLASS: &str = "PagedInventoryWithLogComponent";
/// Attributes logged when a player is found, to compare base and effective values.
const LOGGED_ATTRS: [&str; 6] = [
    "HealthAttribute",
    "MaxHealthAttribute",
    "StaminaAttribute",
    "MaxStaminaAttribute",
    "SpecialChargeAttribute",
    "MaxSpecialChargeAttribute",
];

struct Offsets {
    viewport: u64,
    world: u64,
    game_instance: u64,
    game_state: u64,
    player_array: u64,
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
    damage_amount: u64,
    damage_instigator: u64,
    /// TArray<TypeDamage> inside the damage event (event + TypeDamagesArray + TypeDamages).
    type_damages: u64,
    type_damage_amount: u64,
    type_damage_size: u64,
    /// AActor arrays listing an actor's components.
    actor_components: [u64; 2],
}

/// A damage function whose parameters carry a DominionDamageEvent.
struct DamageWatch {
    name: &'static str,
    /// Offset of the event inside the function's parameter block.
    event_offset: u64,
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
    decrease_health: Option<FunctionInfo>,
    is_dead: Option<FunctionInfo>,
    get_health: Option<FunctionInfo>,
    damage_events: Vec<DamageWatch>,
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
    /// Damage events logged for the current player (the first few, as evidence).
    damage_logged: u32,
    /// Actors the player hit with one hit on; the next tick finishes any still alive.
    doomed: Vec<u64>,
    finishes_logged: u32,
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
        game_state: prop("World", "GameState")?,
        player_array: prop("GameStateBase", "PlayerArray")?,
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
        damage_amount: prop("DominionDamageEvent", "Amount")?,
        damage_instigator: prop("DominionDamageEvent", "Instigator")?,
        type_damages: prop("DominionDamageEvent", "TypeDamagesArray")? + prop("TypeDamageArray", "TypeDamages")?,
        type_damage_amount: prop("TypeDamage", "Amount")?,
        type_damage_size: r
            .find_type("TypeDamage")
            .and_then(|s| MEM.read_i32(s + r.layout.ustruct_size))
            .filter(|&n| n > 0)
            .ok_or("TypeDamage size not readable")? as u64,
        actor_components: [prop("Actor", "BlueprintCreatedComponents")?, prop("Actor", "InstanceComponents")?],
    };
    // Damage functions run through ProcessEvent with the event by reference:
    // the "will receive" Blueprint events first, the multicasts as a backup.
    let mut damage_events = Vec::new();
    for (name, event) in [
        ("BP_OnWillReceiveDamage", "DamageEvent"),
        ("BP_OnWillReceivePointDamage", "PointDamageEvent"),
        ("Multicast_ApplyDamageFromGameplayEffect", "DamageEvent"),
        ("Multicast_ApplyPointDamageFromGameplayEffect", "PointDamageEvent"),
    ] {
        match func("DamageComponent", name).and_then(|f| f.params.iter().find(|p| p.name == event).map(|p| (f.object, p.offset))) {
            Some((ufunc, at)) if damage_events.len() < engine::WATCHED.len() => {
                engine::WATCHED[damage_events.len()].store(ufunc as usize, Ordering::Relaxed);
                damage_events.push(DamageWatch { name, event_offset: at as u64 });
            }
            _ => log::line(&format!("damage function {name} not found; one hit misses it")),
        }
    }
    let game = Game {
        item_class: r.find_type("Item").ok_or("class Item not found")?,
        item_data_class: r.find_type("ItemData").ok_or("class ItemData not found")?,
        set_immortal: func("HealthComponent", "SetIsImmortal"),
        set_can_die: func("HealthComponent", "SetCanDie"),
        set_stack: func("Item", "SetStackSize"),
        update_encumbrance: func("PlayerEncumbranceComponent", "UpdateEncumberedStatus"),
        get_attribute: func("DominionAttributesComponent", "BP_TryGetFloatAttributeValue"),
        decrease_health: func("HealthComponent", "DecreaseHealth"),
        is_dead: func("HealthComponent", "IsDead"),
        get_health: func("HealthComponent", "GetAuthoritativeHealth"),
        damage_events,
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

    /// Players in the world the local player is in.
    fn player_count(&self) -> Option<usize> {
        let o = &self.off;
        let world = self.ptr(self.ptr(self.engine, o.viewport)?, o.world)?;
        let gs = self.ptr(world, o.game_state)?;
        Some(MEM.read_i32(gs + o.player_array + 8)?.max(0) as usize)
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

    /// First component of `actor` that is a `class`, from the actor's component arrays.
    fn actor_component(&self, actor: u64, class: &str) -> Option<u64> {
        self.off.actor_components.iter().find_map(|&arr| {
            let (data, num) = (MEM.read_u64(actor + arr)?, MEM.read_i32(actor + arr + 8)?);
            (0..num.clamp(0, 256) as u64).find_map(|i| MEM.read_u64(data + i * 8).filter(|&c| self.r.is_object(c) && self.r.is_a(c, class)))
        })
    }

    /// Health component of any actor: AI characters, buildings and destructibles
    /// keep it in a native `HealthComponent` field; Blueprint actors list it.
    fn health_component(&self, actor: u64) -> Option<u64> {
        let field = self.r.class_of(actor).and_then(|c| self.r.find_property(c, "HealthComponent"));
        field
            .and_then(|f| self.ptr(actor, f.offset as u64))
            .filter(|&hc| self.r.is_a(hc, "HealthComponent"))
            .or_else(|| self.actor_component(actor, "HealthComponent"))
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

/// Calls a no-argument game function that returns a float.
fn returned_f32(obj: u64, f: &FunctionInfo) -> Option<f32> {
    let at = f.params.iter().find(|p| p.name == "ReturnValue")?.offset as usize;
    let mut buf = vec![0u8; engine::params_size(f)];
    engine::call(obj, f, &mut buf);
    Some(f32::from_le_bytes(buf.get(at..at + 4)?.try_into().ok()?))
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

/// The saved switches, all off while other players share the world.
fn active_toggles() -> Toggles {
    features::solo_only(settings::toggles(), PLAYERS.load(Ordering::Relaxed))
}

/// Game thread, 4x a second.
pub fn tick() {
    let Some(g) = GAME.get() else { return };
    if PAUSED.load(Ordering::Relaxed) {
        return;
    }
    let t = active_toggles();
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
    if t.one_hit && !st.doomed.is_empty() {
        finish_doomed(g, &mut st);
    }
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

/// One hit, layer 1: called from the ProcessEvent detour *before* the game runs
/// one of the watched damage functions (`engine::WATCHED[index]`). Those are
/// notifications that get a *copy* of the damage event (editing it changes
/// nothing, tested 21/11), but they fire before the blow lands: the target's
/// health is dropped to 1 through the game's own DecreaseHealth, so the real
/// blow kills it and the game credits the player (kill, XP, loot). Only for
/// damage the local player caused; the target is also queued for layer 2.
pub fn before_call(index: usize, obj: u64, params: u64) {
    let Some(g) = GAME.get() else { return };
    let Some(watch) = g.damage_events.get(index) else { return };
    if PAUSED.load(Ordering::Relaxed) || !active_toggles().one_hit {
        return;
    }
    let Ok(mut st) = g.state.try_lock() else { return };
    let Some((pc, pawn)) = st.player.as_ref().map(|p| (p.pc, p.pawn)) else { return };
    let event = params + watch.event_offset;
    let Some(instigator) = MEM.read_u64(event + g.off.damage_instigator) else { return };
    let Some(amount) = MEM.read_f32(event + g.off.damage_amount) else { return };
    // Damage the player causes to something else; never to the player (fall damage
    // can name the player as its own instigator).
    let target = g.r.outer_of(obj);
    let mine = instigator != 0 && (instigator == pawn || instigator == pc) && target != Some(pawn);
    let list = event + g.off.type_damages;
    let (data, num) = (MEM.read_u64(list).unwrap_or(0), MEM.read_i32(list + 8).unwrap_or(0).clamp(0, 16) as u64);
    let type_amount = |i: u64| data + i * g.off.type_damage_size + g.off.type_damage_amount;
    let hc = target.filter(|_| mine && amount > 0.0).and_then(|a| g.health_component(a));
    let hp = hc.and_then(|hc| g.get_health.as_ref().and_then(|f| returned_f32(hc, f)));
    if st.damage_logged < 5 {
        st.damage_logged += 1;
        let types: Vec<f32> = (0..num).filter_map(|i| MEM.read_f32(type_amount(i))).collect();
        log::line(&format!(
            "damage via {}: target={} ({}) instigator={} amount={amount} types={types:?} mine={mine} health={} hp={hp:?}",
            watch.name,
            g.label(Some(obj)),
            g.label(target),
            g.label(Some(instigator)),
            g.label(hc),
        ));
    }
    if !mine || amount <= 0.0 {
        return;
    }
    if let (Some(hc), Some(hp), Some(decrease)) = (hc, hp, &g.decrease_health) {
        if hp > 1.0 {
            call_with(hc, decrease, "FloatProperty", &(hp - 1.0).to_le_bytes());
        }
    }
    if let Some(actor) = target.filter(|a| !st.doomed.contains(a)) {
        if st.doomed.len() < 32 {
            st.doomed.push(actor);
        }
    }
}

/// One hit, layer 2 (game thread): any actor the player hit that is still
/// alive gets its health component's DecreaseHealth(999999).
fn finish_doomed(g: &Game, st: &mut State) {
    let (Some(decrease), Some(is_dead)) = (&g.decrease_health, &g.is_dead) else { return };
    for actor in std::mem::take(&mut st.doomed) {
        if !g.r.is_object(actor) {
            continue;
        }
        let Some(hc) = g.health_component(actor) else {
            if st.finishes_logged < 5 {
                st.finishes_logged += 1;
                log::line(&format!("finish: no HealthComponent on {}", g.label(Some(actor))));
            }
            continue;
        };
        let dead = |hc: u64| {
            let mut buf = vec![0u8; engine::params_size(is_dead)];
            engine::call(hc, is_dead, &mut buf);
            is_dead.params.iter().find(|p| p.name == "ReturnValue").map(|p| buf[p.offset as usize] != 0)
        };
        if st.finishes_logged < 5 {
            st.finishes_logged += 1;
            log::line(&format!("finish: {} dead={:?}", g.label(Some(actor)), dead(hc)));
        }
        if dead(hc) != Some(false) {
            continue; // layer 1 already killed it (or the state is unknown)
        }
        call_with(hc, decrease, "FloatProperty", &features::ONE_HIT_DAMAGE.to_le_bytes());
        if st.finishes_logged < 5 {
            st.finishes_logged += 1;
            log::line(&format!("finish: {} still alive after the hit; DecreaseHealth -> dead={:?}", g.label(Some(actor)), dead(hc)));
        }
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

/// One log line per occupied slot in `range`: item, count and the item's max.
fn log_slots(g: &Game, bag: u64, range: std::ops::RangeInclusive<usize>) {
    let slots = bag + g.off.item_slots;
    let (Some(data), Some(num)) = (MEM.read_u64(slots), MEM.read_i32(slots + 8)) else { return };
    for slot in range.filter(|&s| (s as i32) < num) {
        let Some(item) = MEM.read_u64(data + slot as u64 * 8).filter(|&i| g.r.is_object(i)) else { continue };
        let idata = g.ptr(item, g.off.item_data);
        log::line(&format!(
            "slot {slot}: {} count={:?} max={:?}",
            idata.and_then(|d| g.r.object_name(d)).unwrap_or_else(|| "?".into()),
            MEM.read_i32(item + g.off.item_count),
            idata.and_then(|d| MEM.read_i32(d + g.off.max_stack)),
        ));
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
    if let Some(n) = g.player_count() {
        if PLAYERS.swap(n, Ordering::Relaxed) != n {
            log::line(&format!("players in world: {n}{}", if n > 1 { " (others present: features off)" } else { "" }));
        }
    }
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
    if let Some(bag) = p.backpack {
        log_slots(g, bag, 24..=48);
    }
    PLAYER_FOUND.store(p.health.is_some() || p.backpack.is_some(), Ordering::Relaxed);
    if let Ok(mut st) = g.state.lock() {
        // A new pawn: log its values and damage events again.
        st.noted.clear();
        st.damage_logged = 0;
        st.finishes_logged = 0;
        st.doomed.clear();
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
    let on = active_toggles().zero_weight;
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
