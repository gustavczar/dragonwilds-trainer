//! What each trainer feature decides, independent of how the platform applies it.

use serde::{Deserialize, Serialize};

/// The switches shown in the in-game panel and stored in `settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Toggles {
    /// Keep the whole rune pouch full, so spells never run out of runes.
    pub free_runes: bool,
    /// Spells are ready again right after being cast.
    pub no_cooldown: bool,
    /// Keep backpack slots 37-39 and the first ammo slot full.
    pub fill_slots: bool,
    /// Health bar kept full and the player cannot die. Off by default on purpose.
    pub infinite_health: bool,
    /// Stamina kept full.
    pub infinite_stamina: bool,
    /// Special charge (the blue bar used by special abilities) kept full.
    pub infinite_special: bool,
    /// Every attack, any weapon or spell, deals overwhelming damage. Off by default.
    pub one_hit: bool,
    /// Carried items weigh nothing.
    pub zero_weight: bool,
}

impl Default for Toggles {
    fn default() -> Self {
        Self {
            free_runes: true,
            no_cooldown: true,
            fill_slots: true,
            infinite_health: false,
            infinite_stamina: true,
            infinite_special: true,
            one_hit: false,
            zero_weight: true,
        }
    }
}

/// Damage written into every damage event the player causes, by `one_hit`.
pub const ONE_HIT_DAMAGE: f32 = 999_999.0;

/// One switch of [`Toggles`], for building the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    FreeRunes,
    NoCooldown,
    FillSlots,
    InfiniteHealth,
    InfiniteStamina,
    InfiniteSpecial,
    OneHit,
    ZeroWeight,
}

impl Feature {
    pub const ALL: [Feature; 8] = [
        Feature::FreeRunes,
        Feature::NoCooldown,
        Feature::FillSlots,
        Feature::InfiniteHealth,
        Feature::InfiniteStamina,
        Feature::InfiniteSpecial,
        Feature::OneHit,
        Feature::ZeroWeight,
    ];

    pub fn label(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Feature::FreeRunes, Lang::En) => "Free runes",
            (Feature::FreeRunes, Lang::PtBr) => "Runas grátis",
            (Feature::NoCooldown, Lang::En) => "No cooldown",
            (Feature::NoCooldown, Lang::PtBr) => "Sem cooldown",
            (Feature::FillSlots, Lang::En) => "Full slots (37-39, ammo)",
            (Feature::FillSlots, Lang::PtBr) => "Slots cheios (37-39, munição)",
            (Feature::InfiniteHealth, Lang::En) => "Infinite health",
            (Feature::InfiniteHealth, Lang::PtBr) => "Vida infinita",
            (Feature::InfiniteStamina, Lang::En) => "Infinite stamina",
            (Feature::InfiniteStamina, Lang::PtBr) => "Stamina infinita",
            (Feature::InfiniteSpecial, Lang::En) => "Infinite special charge",
            (Feature::InfiniteSpecial, Lang::PtBr) => "Barra azul infinita",
            (Feature::OneHit, Lang::En) => "One hit (any weapon or spell)",
            (Feature::OneHit, Lang::PtBr) => "One Hit (qualquer arma ou magia)",
            (Feature::ZeroWeight, Lang::En) => "Zero weight",
            (Feature::ZeroWeight, Lang::PtBr) => "Peso zero",
        }
    }

    pub fn get(self, t: &Toggles) -> bool {
        match self {
            Feature::FreeRunes => t.free_runes,
            Feature::NoCooldown => t.no_cooldown,
            Feature::FillSlots => t.fill_slots,
            Feature::InfiniteHealth => t.infinite_health,
            Feature::InfiniteStamina => t.infinite_stamina,
            Feature::InfiniteSpecial => t.infinite_special,
            Feature::OneHit => t.one_hit,
            Feature::ZeroWeight => t.zero_weight,
        }
    }

    pub fn set(self, t: &mut Toggles, on: bool) {
        match self {
            Feature::FreeRunes => t.free_runes = on,
            Feature::NoCooldown => t.no_cooldown = on,
            Feature::FillSlots => t.fill_slots = on,
            Feature::InfiniteHealth => t.infinite_health = on,
            Feature::InfiniteStamina => t.infinite_stamina = on,
            Feature::InfiniteSpecial => t.infinite_special = on,
            Feature::OneHit => t.one_hit = on,
            Feature::ZeroWeight => t.zero_weight = on,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Lang {
    #[serde(rename = "en")]
    En,
    #[default]
    #[serde(rename = "pt-BR")]
    PtBr,
}

/// Inventory layout (live-read on 02/11/2026): 0-7 hotbar, 8-39 backpack (new
/// loot lands in the first row, so only the last three slots are kept full),
/// 40-48 rune pouch (runes stack to 9999), 72 first ammo slot (arrows to 999).
pub const BACKPACK_FILL: std::ops::RangeInclusive<usize> = 37..=39;
pub const RUNE_POUCH: std::ops::RangeInclusive<usize> = 40..=48;
pub const AMMO_SLOT: usize = 72;
/// Backpack slots are capped at 99 even when the item stacks higher.
pub const BACKPACK_CAP: i32 = 99;

/// Seconds subtracted from "now" when rewriting a spell's last-cast time, so
/// every cooldown reads as long expired (same trick the server mod used).
pub const COOLDOWN_AGE_SECONDS: f64 = 1.0e6;

/// Stack size a slot should be raised to, or 0 to leave it alone: only
/// stackable items (`max > 1`) that are present (`count > 0`) and below
/// `min(max, cap)`.
pub fn fill_target(count: i32, max: i32, cap: i32) -> i32 {
    let top = max.min(cap);
    if max > 1 && count > 0 && count < top { top } else { 0 }
}

/// What one inventory slot currently holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotState {
    pub slot: usize,
    pub count: i32,
    /// ItemData.MaxStackSize of the item in the slot.
    pub max_stack: i32,
}

/// A refill to apply: set `slot`'s stack from `count` to `target`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fill {
    pub slot: usize,
    pub count: i32,
    pub target: i32,
}

