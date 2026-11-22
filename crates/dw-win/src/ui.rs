//! In-game overlay. While the trainer boots, a card in the top-right corner shows
//! each step; once it is ready, `Esc` shows an icon there and clicking it opens
//! the list of switches. The icon hides again when the game hides its cursor
//! (back to gameplay). Colours and shapes follow `assets/banner.svg`.

use crate::{game, settings};
use dw_core::features::{Feature, Lang};
use hudhook::imgui::{Condition, Context, FontConfig, FontSource, Io, ProgressBar, StyleColor, StyleVar, Ui, WindowFlags};
use hudhook::{ImguiRenderLoop, MessageFilter, RenderContext};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorInfo, CURSORINFO, CURSOR_SHOWING};

/// After `Esc`, how long the game gets to show its cursor before the icon gives up.
const CURSOR_GRACE: Duration = Duration::from_millis(600);
/// How long the card stays up after the trainer is ready, or after it failed.
const READY_CARD: Duration = Duration::from_secs(10);
const FAILED_CARD: Duration = Duration::from_secs(45);

// Banner palette.
const BG: [f32; 4] = rgb(0x15120f, 0.96);
const TITLE: [f32; 4] = rgb(0x231c16, 1.0);
const LINE: [f32; 4] = rgb(0x3a3127, 1.0);
const TEXT: [f32; 4] = rgb(0xd9cfc1, 1.0);
const TEXT_OFF: [f32; 4] = rgb(0xa39888, 1.0);
const MUTED: [f32; 4] = rgb(0x7d7366, 1.0);
const BOX_OFF: [f32; 4] = rgb(0x6b5f52, 1.0);
const EMBER: [f32; 4] = rgb(0xe0782f, 1.0);
const EMBER_HOVER: [f32; 4] = rgb(0xf08a42, 1.0);
const INK: [f32; 4] = rgb(0x1b140f, 1.0);
const ICON_BG: [f32; 4] = rgb(0x2a221b, 1.0);
const MOSS: [f32; 4] = rgb(0x8fc0a1, 1.0);
const AMBER: [f32; 4] = rgb(0xf0b04a, 1.0);
const RED: [f32; 4] = rgb(0xe06a4f, 1.0);

const fn rgb(hex: u32, a: f32) -> [f32; 4] {
    [((hex >> 16) & 0xff) as f32 / 255.0, ((hex >> 8) & 0xff) as f32 / 255.0, (hex & 0xff) as f32 / 255.0, a]
}

/// Boot steps, in order. The card shows the first four as a progress bar.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd)]
pub enum Stage {
    Engine,
    Menu,
    Reading,
    Hooking,
    Ready,
    Failed,
}

static STAGE: AtomicU8 = AtomicU8::new(Stage::Engine as u8);
/// Milliseconds since boot when the current stage began.
static STAGE_AT: AtomicU64 = AtomicU64::new(0);
static FAILURE: Mutex<String> = Mutex::new(String::new());

fn boot() -> Instant {
    static BOOT: OnceLock<Instant> = OnceLock::new();
    *BOOT.get_or_init(Instant::now)
}

fn current() -> Stage {
    match STAGE.load(Ordering::Relaxed) {
        0 => Stage::Engine,
        1 => Stage::Menu,
        2 => Stage::Reading,
        3 => Stage::Hooking,
        4 => Stage::Ready,
        _ => Stage::Failed,
    }
}

fn in_stage() -> Duration {
    boot().elapsed().saturating_sub(Duration::from_millis(STAGE_AT.load(Ordering::Relaxed)))
}

pub fn stage(s: Stage) {
    STAGE_AT.store(boot().elapsed().as_millis() as u64, Ordering::Relaxed);
    STAGE.store(s as u8, Ordering::Relaxed);
}

pub fn fail(reason: &str) {
    *FAILURE.lock().unwrap_or_else(|e| e.into_inner()) = reason.to_string();
    stage(Stage::Failed);
}

/// Bar position: whole steps done, plus an eased share of the running one.
/// The share never reaches the next step, so the bar only fills when it is real.
fn progress(s: Stage, t: f32) -> f32 {
    let (done, part) = match s {
        Stage::Engine => (0.0, 0.0),
        Stage::Menu => (1.0, (t / 20.0).min(0.95)), // the fixed 20 s wait
        Stage::Reading => (2.0, 0.9 * (1.0 - (-t / 15.0).exp())),
        Stage::Hooking => (3.0, (t / 6.0).min(0.95)),
        Stage::Ready | Stage::Failed => (4.0, 0.0),
    };
    (done + part) / 4.0
}

