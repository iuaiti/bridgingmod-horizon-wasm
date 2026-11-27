//! BridgingMod for Horizon: reach-around placement ("bridge assist").
//!
//! A port of squeeglii's BridgingMod (MIT) to the Horizon mod SDK, replacing
//! the retired `.hnmod` mixin bundle. `runtime` is the search, the synthetic
//! hit and the settings, unchanged from 1.0.1; `adapter` gives it a world
//! backed by the SDK's `ClientHost`; this file wires the hooks.
//!
//! The bridge click goes out as the game's own `use_item_on` packet, built
//! here and sent through `ClientHost::send_packet`, so the game needs no
//! compiled-in hook and the server sees an ordinary right click on a block.

mod adapter;
mod place;
mod runtime;

/// The player's block reach, as `Game.block_reach` reported it in the 1.0.1
/// bundle: vanilla's 4.5 blocks plus the assist's own margin.
const BLOCK_REACH: f64 = 4.5;

use mod_api::{
    ClientHost, ClientTickPostEvent, Corner, DrawContext, Flow, GuiLayerPostEvent, Hooks,
    HudMode, InputKeyEvent, InteractionKey, InteractionKeyMappingTriggeredEvent, KeyAction,
    KeyRegistration, Registrar, RegistrationContext, SettingSpec, SubmitCustomGeometryEvent, Text,
    WorldDepth, WorldLine,
};

use adapter::{FrameInputs, GameWorld};
pub use runtime::{DEFAULT_KEY, KEY};
use runtime::{Config, ID, Pos};

use std::sync::Mutex;

/// The state kept between frames: the last computed target and post-placement cooldown ticks.
#[derive(Default)]
struct State {
    target: Option<runtime::Assist>,
    delay_ticks: u32,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(State::default))
}

#[derive(Default)]
struct BridgingMod;

/// The settings page under Options > Mods, in the same order as the 1.0.1
/// `.hnmod`, so a returning player finds them where they were.
pub fn settings() -> Vec<SettingSpec> {
    vec![
        SettingSpec::toggle("enabled", "Bridging assist", true),
        SettingSpec::toggle("only_crouch", "Only while crouching", false),
        SettingSpec::choice(
            "axes",
            "Bridge axes",
            ["Both", "Horizontal", "Vertical"],
            0,
        ),
        SettingSpec::choice(
            "axes_crouched",
            "Axes while crouching",
            ["As above", "Both", "Horizontal", "Vertical"],
            0,
        ),
        SettingSpec::range("min_distance", "Minimum distance", 0.0, 100.0, 5.0, 20.0),
        SettingSpec::range("delay", "Delay after bridging", 0.0, 40.0, 1.0, 4.0),
        SettingSpec::toggle("show_crosshair", "Crosshair indicator", true),
        SettingSpec::toggle("show_outline", "Debug box", true),
        SettingSpec::toggle("slab_assist", "Slab assist", true),
        SettingSpec::toggle("replace_plants", "Replace plants and snow", true),
        SettingSpec::toggle("skip_torches", "Skip torches", true),
        SettingSpec::choice(
            "adjacency",
            "Missed corners",
            ["Off", "Faces", "Edges", "Corners"],
            3,
        ),
        SettingSpec::toggle("show_debug_highlight", "F3 debug box", true),
        SettingSpec::toggle("show_debug_trace", "F3 trace", false),
        SettingSpec::toggle("debug_chat", "Debug messages in chat", false),
    ]
}

