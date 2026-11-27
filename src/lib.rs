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

    /// The chevron over the crosshair.
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
        // a support above the gap draws the "down" chevron.
        let alignment = runtime::placement_alignment(target.side);
        let icon = 32.0;
        let [w, h] = draw.size();
        let x = ((w - icon + 1.0) / 2.0).floor();
        let mut y = ((h - icon + 1.0) / 2.0).floor();
        // The F3 debug screen covers the middle of the screen: the 1.0.1
        // bundle pushed the chevron down 15 GUI pixels there.
        if !host.native_corner_shown(Corner::TopLeft) {
            y += 15.0;
        }
        chevron(draw, [x, y], icon, alignment, runtime::INDICATOR_TINT);
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
        if config.show_outline {
            push_block_outline(&mut lines, target.pos, config.outline_colour, 2.0);
        }
        if f3_open && config.show_debug_highlight {
            push_block_outline(&mut lines, target.pos, 0xCC_00_FF_88, 2.0);
            push_block_outline(
                &mut lines,
                target.pos.offset(target.side),
                0xCC_FF_AA_00,
                2.0,
            );
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

/// Compute the two quad arms of the direction chevron in GUI pixels.
fn chevron_quads(at: [f32; 2], size: f32, alignment: &str) -> [[[f32; 2]; 4]; 2] {
    // The arrow points the way the block goes: "up" means the block lands
    // above the crosshair, so the bar points up.
    let (dx, dy) = match alignment {
        "up" => (0.0, -1.0),
        "down" => (0.0, 1.0),
        _ => (1.0, 0.0),
    };
    let [x, y] = at;
    let half = size / 2.0;
    let cx = x + half;
    let cy = y + half;
    // The tip, and the two ends of the arms, `thick` wide.
    let arm = half * 0.55;
    let thick = size * 0.16;
    let tip = [cx + dx * arm, cy + dy * arm];
    // Perpendicular to the direction.
    let (px, py) = (-dy, dx);
    let mut quads = [[[0.0; 2]; 4]; 2];
    for (idx, side) in [-1.0_f32, 1.0].into_iter().enumerate() {
        let base = [
            cx - dx * arm * 0.15 + px * arm * side,
            cy - dy * arm * 0.15 + py * arm * side,
        ];
        // A bar from the tip to a base corner, offset perpendicular to the arm.
        let (vx, vy) = (base[0] - tip[0], base[1] - tip[1]);
        let len = (vx * vx + vy * vy).sqrt().max(0.0001);
        let (ux, uy) = ((-vy / len) * (thick / 2.0), (vx / len) * (thick / 2.0));
        quads[idx] = [
            [tip[0] + ux, tip[1] + uy],
            [base[0] + ux, base[1] + uy],
            [base[0] - ux, base[1] - uy],
            [tip[0] - ux, tip[1] - uy],
        ];
    }
    quads
}

/// Draw the direction chevron: two thick bars meeting at a point, in one colour.
fn chevron(draw: &mut dyn DrawContext, at: [f32; 2], size: f32, alignment: &str, argb: u32) {
    for quad in chevron_quads(at, size, alignment) {
        draw.quad(quad, argb);
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
    use super::chevron_quads;

    fn quad_area(q: [[f32; 2]; 4]) -> f32 {
        let mut sum = 0.0_f32;
        for i in 0..4 {
            let [x1, y1] = q[i];
            let [x2, y2] = q[(i + 1) % 4];
            sum += x1 * y2 - x2 * y1;
        }
        0.5 * sum.abs()
    }

    #[test]
    fn chevron_quads_have_positive_area() {
        for align in ["up", "down", "horizontal"] {
            for quad in chevron_quads([100.0, 100.0], 32.0, align) {
                assert!(
                    quad_area(quad) > 10.0,
                    "chevron alignment {align} produced degenerate quad {quad:?}"
                );
            }
        }
    }
}