fn step_label(s: Stage, lang: Lang) -> &'static str {
    match (s, lang) {
        (Stage::Engine, Lang::PtBr) => "Procurando o motor do jogo",
        (Stage::Engine, Lang::En) => "Finding the game engine",
        (Stage::Menu, Lang::PtBr) => "Esperando o menu do jogo",
        (Stage::Menu, Lang::En) => "Waiting for the main menu",
        (Stage::Reading, Lang::PtBr) => "Lendo os dados do jogo",
        (Stage::Reading, Lang::En) => "Reading the game data",
        (Stage::Hooking, Lang::PtBr) => "Conectando ao jogo",
        (Stage::Hooking, Lang::En) => "Hooking into the game",
        (Stage::Ready, Lang::PtBr) => "Pronto",
        (Stage::Ready, Lang::En) => "Ready",
        (Stage::Failed, Lang::PtBr) => "Não carregou",
        (Stage::Failed, Lang::En) => "Did not load",
    }
}

#[derive(Default)]
pub struct Panel {
    icon: bool,
    open: bool,
    esc_down: bool,
    cursor_seen: bool,
    shown_at: Option<Instant>,
    /// Indexes into the font atlas (FontId is not Send): regular, semibold.
    fonts: Option<(usize, usize)>,
    scale: f32,
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

    fn bold<'u>(&self, ui: &'u Ui) -> Option<hudhook::imgui::FontStackToken<'u>> {
        let (_, b) = self.fonts?;
        ui.fonts().fonts().get(b).map(|&id| ui.push_font(id))
    }

    /// Top-right card while booting, then briefly "ready" or the failure reason.
    fn boot_card(&self, ui: &Ui, w: f32, lang: Lang) {
        let s = current();
        let t = in_stage();
        let linger = match s {
            Stage::Ready => READY_CARD,
            Stage::Failed => FAILED_CARD,
            _ => Duration::MAX,
        };
        if t > linger {
            return;
        }
        let k = self.scale;
        ui.window("##dw-boot")
            .position([w - 16.0 * k, 16.0 * k], Condition::Always)
            .position_pivot([1.0, 0.0])
            .size([340.0 * k, 0.0], Condition::Always)
            .flags(
                WindowFlags::NO_DECORATION
                    | WindowFlags::NO_INPUTS
                    | WindowFlags::NO_MOVE
                    | WindowFlags::NO_SAVED_SETTINGS
                    | WindowFlags::NO_FOCUS_ON_APPEARING
                    | WindowFlags::NO_NAV,
            )
            .build(|| {
                {
                    let _b = self.bold(ui);
                    ui.text_colored(rgb(0xe9e1d5, 1.0), "Dragonwilds Trainer");
                }
                let secs = boot().elapsed().as_secs();
                let clock = format!("{}:{:02}", secs / 60, secs % 60);
                let right = ui.content_region_avail()[0] + ui.cursor_pos()[0] - ui.calc_text_size(&clock)[0];
                ui.same_line_with_pos(right);
                ui.text_colored(MUTED, &clock);

                match s {
                    Stage::Ready => {
                        ui.text_colored(MOSS, step_label(s, lang));
                        ui.text_colored(TEXT_OFF, match lang {
                            Lang::PtBr => "Aperte Esc e clique em DW",
                            Lang::En => "Press Esc and click DW",
                        });
                    }
                    Stage::Failed => {
                        ui.text_colored(RED, step_label(s, lang));
                        let why = FAILURE.lock().map(|f| f.clone()).unwrap_or_default();
                        ui.text_wrapped(&why);
                        ui.text_colored(MUTED, match lang {
                            Lang::PtBr => "Detalhes em dw-trainer\\trainer.log",
                            Lang::En => "Details in dw-trainer\\trainer.log",
                        });
                    }
                    _ => {
                        let n = s as u8 + 1;
                        ui.text_colored(TEXT, format!(
                            "{} · {}",
                            step_label(s, lang),
                            match lang {
                                Lang::PtBr => format!("passo {n} de 4"),
                                Lang::En => format!("step {n} of 4"),
                            }
                        ));
                    }
                }
                let fill = progress(s, t.as_secs_f32());
                let _c = ui.push_style_color(StyleColor::PlotHistogram, if s == Stage::Failed { RED } else { EMBER });
                ProgressBar::new(fill).size([-1.0, 6.0 * k]).overlay_text("").build(ui);
            });
    }
}