impl BridgingMod {
    /// One frame: where the crosshair would bridge to, if it hits nothing.
    fn frame(&self, host: &mut dyn ClientHost) {
        with_state(|s| s.delay_ticks = s.delay_ticks.saturating_sub(1));
        if !host.on(ID) || !host.playing() {
            with_state(|s| s.target = None);
            runtime::reset();
            return;
        }
        let config = adapter::config(host);
        if !config.enabled {
            with_state(|s| s.target = None);
            runtime::reset();
            return;
        }
        let Some(inputs) = frame_inputs(host) else {
            with_state(|s| s.target = None);
            runtime::reset();
            return;
        };
        // Vanilla can place or interact: no reach-around.
        if inputs.has_target || !playing_behaviour(&config, inputs.crouching) {
            with_state(|s| s.target = None);
            runtime::reset();
            return;
        }
        // The slab assist's Y fraction, as `Mth.frac(player.getY())`.
        if inputs.on_ground {
            runtime::set_y_frac(runtime::fraction(inputs.feet[1]));
        }
        let Some(hand) = adapter::placing_hand(host, inputs.player, config.skip_torch_bridging)
        else {
            with_state(|s| s.target = None);
            runtime::reset();
            return;
        };
        let world = GameWorld::new(host, &inputs, hand, config.skip_torch_bridging);
        runtime::refresh(
            &world,
            &config,
            inputs.eye,
            inputs.look,
            inputs.eye,
            inputs.look,
            inputs.reach,
        );
        let target = runtime::target();
        with_state(|s| s.target = target);
    }

    /// The key: bridging assist on or off, with a client-only chat line.
    fn key(&self, host: &mut dyn ClientHost, key: &str, pressed: bool) {
        if !host.on(ID) || !host.playing() {
            return;
        }
        let Some((bound, _)) = host.mod_key(ID, KEY) else {
            return;
        };
        if !runtime::toggles_on(&bound, key, pressed) {
            return;
        }
        let mut config = adapter::config(host);
        config.enabled = !config.enabled;
        with_state(|s| s.target = None);
        runtime::reset();
        host.set_setting(
            ID,
            "enabled",
            if config.enabled { "true" } else { "false" },
        );
        let _ = host.chat_line(Text::literal(if config.enabled {
            runtime::ON
        } else {
            runtime::OFF
        }));
    }

    /// The direction indicator flanking the crosshair.
    fn crosshair(&self, host: &dyn ClientHost, layer: &str, draw: &mut dyn DrawContext) {
        if layer != "minecraft:crosshair"
            || !host.on(ID)
            || !host.playing()
            || host.hud_mode() != HudMode::AllVisible
        {
            return;
        }
        let config = adapter::config(host);
        if !config.enabled || !config.show_crosshair {
            return;
        }
        let Some(target) = with_state(|s| s.target) else {
            return;
        };
        // The direction the block goes, from the side the support was found on:
        // a support above the gap draws the "down" indicator.
        let alignment = runtime::placement_alignment(target.side);
        let icon = 32.0;
        let [w, h] = draw.size();
        let x = ((w - icon + 1.0) / 2.0).floor();
        let mut y = ((h - icon + 1.0) / 2.0).floor();
        // The F3 debug screen covers the middle of the screen: the 1.0.1
        // bundle pushed the indicator down 15 GUI pixels there.
        if !host.native_corner_shown(Corner::TopLeft) {
            y += 15.0;
        }
        draw_indicator(draw, [x, y], alignment, runtime::INDICATOR_TINT);
    }

    /// Submit the placement outline and F3 debug/trace boxes in world space.
    fn debug_geometry(&self, host: &dyn ClientHost, event: &mut SubmitCustomGeometryEvent<'_>) {
        if !host.on(ID) || !host.playing() || host.hud_mode() != HudMode::AllVisible {
            return;
        }
        let config = adapter::config(host);
        if !config.enabled {
            return;
        }
        let Some(target) = with_state(|s| s.target) else {
            return;
        };
        let f3_open = !host.native_corner_shown(Corner::TopLeft);
        let mut lines = Vec::new();
        let show_box = if f3_open {
            config.show_debug_highlight
        } else {
            config.show_outline
        };
        if show_box {
            let color = if f3_open {
                0xCC_00_FF_88
            } else {
                config.outline_colour
            };
            push_block_outline(&mut lines, target.pos, color, 1.5);
        }
        if f3_open && config.show_debug_trace {
            for pos in runtime::trace() {
                push_block_outline(&mut lines, pos, 0x66_FF_FF_00, 1.0);
            }
        }
        if !lines.is_empty() {
            let _ = event.draw.lines(&lines);
        }
    }
}

