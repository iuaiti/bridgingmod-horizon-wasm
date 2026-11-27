// The game, as `runtime`'s search sees it.
//
// `runtime` is the same game-free code as the 1.0.1 `.hnmod`; this module is
// the only place that knows about the SDK. It answers the two traits the
// search asks for — `World` and `SlabWorld` — from handle reads, so the
// algorithm above it does not change between containers.

use mod_api::{BlockState, ClientHost, Hand, HostBlockStates, LevelRef, PlayerRef};

use crate::runtime::{Adjacency, AxisMode, AxisOverride, Config, Pos, SlabWorld, World};

/// Where the search reads the world from, and for which hand.
pub struct GameWorld<'a> {
    host: &'a dyn ClientHost,
    level: LevelRef,
    player: PlayerRef,
    hand: Hand,
    crouching: bool,
    skip_torches: bool,
}

/// What a frame needs from the game before the search runs.
#[derive(Clone, Copy, Debug)]
pub struct FrameInputs {
    pub level: LevelRef,
    pub player: PlayerRef,
    pub eye: [f64; 3],
    pub look: [f64; 3],
    pub reach: f64,
    pub crouching: bool,
    pub on_ground: bool,
    pub feet: [f64; 3],
    /// A block or entity is already under the crosshair.
    pub has_target: bool,
}

/// Whether `name` is a torch block (`minecraft:torch`, `soul_torch`, `redstone_torch`, etc.).
pub fn is_torch_block(name: &str) -> bool {
    name.ends_with(":torch") || name.ends_with("_torch")
}

fn is_air_name(name: &str) -> bool {
    matches!(
        name,
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

fn held_placeable_name(
    host: &dyn ClientHost,
    player: PlayerRef,
    hand: Hand,
    skip_torches: bool,
) -> Option<String> {
    let stack = host.living(player.entity())?.hand_item(host, hand)?;
    let block = host.item_block(stack.item())?;
    let name = block.id(host);
    if skip_torches && is_torch_block(&name) {
        return None;
    }
    Some(name)
}

/// Whether `hand` holds a block item eligible for bridging assist.
pub fn hand_is_placeable(
    host: &dyn ClientHost,
    player: PlayerRef,
    hand: Hand,
    skip_torches: bool,
) -> bool {
    held_placeable_name(host, player, hand, skip_torches).is_some()
}

impl<'a> GameWorld<'a> {
    pub fn new(
        host: &'a dyn ClientHost,
        inputs: &FrameInputs,
        hand: Hand,
        skip_torches: bool,
    ) -> Self {
        Self {
            host,
            level: inputs.level,
            player: inputs.player,
            hand,
            crouching: inputs.crouching,
            skip_torches,
        }
    }

    fn state(&self, pos: Pos) -> Option<BlockState> {
        self.level
            .block_state(self.host, [pos.x, pos.y, pos.z])
    }

    /// The block's name (`minecraft:oak_slab`), for the slab, trapdoor and
    /// torch checks.
    fn name_of(&self, state: BlockState) -> String {
        state.block().id(self.host)
    }

    /// The block the held item places, by name, or none.
    fn held_name(&self) -> Option<String> {
        held_placeable_name(self.host, self.player, self.hand, self.skip_torches)
    }
}

impl World for GameWorld<'_> {
    fn is_air(&self, pos: Pos) -> bool {
        match self.state(pos) {
            Some(state) => is_air_name(&self.name_of(state)),
            None => true,
        }
    }

    fn is_replaceable(&self, pos: Pos) -> bool {
        // A block is replaceable when it is air or a plant/snow/fluid the mod
        // may overwrite; the SDK exposes the block's own flag through the
        // metadata read on the state's block.
        self.state(pos).is_some_and(|state| {
            is_air_name(&self.name_of(state))
                || self.host.block_states().replaceable(state).unwrap_or(false)
        })
    }

    fn is_liquid(&self, pos: Pos) -> bool {
        self.level
            .fluid_state(self.host, [pos.x, pos.y, pos.z])
            .is_some_and(|fluid| !fluid.is_empty())
    }

    fn player_overlaps_block(&self, pos: Pos) -> bool {
        let Some(bounds) = self.player.entity().bounding_box(self.host) else {
            return false;
        };
        let (min, max) = (bounds[0], bounds[1]);
        let block_min = [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)];
        let block_max = [block_min[0] + 1.0, block_min[1] + 1.0, block_min[2] + 1.0];
        min[0] < block_max[0]
            && max[0] > block_min[0]
            && min[1] < block_max[1]
            && max[1] > block_min[1]
            && min[2] < block_max[2]
            && max[2] > block_min[2]
    }

    fn holding_placeable(&self) -> bool {
        self.held_name().is_some()
    }

    fn crouching(&self) -> bool {
        self.crouching
    }
}

