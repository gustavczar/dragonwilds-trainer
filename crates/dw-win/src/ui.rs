//! In-game panel: `Esc` shows an icon in the top-right corner; clicking it
//! opens the list of switches. The icon hides again when the game hides its
//! cursor (back to gameplay).

use crate::{game, settings};
use dw_core::features::{Feature, Lang};
use hudhook::imgui::{Condition, Io, Ui, WindowFlags};
use hudhook::{ImguiRenderLoop, MessageFilter};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorInfo, CURSORINFO, CURSOR_SHOWING};

/// After `Esc`, how long the game gets to show its cursor before the icon gives up.
const CURSOR_GRACE: Duration = Duration::from_millis(600);

#[derive(Default)]
pub struct Panel {
    icon: bool,
    open: bool,
    esc_down: bool,
    cursor_seen: bool,
    shown_at: Option<Instant>,
}

fn cursor_showing() -> bool {
    let mut ci = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..unsafe { std::mem::zeroed() } };
    // SAFETY: cbSize is set, as the API requires.
    unsafe { GetCursorInfo(&mut ci) != 0 && ci.flags & CURSOR_SHOWING != 0 }
}

impl Panel {
    fn update_visibility(&mut self) {
        // SAFETY: plain Win32 getter.
        let esc = unsafe { GetAsyncKeyState(VK_ESCAPE as i32) } as u16 & 0x8000 != 0;
        if esc && !self.esc_down {
            self.icon = !self.icon;
            self.open = false;
            self.cursor_seen = false;
            self.shown_at = Some(Instant::now());
        }
        self.esc_down = esc;
        if self.icon {
            if cursor_showing() {
                self.cursor_seen = true;
            } else if self.cursor_seen || self.shown_at.is_some_and(|t| t.elapsed() > CURSOR_GRACE) {
                self.icon = false;
                self.open = false;
            }
        }
    }
}

impl ImguiRenderLoop for Panel {
    fn render(&mut self, ui: &mut Ui) {
        self.update_visibility();
        if !self.icon {
            return;
        }
        let [w, _] = ui.io().display_size;
        let fixed = WindowFlags::NO_MOVE | WindowFlags::NO_RESIZE | WindowFlags::NO_COLLAPSE | WindowFlags::NO_SAVED_SETTINGS;

        let mut open = self.open;
        ui.window("##dw-icon")
            .position([w - 64.0, 16.0], Condition::Always)
            .flags(fixed | WindowFlags::NO_DECORATION | WindowFlags::ALWAYS_AUTO_RESIZE)
            .build(|| {
                if ui.button_with_size("DW", [40.0, 40.0]) {
                    open = !open;
                }
                if ui.is_item_hovered() {
                    ui.tooltip_text("Dragonwilds Trainer");
                }
            });
        self.open = open;
        if !self.open {
            return;
        }

        let mut s = settings::get();
        let mut changed = false;
        ui.window("Dragonwilds Trainer")
            .position([w - 16.0, 72.0], Condition::Always)
            .position_pivot([1.0, 0.0])
            .flags(fixed | WindowFlags::ALWAYS_AUTO_RESIZE)
            .build(|| {
                for f in Feature::ALL {
                    let mut on = f.get(&s.toggles);
                    if ui.checkbox(f.label(s.lang), &mut on) {
                        f.set(&mut s.toggles, on);
                        changed = true;
                    }
                }
                ui.separator();
                let found = game::PLAYER_FOUND.load(Ordering::Relaxed);
                let shared = game::PLAYERS.load(Ordering::Relaxed) > 1;
                ui.text_disabled(match (shared, found, s.lang) {
                    (true, _, Lang::PtBr) => "Outros jogadores no mundo: pausado (só single player)",
                    (true, _, Lang::En) => "Other players in this world: paused (single player only)",
                    (false, true, Lang::PtBr) => "Jogador encontrado",
                    (false, true, Lang::En) => "Player found",
                    (false, false, Lang::PtBr) => "Aguardando o mundo carregar",
                    (false, false, Lang::En) => "Waiting for the world to load",
                });
                for (lang, name) in [(Lang::En, "English"), (Lang::PtBr, "Português")] {
                    if ui.radio_button_bool(name, s.lang == lang) && s.lang != lang {
                        s.lang = lang;
                        changed = true;
                    }
                    ui.same_line();
                }
                ui.new_line();
            });
        if changed {
            settings::set(s);
        }
    }

    /// Clicks on the icon or the panel must not reach the game.
    fn message_filter(&self, io: &Io) -> MessageFilter {
        if self.icon && io.want_capture_mouse {
            MessageFilter::InputMouse
        } else {
            MessageFilter::empty()
        }
    }
}