/// Refills the enabled features ask for, in slot order of the input.
pub fn fills(t: &Toggles, slots: impl IntoIterator<Item = SlotState>) -> Vec<Fill> {
    slots
        .into_iter()
        .filter_map(|s| {
            let cap = if t.fill_slots && BACKPACK_FILL.contains(&s.slot) {
                BACKPACK_CAP
            } else if (t.fill_slots && s.slot == AMMO_SLOT) || (t.free_runes && RUNE_POUCH.contains(&s.slot)) {
                i32::MAX
            } else {
                return None;
            };
            match fill_target(s.count, s.max_stack, cap) {
                0 => None,
                target => Some(Fill { slot: s.slot, count: s.count, target }),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(slot: usize, count: i32, max_stack: i32) -> SlotState {
        SlotState { slot, count, max_stack }
    }

    #[test]
    fn fill_target_rules_match_the_server_mod() {
        // Same assertions as the C++ mod's tests.cpp (v3/v4).
        assert_eq!(fill_target(5, 99, 99), 99);
        assert_eq!(fill_target(99, 99, 99), 0);
        assert_eq!(fill_target(3, 20, 99), 20);
        assert_eq!(fill_target(20, 20, 99), 0);
        assert_eq!(fill_target(1, 1, 99), 0, "non-stackable stays");
        assert_eq!(fill_target(0, 99, 99), 0, "empty stays empty");
        assert_eq!(fill_target(-1, 99, 99), 0);
        assert_eq!(fill_target(50, 999, 99), 99, "backpack cap wins over item max");
        assert_eq!(fill_target(1529, 9999, i32::MAX), 9999, "runes fill to item max");
        assert_eq!(fill_target(9999, 9999, i32::MAX), 0);
    }

    #[test]
    fn fills_only_touches_enabled_slot_groups() {
        let inv = [s(8, 3, 99), s(36, 1, 99), s(37, 5, 99), s(38, 99, 99), s(39, 1, 1), s(40, 50, 9999),
                   s(48, 1, 9999), s(49, 1, 9999), s(72, 992, 999), s(73, 717, 999)];
        let all = fills(&Toggles::default(), inv);
        assert_eq!(all, vec![
            Fill { slot: 37, count: 5, target: 99 },
            Fill { slot: 40, count: 50, target: 9999 },
            Fill { slot: 48, count: 1, target: 9999 },
            Fill { slot: 72, count: 992, target: 999 },
        ]);

        let runes_only = Toggles { fill_slots: false, ..Toggles::default() };
        assert_eq!(fills(&runes_only, inv).iter().map(|f| f.slot).collect::<Vec<_>>(), vec![40, 48]);

        let slots_only = Toggles { free_runes: false, ..Toggles::default() };
        assert_eq!(fills(&slots_only, inv).iter().map(|f| f.slot).collect::<Vec<_>>(), vec![37, 72]);

        let off = Toggles { free_runes: false, fill_slots: false, ..Toggles::default() };
        assert!(fills(&off, inv).is_empty());
    }

    #[test]
    fn every_feature_round_trips_through_its_switch() {
        for f in Feature::ALL {
            let mut t = Toggles::default();
            f.set(&mut t, true);
            assert!(f.get(&t), "{f:?} on");
            f.set(&mut t, false);
            assert!(!f.get(&t), "{f:?} off");
            assert!(!f.label(Lang::PtBr).is_empty() && !f.label(Lang::En).is_empty());
        }
    }

    #[test]
    fn settings_saved_before_a_new_feature_get_its_default() {
        let t: Toggles = serde_json::from_str(r#"{"free_runes": false}"#).unwrap();
        assert!(!t.free_runes, "saved choice kept");
        assert!(t.infinite_stamina, "new switch takes its default");
    }

    #[test]
    fn defaults_keep_infinite_health_off() {
        assert!(!Toggles::default().infinite_health);
        let t: Toggles = serde_json::from_str("{}").unwrap();
        assert_eq!(t, Toggles::default(), "missing keys fall back to defaults");
    }
}
