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
    /// Keep the last backpack row (slots 32-39) and the first ammo slot full.
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
    /// Building pieces the player has unlocked cost no materials. Off by default.
    pub free_build: bool,
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
            free_build: false,
        }
    }
}

impl Toggles {
    /// Every feature off.
    pub const OFF: Toggles = Toggles {
        free_runes: false,
        no_cooldown: false,
        fill_slots: false,
        infinite_health: false,
        infinite_stamina: false,
        infinite_special: false,
        one_hit: false,
        zero_weight: false,
        free_build: false,
    };
}

/// Damage `one_hit` uses to finish a target the player hit that survived the blow.
pub const ONE_HIT_DAMAGE: f32 = 999_999.0;

/// Longest wait, after a hit, for the game to credit the player before `one_hit`
/// finishes the target (the credit is what makes it drop loot).
pub const ONE_HIT_GRACE_SECONDS: f32 = 1.5;

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
    FreeBuild,
}

impl Feature {
    pub const ALL: [Feature; 9] = [
        Feature::FreeRunes,
        Feature::NoCooldown,
        Feature::FillSlots,
        Feature::InfiniteHealth,
        Feature::InfiniteStamina,
        Feature::InfiniteSpecial,
        Feature::OneHit,
        Feature::ZeroWeight,
        Feature::FreeBuild,
    ];

    pub fn label(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Feature::FreeRunes, Lang::En) => "Free runes",
            (Feature::FreeRunes, Lang::PtBr) => "Runas grátis",
            (Feature::NoCooldown, Lang::En) => "No cooldown",
            (Feature::NoCooldown, Lang::PtBr) => "Sem cooldown",
            (Feature::FillSlots, Lang::En) => "Full slots (last backpack row, ammo)",
            (Feature::FillSlots, Lang::PtBr) => "Slots cheios (última fileira, munição)",
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
            (Feature::FreeBuild, Lang::En) => "Free build (no materials)",
            (Feature::FreeBuild, Lang::PtBr) => "Construção livre (sem materiais)",
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
            Feature::FreeBuild => t.free_build,
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
            Feature::FreeBuild => t.free_build = on,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Lang {
    #[default]
    #[serde(rename = "en")]
    En,
    #[serde(rename = "pt-BR")]
    PtBr,
}

/// Inventory layout (live-read on 02/11/2026): 0-7 hotbar, 8-39 backpack in four
/// rows of eight (new loot lands in the first row, so the last row, 32-39, is the
/// one kept full), 40-48 rune pouch (runes stack to 9999), 72 first ammo slot
/// (arrows to 999). Every refilled slot goes to its item's own maximum.
pub const BACKPACK_FILL: std::ops::RangeInclusive<usize> = 32..=39;
pub const RUNE_POUCH: std::ops::RangeInclusive<usize> = 40..=48;
pub const AMMO_SLOT: usize = 72;

/// Seconds subtracted from "now" when rewriting a spell's last-cast time, so
/// every cooldown reads as long expired (same trick the server mod used).
pub const COOLDOWN_AGE_SECONDS: f64 = 1.0e6;

/// Stack size a slot should be raised to, or 0 to leave it alone: only
/// stackable items (`max > 1`) that are present (`count > 0`) and below their
/// own maximum (50, 99, 999 or 9999 depending on the item).
pub fn fill_target(count: i32, max: i32) -> i32 {
    if max > 1 && count > 0 && count < max { max } else { 0 }
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
        .filter(|s| {
            (t.fill_slots && (BACKPACK_FILL.contains(&s.slot) || s.slot == AMMO_SLOT))
                || (t.free_runes && RUNE_POUCH.contains(&s.slot))
        })
        .filter_map(|s| match fill_target(s.count, s.max_stack) {
            0 => None,
            target => Some(Fill { slot: s.slot, count: s.count, target }),
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
    fn fill_target_goes_to_the_items_own_maximum() {
        assert_eq!(fill_target(5, 99), 99);
        assert_eq!(fill_target(99, 99), 0, "already full");
        assert_eq!(fill_target(3, 50), 50, "items capped at 50 stop at 50");
        assert_eq!(fill_target(50, 999), 999, "no 99 ceiling any more");
        assert_eq!(fill_target(1529, 9999), 9999);
        assert_eq!(fill_target(1, 1), 0, "non-stackable stays");
        assert_eq!(fill_target(0, 99), 0, "empty stays empty");
        assert_eq!(fill_target(-1, 99), 0);
    }

    #[test]
    fn fills_only_touches_enabled_slot_groups() {
        let inv = [s(8, 3, 99), s(31, 1, 99), s(32, 7, 50), s(35, 5, 9999), s(38, 99, 99), s(39, 1, 1),
                   s(40, 50, 9999), s(48, 1, 9999), s(49, 1, 9999), s(72, 992, 999), s(73, 717, 999)];
        let all = fills(&Toggles::default(), inv);
        assert_eq!(all, vec![
            Fill { slot: 32, count: 7, target: 50 },
            Fill { slot: 35, count: 5, target: 9999 },
            Fill { slot: 40, count: 50, target: 9999 },
            Fill { slot: 48, count: 1, target: 9999 },
            Fill { slot: 72, count: 992, target: 999 },
        ]);

        let runes_only = Toggles { fill_slots: false, ..Toggles::default() };
        assert_eq!(fills(&runes_only, inv).iter().map(|f| f.slot).collect::<Vec<_>>(), vec![40, 48]);

        let slots_only = Toggles { free_runes: false, ..Toggles::default() };
        assert_eq!(fills(&slots_only, inv).iter().map(|f| f.slot).collect::<Vec<_>>(), vec![32, 35, 72]);

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
        assert!(!Toggles::default().free_build);
        let t: Toggles = serde_json::from_str("{}").unwrap();
        assert_eq!(t, Toggles::default(), "missing keys fall back to defaults");
    }
}