impl SlabWorld for GameWorld<'_> {
    fn slab_half(&self, pos: Pos) -> Option<bool> {
        let state = self.state(pos)?;
        if !self.name_of(state).ends_with("_slab") {
            return None;
        }
        match self
            .level
            .block_property(self.host, [pos.x, pos.y, pos.z], "type")
            .as_deref()
        {
            Some("top") => Some(true),
            Some("bottom") => Some(false),
            _ => None,
        }
    }

    fn holding_slab(&self) -> bool {
        self.held_name().is_some_and(|n| n.ends_with("_slab"))
    }

    fn holding_trapdoor(&self) -> bool {
        self.held_name().is_some_and(|n| n.ends_with("_trapdoor"))
    }
}

/// The mod's live settings, read from the typed Options > Mods getters.
pub fn config(host: &dyn ClientHost) -> Config {
    let defaults = Config::default();
    Config {
        enabled: host.setting_bool("enabled").unwrap_or(defaults.enabled),
        min_bridge_distance: host
            .setting_f64("min_distance")
            .filter(|v| v.is_finite())
            .map(|v| v.clamp(0.0, 100.0))
            .unwrap_or(defaults.min_bridge_distance),
        only_bridge_when_crouched: host
            .setting_bool("only_crouch")
            .unwrap_or(defaults.only_bridge_when_crouched),
        supported_axes: host
            .setting_choice("axes")
            .and_then(AxisMode::from_choice)
            .unwrap_or(defaults.supported_axes),
        supported_axes_when_crouched: host
            .setting_choice("axes_crouched")
            .and_then(AxisOverride::from_choice)
            .unwrap_or(defaults.supported_axes_when_crouched),
        delay_post_bridging: host
            .setting_f64("delay")
            .filter(|v| v.is_finite())
            .map(|v| v.round().clamp(0.0, 40.0) as i32)
            .unwrap_or(defaults.delay_post_bridging),
        show_crosshair: host
            .setting_bool("show_crosshair")
            .unwrap_or(defaults.show_crosshair),
        show_outline: host
            .setting_bool("show_outline")
            .unwrap_or(defaults.show_outline),
        skip_torch_bridging: host
            .setting_bool("skip_torches")
            .unwrap_or(defaults.skip_torch_bridging),
        enable_slab_assist: host
            .setting_bool("slab_assist")
            .unwrap_or(defaults.enable_slab_assist),
        enable_non_solid_replace: host
            .setting_bool("replace_plants")
            .unwrap_or(defaults.enable_non_solid_replace),
        bridging_adjacency: host
            .setting_choice("adjacency")
            .and_then(Adjacency::from_choice)
            .unwrap_or(defaults.bridging_adjacency),
        show_debug_highlight: host
            .setting_bool("show_debug_highlight")
            .unwrap_or(defaults.show_debug_highlight),
        show_debug_trace: host
            .setting_bool("show_debug_trace")
            .unwrap_or(defaults.show_debug_trace),
        ..defaults
    }
}

/// The hand whose item would place: the main hand's, or the off hand's when
/// the main hand holds no eligible block (vanilla tries them in that order).
pub fn placing_hand(host: &dyn ClientHost, player: PlayerRef, skip_torches: bool) -> Option<Hand> {
    if hand_is_placeable(host, player, Hand::Main, skip_torches) {
        Some(Hand::Main)
    } else if hand_is_placeable(host, player, Hand::Off, skip_torches) {
        Some(Hand::Off)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::is_torch_block;

    #[test]
    fn torch_filter_matches_torches_only() {
        assert!(is_torch_block("minecraft:torch"));
        assert!(is_torch_block("minecraft:soul_torch"));
        assert!(is_torch_block("minecraft:redstone_torch"));
        assert!(is_torch_block("minecraft:copper_torch"));
        assert!(is_torch_block("example:magic_torch"));

        assert!(!is_torch_block("minecraft:torchflower"));
        assert!(!is_torch_block("minecraft:stone"));
        assert!(!is_torch_block("minecraft:oak_slab"));
    }
}