impl ImguiRenderLoop for Panel {
    fn initialize<'a>(&'a mut self, ctx: &mut Context, _render_context: &'a mut dyn RenderContext) {
        let [_, h] = ctx.io().display_size;
        let k = (h / 1080.0).clamp(1.0, 2.5);
        self.scale = k;

        // Segoe UI ships with every Windows; the default pixel font is the fallback.
        let font = |file: &str| std::fs::read(format!("C:\\Windows\\Fonts\\{file}")).ok();
        if let (Some(regular), Some(semibold)) = (font("segoeui.ttf"), font("seguisb.ttf")) {
            let cfg = || Some(FontConfig { oversample_h: 2, ..FontConfig::default() });
            let size = 17.0 * k;
            let atlas = ctx.fonts();
            atlas.add_font(&[FontSource::TtfData { data: &regular, size_pixels: size, config: cfg() }]);
            atlas.add_font(&[FontSource::TtfData { data: &semibold, size_pixels: size, config: cfg() }]);
            self.fonts = Some((0, 1));
        } else {
            ctx.fonts().add_font(&[FontSource::DefaultFontData {
                config: Some(FontConfig { size_pixels: 13.0 * k, ..FontConfig::default() }),
            }]);
        }

        let st = ctx.style_mut();
        st.window_rounding = 10.0;
        st.window_border_size = 1.0;
        st.window_padding = [18.0, 14.0];
        st.window_title_align = [0.0, 0.5];
        st.frame_rounding = 4.0;
        st.frame_padding = [4.0, 3.0];
        st.frame_border_size = 1.0;
        st.item_spacing = [10.0, 9.0];
        st.item_inner_spacing = [10.0, 6.0];
        st.popup_rounding = 6.0;
        st.grab_rounding = 4.0;
        st.scale_all_sizes(k);
        let c = &mut st.colors;
        for (slot, color) in [
            (StyleColor::Text, TEXT),
            (StyleColor::TextDisabled, MUTED),
            (StyleColor::WindowBg, BG),
            (StyleColor::PopupBg, BG),
            (StyleColor::Border, LINE),
            (StyleColor::BorderShadow, [0.0; 4]),
            (StyleColor::FrameBg, TITLE),
            (StyleColor::FrameBgHovered, LINE),
            (StyleColor::FrameBgActive, LINE),
            (StyleColor::TitleBg, TITLE),
            (StyleColor::TitleBgActive, TITLE),
            (StyleColor::TitleBgCollapsed, TITLE),
            (StyleColor::CheckMark, EMBER),
            (StyleColor::Button, ICON_BG),
            (StyleColor::ButtonHovered, LINE),
            (StyleColor::ButtonActive, EMBER),
            (StyleColor::Header, LINE),
            (StyleColor::HeaderHovered, LINE),
            (StyleColor::HeaderActive, EMBER),
            (StyleColor::Separator, LINE),
            (StyleColor::PlotHistogram, EMBER),
            (StyleColor::NavHighlight, EMBER),
        ] {
            c[slot as usize] = color;
        }
    }

    fn render(&mut self, ui: &mut Ui) {
        self.update_visibility();
        let [w, _] = ui.io().display_size;
        let lang = settings::get().lang;
        if !self.icon {
            self.boot_card(ui, w, lang);
            return;
        }
        let k = self.scale;
        let fixed = WindowFlags::NO_MOVE | WindowFlags::NO_RESIZE | WindowFlags::NO_COLLAPSE | WindowFlags::NO_SAVED_SETTINGS;

        let mut open = self.open;
        {
            let _b = self.bold(ui);
            let _wp = ui.push_style_var(StyleVar::WindowPadding([0.0, 0.0]));
            let _wb = ui.push_style_var(StyleVar::WindowBorderSize(0.0));
            let _r = ui.push_style_var(StyleVar::FrameRounding(8.0 * k));
            let _bs = ui.push_style_var(StyleVar::FrameBorderSize(1.5 * k));
            let _bc = ui.push_style_color(StyleColor::Border, EMBER);
            let _bg = ui.push_style_color(StyleColor::Button, if open { EMBER } else { ICON_BG });
            let _tx = ui.push_style_color(StyleColor::Text, if open { INK } else { TEXT });
            ui.window("##dw-icon")
                .position([w - 16.0 * k, 16.0 * k], Condition::Always)
                .position_pivot([1.0, 0.0])
                .bg_alpha(0.0)
                .flags(fixed | WindowFlags::NO_DECORATION | WindowFlags::ALWAYS_AUTO_RESIZE)
                .build(|| {
                    if ui.button_with_size("DW", [44.0 * k, 44.0 * k]) {
                        open = !open;
                    }
                    if ui.is_item_hovered() {
                        ui.tooltip_text("Dragonwilds Trainer");
                    }
                });
        }
        self.open = open;
        if !self.open {
            return;
        }

        let mut s = settings::get();
        let mut changed = false;
        let bold = self.bold(ui);
        ui.window("Dragonwilds Trainer")
            .position([w - 16.0 * k, 72.0 * k], Condition::Always)
            .position_pivot([1.0, 0.0])
            .flags(fixed | WindowFlags::ALWAYS_AUTO_RESIZE)
            .build(|| {
                // The title bar used the semibold font; the body goes back to regular.
                let _regular = self.fonts.and_then(|(r, _)| ui.fonts().fonts().get(r).map(|&id| ui.push_font(id)));
                for f in Feature::ALL {
                    let mut on = f.get(&s.toggles);
                    // Banner checkboxes: filled ember with a dark tick when on, an outline when off.
                    let _fb = ui.push_style_color(StyleColor::FrameBg, if on { EMBER } else { [0.0; 4] });
                    let _fh = ui.push_style_color(StyleColor::FrameBgHovered, if on { EMBER_HOVER } else { LINE });
                    let _bd = ui.push_style_color(StyleColor::Border, if on { EMBER } else { BOX_OFF });
                    let _ck = ui.push_style_color(StyleColor::CheckMark, INK);
                    let _tx = ui.push_style_color(StyleColor::Text, if on { TEXT } else { TEXT_OFF });
                    if ui.checkbox(f.label(s.lang), &mut on) {
                        f.set(&mut s.toggles, on);
                        changed = true;
                    }
                }
                ui.separator();
                let players = game::PLAYERS.load(Ordering::Relaxed);
                let found = game::PLAYER_FOUND.load(Ordering::Relaxed);
                match current() {
                    Stage::Ready => {}
                    Stage::Failed => {
                        ui.text_colored(RED, match s.lang {
                            Lang::PtBr => "Não carregou: veja dw-trainer\\trainer.log",
                            Lang::En => "Did not load: see dw-trainer\\trainer.log",
                        });
                    }
                    st => {
                        ui.text_colored(AMBER, format!(
                            "{}: {}",
                            match s.lang {
                                Lang::PtBr => "Carregando",
                                Lang::En => "Loading",
                            },
                            step_label(st, s.lang)
                        ));
                    }
                }
                if current() == Stage::Ready {
                    if players > 1 {
                        ui.text_colored(AMBER, match s.lang {
                            Lang::PtBr => format!("Mundo compartilhado ({players} jogadores)"),
                            Lang::En => format!("Shared world ({players} players)"),
                        });
                        if ui.is_item_hovered() {
                            ui.tooltip_text(match s.lang {
                                Lang::PtBr => "As diretrizes da Jagex só permitem cheats no single player.\nUse apenas com quem está no mundo e concorda.",
                                Lang::En => "Jagex's guidelines allow cheats in single player only.\nUse it only with everyone in the world agreeing.",
                            });
                        }
                    } else {
                        ui.text_colored(MUTED, match (found, s.lang) {
                            (true, Lang::PtBr) => "Jogador encontrado",
                            (true, Lang::En) => "Player found",
                            (false, Lang::PtBr) => "Aguardando o mundo carregar",
                            (false, Lang::En) => "Waiting for the world to load",
                        });
                    }
                }
                for (lang, name) in [(Lang::En, "English"), (Lang::PtBr, "Português")] {
                    let _ck = ui.push_style_color(StyleColor::CheckMark, EMBER);
                    if ui.radio_button_bool(name, s.lang == lang) && s.lang != lang {
                        s.lang = lang;
                        changed = true;
                    }
                    ui.same_line();
                }
                ui.new_line();
            });
        drop(bold);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_only_fills_when_a_step_really_ends() {
        // Every running step stays below the next one, however long it takes.
        for (s, next) in [(Stage::Menu, 0.5), (Stage::Reading, 0.75), (Stage::Hooking, 1.0)] {
            assert!(progress(s, 1.0e6) < next, "{} reached the next step", s as u8);
            assert_eq!(progress(s, 0.0), next - 0.25, "{} starts where the last step ended", s as u8);
        }
        assert_eq!(progress(Stage::Ready, 0.0), 1.0);
        assert!(progress(Stage::Reading, 5.0) < progress(Stage::Reading, 30.0), "moves while waiting");
    }
}