/// Pixel-perfect 32x32 crosshair indicator rectangles matching upstream
/// `squeeglii/BridgingMod` (`indicator/{up,down,horizontal}.png`).
/// All pixels sit outside the central 15x15 vanilla crosshair (`8..23`).
const INDICATOR_UP: &[[f32; 4]] = &[
    // Left up-arrow
    [5.0, 12.0, 1.0, 1.0],
    [4.0, 13.0, 1.0, 1.0],
    [6.0, 13.0, 1.0, 1.0],
    [3.0, 14.0, 1.0, 1.0],
    [7.0, 14.0, 1.0, 1.0],
    [5.0, 15.0, 1.0, 4.0],
    // Right up-arrow
    [25.0, 12.0, 1.0, 1.0],
    [24.0, 13.0, 1.0, 1.0],
    [26.0, 13.0, 1.0, 1.0],
    [23.0, 14.0, 1.0, 1.0],
    [27.0, 14.0, 1.0, 1.0],
    [25.0, 15.0, 1.0, 4.0],
];

const INDICATOR_DOWN: &[[f32; 4]] = &[
    // Left down-arrow
    [5.0, 12.0, 1.0, 4.0],
    [3.0, 16.0, 1.0, 1.0],
    [7.0, 16.0, 1.0, 1.0],
    [4.0, 17.0, 1.0, 1.0],
    [6.0, 17.0, 1.0, 1.0],
    [5.0, 18.0, 1.0, 1.0],
    // Right down-arrow
    [25.0, 12.0, 1.0, 4.0],
    [23.0, 16.0, 1.0, 1.0],
    [27.0, 16.0, 1.0, 1.0],
    [24.0, 17.0, 1.0, 1.0],
    [26.0, 17.0, 1.0, 1.0],
    [25.0, 18.0, 1.0, 1.0],
];

const INDICATOR_HORIZONTAL: &[[f32; 4]] = &[
    // Left `<` bracket
    [7.0, 13.0, 1.0, 1.0],
    [6.0, 14.0, 1.0, 1.0],
    [5.0, 15.0, 1.0, 1.0],
    [6.0, 16.0, 1.0, 1.0],
    [7.0, 17.0, 1.0, 1.0],
    // Right `>` bracket
    [23.0, 13.0, 1.0, 1.0],
    [24.0, 14.0, 1.0, 1.0],
    [25.0, 15.0, 1.0, 1.0],
    [24.0, 16.0, 1.0, 1.0],
    [23.0, 17.0, 1.0, 1.0],
];

fn indicator_rects(alignment: &str) -> &'static [[f32; 4]] {
    match alignment {
        "up" => INDICATOR_UP,
        "down" => INDICATOR_DOWN,
        _ => INDICATOR_HORIZONTAL,
    }
}

/// Draw the flanking direction indicator around the crosshair.
fn draw_indicator(draw: &mut dyn DrawContext, at: [f32; 2], alignment: &str, argb: u32) {
    let [x, y] = at;
    for &[rx, ry, rw, rh] in indicator_rects(alignment) {
        draw.fill([x + rx, y + ry, rw, rh], argb);
    }
}

/// Append the 12 wireframe edges of `pos`'s unit block box (slightly inflated to avoid z-fighting).
fn push_block_outline(lines: &mut Vec<WorldLine>, pos: Pos, color: u32, width: f32) {
    const EPS: f64 = 0.002;
    let x0 = f64::from(pos.x) - EPS;
    let y0 = f64::from(pos.y) - EPS;
    let z0 = f64::from(pos.z) - EPS;
    let x1 = f64::from(pos.x) + 1.0 + EPS;
    let y1 = f64::from(pos.y) + 1.0 + EPS;
    let z1 = f64::from(pos.z) + 1.0 + EPS;

    let edges = [
        ([x0, y0, z0], [x1, y0, z0]),
        ([x0, y1, z0], [x1, y1, z0]),
        ([x0, y0, z1], [x1, y0, z1]),
        ([x0, y1, z1], [x1, y1, z1]),
        ([x0, y0, z0], [x0, y1, z0]),
        ([x1, y0, z0], [x1, y1, z0]),
        ([x0, y0, z1], [x0, y1, z1]),
        ([x1, y0, z1], [x1, y1, z1]),
        ([x0, y0, z0], [x0, y0, z1]),
        ([x1, y0, z0], [x1, y0, z1]),
        ([x0, y1, z0], [x0, y1, z1]),
        ([x1, y1, z0], [x1, y1, z1]),
    ];
    for (from, to) in edges {
        lines.push(WorldLine::new(from, to, color, width, WorldDepth::Tested));
    }
}

/// Whether the assist may act: the mod's own rules, and the crouch rule of
/// the 1.0.1 bundle (`assist_visible`).
fn playing_behaviour(config: &Config, crouching: bool) -> bool {
    config.enabled && (!config.only_bridge_when_crouched || crouching)
}

/// What a frame reads before the search runs.
fn frame_inputs(host: &dyn ClientHost) -> Option<FrameInputs> {
    let player = host.local_player()?;
    let level = player.level(host)?;
    let entity = player.entity();
    let eye = entity.eye_position(host)?;
    let look = entity.view_vector(host)?;
    let feet = entity.position(host)?;
    let reach = host
        .living(entity)
        .and_then(|living| living.attribute_value(host, "minecraft:block_interaction_range"))
        .filter(|r| r.is_finite() && *r > 0.0)
        .unwrap_or(BLOCK_REACH);
    let on_ground = entity.on_ground(host).unwrap_or_else(|| {
        entity
            .velocity(host)
            .is_some_and(|v| v[1] <= 0.0 && v[1] > -0.1)
    });
    let crossing = host.crosshair_target();
    // The live semantic sneak state, sneak key, or crouching pose.
    let crouching = entity.is_shift_key_down(host).unwrap_or(false)
        || host
            .mapping("key.sneak")
            .ok()
            .flatten()
            .is_some_and(|m| m.down)
        || entity.is_crouching(host).unwrap_or(false);
    Some(FrameInputs {
        level,
        player,
        eye,
        look,
        reach,
        crouching,
        on_ground,
        feet,
        has_target: crossing.is_some(),
    })
}

#[mod_api::horizon_mod]
impl Hooks for BridgingMod {
    fn register(&self, r: &mut dyn Registrar, _: &RegistrationContext) {
        r.register_key_mapping(
            KeyRegistration::new(KEY, DEFAULT_KEY).label("Toggle bridging assist"),
        );
        for setting in settings() {
            let _ = r.register_setting(setting);
        }
    }

    fn client_tick_post(&self, host: &mut dyn ClientHost, _: &mut ClientTickPostEvent) {
        self.frame(host);
    }

    fn input_key(&self, host: &mut dyn ClientHost, event: &mut InputKeyEvent<'_>) {
        let pressed = matches!(event.action, KeyAction::Press);
        self.key(host, event.key, pressed);
    }

    /// The right click is about to act on a crosshair that hit nothing: place
    /// in the gap instead, and cancel the click so the game does not also
    /// swing at the air.
    fn interaction_key_mapping_triggered(
        &self,
        host: &mut dyn ClientHost,
        event: &mut InteractionKeyMappingTriggeredEvent,
    ) -> Flow {
        if event.key != InteractionKey::Use || !host.on(ID) || !host.playing() || event.cancelled {
            return Flow::Continue;
        }
        if with_state(|s| s.delay_ticks > 0) {
            return Flow::Continue;
        }
        let config = adapter::config(host);
        let Some(inputs) = frame_inputs(host) else {
            return Flow::Continue;
        };
        let debug = host.setting_bool("debug_chat").unwrap_or(false);
        if debug {
            let line = format!(
                "[bm] use: hand={:?} has_target={} swing={} crouch={} enabled={} eye={:?} look={:?}",
                event.hand,
                inputs.has_target,
                event.swing_hand,
                inputs.crouching,
                config.enabled,
                inputs.eye,
                inputs.look
            );
            let _ = host.chat_line(Text::literal(line));
        }
        if inputs.has_target || !playing_behaviour(&config, inputs.crouching) {
            return Flow::Continue;
        }
        if !adapter::hand_is_placeable(host, inputs.player, event.hand, config.skip_torch_bridging) {
            return Flow::Continue;
        }
        if inputs.on_ground {
            runtime::set_y_frac(runtime::fraction(inputs.feet[1]));
        }
        let world = GameWorld::new(host, &inputs, event.hand, config.skip_torch_bridging);
        let Some(target) = runtime::closest_assist_target(
            &world,
            &config,
            inputs.eye,
            inputs.look,
            inputs.eye,
            inputs.look,
            inputs.reach,
        ) else {
            return Flow::Continue;
        };
        let hit = runtime::place_assist_target(&world, &config, target, runtime::y_frac());
        if debug {
            let line = format!(
                "[bm] target={:?} side={:?} hit pos=({},{},{}) face={:?} hand={:?}",
                target.pos, target.side, hit.pos.x, hit.pos.y, hit.pos.z, hit.face, event.hand
            );
            let _ = host.chat_line(Text::literal(line));
        }
        if place::place(host, inputs.player, event.hand, &hit) {
            if debug {
                let line = format!(
                    "[bm] sent use_item_on at ({},{},{})",
                    hit.pos.x, hit.pos.y, hit.pos.z
                );
                let _ = host.chat_line(Text::literal(line));
            }
            let delay = config.delay_post_bridging.max(0) as u32;
            with_state(|s| {
                s.target = None;
                s.delay_ticks = delay;
            });
            runtime::reset();
            event.cancelled = true;
            return Flow::Cancel;
        }
        if debug {
            let _ = host.chat_line(Text::literal("[bm] place refused"));
        }
        Flow::Continue
    }

    fn gui_layer_post(&self, host: &dyn ClientHost, event: &mut GuiLayerPostEvent<'_>) {
        self.crosshair(host, event.layer, event.draw);
    }

    fn submit_custom_geometry(
        &self,
        host: &dyn ClientHost,
        event: &mut SubmitCustomGeometryEvent<'_>,
    ) {
        self.debug_geometry(host, event);
    }
}

/// The settings page's current values, for the tests.
pub fn default_config() -> Config {
    Config::default()
}

#[cfg(test)]
mod tests {
    use super::indicator_rects;

    #[test]
    fn indicator_matches_upstream_and_clears_crosshair() {
        for (align, expected_pixels) in [("up", 18.0_f32), ("down", 18.0), ("horizontal", 10.0)] {
            let rects = indicator_rects(align);
            let mut total_area = 0.0_f32;
            for &[rx, ry, rw, rh] in rects {
                assert!(rw > 0.0 && rh > 0.0);
                assert!(rx >= 0.0 && rx + rw <= 32.0);
                assert!(ry >= 0.0 && ry + rh <= 32.0);
                assert!(
                    rx + rw <= 8.0 || rx >= 23.0,
                    "indicator {align} overlaps central crosshair: [{rx}, {ry}, {rw}, {rh}]"
                );
                total_area += rw * rh;
            }
            assert_eq!(total_area, expected_pixels);
        }
    }
}
