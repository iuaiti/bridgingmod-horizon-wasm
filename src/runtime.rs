// BridgingMod for Horizon: reach-around (bridge assist) placement.
//
// A port of squeeglii's BridgingMod (MIT) from Fabric/NeoForge to a Horizon
// native mixin bundle, rewritten in Rust. When the crosshair hits nothing and
// the player holds a placeable item, this searches the blocks along the view
// line for one that has a solid neighbour to build off of, in the direction
// the player is looking, and hands the game a synthetic block hit so the use
// places there. A small indicator beside the crosshair shows the direction,
// and a debug box shows the exact position (F3).
//
// This module is the game-free half: the search, the hit it makes and the
// settings. `adapter.rs` gives it a `World` backed by the SDK's `ClientHost`,
// and `lib.rs` wires the SDK hooks to it.

use std::cell::RefCell;

/// The mod's id: its settings folder and its key's line.
pub const ID: &str = "bridgingmod_horizon";
/// The toggle key's name, and its default (upstream's comma).
pub const KEY: &str = "toggle";
pub const DEFAULT_KEY: &str = "key.keyboard.comma";
/// The chevron's tint: opaque white, so the mod's own image shows as drawn.
pub const INDICATOR_TINT: u32 = 0xFFFF_FFFF;

/// Said in local chat when the toggle key turns bridging assist on or off.
pub const ON: &str = "Bridging assist: ON";
pub const OFF: &str = "Bridging assist: OFF";

/// The name of "no key" (`options.txt`'s `key_key.unknown`): a mod key the
/// player cleared never matches a real event.
pub const UNBOUND_KEY: &str = "key.keyboard.unknown";

/// Whether the toggle key acts: `pressed` on the key the player bound to the
/// mod, or on the mod's own default until they change it.
///
/// `native_mod_key` answers the default when the player has set no override,
/// so a fresh install hands the default back here: refusing the default would
/// leave the toggle dead until the player rebound it. An unbound or empty key
/// never acts.
pub fn toggles_on(bound: &str, key: &str, pressed: bool) -> bool {
    pressed && !bound.is_empty() && bound != UNBOUND_KEY && bound == key
}

/// `Path.NEAR_ZERO`.
pub const NEAR_ZERO: f64 = 0.01;
/// `Path.CUBE_EXTENT`: the snap boxes are 60% of a block.
const CUBE_EXTENT: [f64; 3] = [0.5, 0.5, 0.5];
/// `GameSupport.TRAPDOOR_HEIGHT`.
const TRAPDOOR_HEIGHT: f64 = 3.0 / 16.0;
/// `GameSupport.SLAB_HEIGHT`.
const SLAB_HEIGHT: f64 = 8.0 / 16.0;
/// `PathTraversalHandler.DIRECTION_SIMILARITY_THRESHOLD`.
const DIRECTION_SIMILARITY_THRESHOLD: f64 = 0.1;

/// A block position (`net.minecraft.core.BlockPos`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Pos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Pos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
    pub fn offset(self, d: Dir) -> Self {
        let v = d.unit();
        Self::new(self.x + v[0], self.y + v[1], self.z + v[2])
    }
    pub fn containing(v: [f64; 3]) -> Self {
        Self::new(
            v[0].floor() as i32,
            v[1].floor() as i32,
            v[2].floor() as i32,
        )
    }
    pub fn center(self) -> [f64; 3] {
        [
            f64::from(self.x) + 0.5,
            f64::from(self.y) + 0.5,
            f64::from(self.z) + 0.5,
        ]
    }
    pub fn bottom_center(self) -> [f64; 3] {
        [
            f64::from(self.x) + 0.5,
            f64::from(self.y),
            f64::from(self.z) + 0.5,
        ]
    }
    pub fn dist_manhattan(self, other: Self) -> i32 {
        (self.x - other.x).abs() + (self.y - other.y).abs() + (self.z - other.z).abs()
    }
}

/// A block face (`net.minecraft.core.Direction`), vanilla's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dir {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Dir {
    pub const ALL: [Dir; 6] = [
        Dir::Down,
        Dir::Up,
        Dir::North,
        Dir::South,
        Dir::West,
        Dir::East,
    ];
    pub fn unit(self) -> [i32; 3] {
        match self {
            Dir::Down => [0, -1, 0],
            Dir::Up => [0, 1, 0],
            Dir::North => [0, 0, -1],
            Dir::South => [0, 0, 1],
            Dir::West => [-1, 0, 0],
            Dir::East => [1, 0, 0],
        }
    }
    pub fn opposite(self) -> Self {
        match self {
            Dir::Down => Dir::Up,
            Dir::Up => Dir::Down,
            Dir::North => Dir::South,
            Dir::South => Dir::North,
            Dir::West => Dir::East,
            Dir::East => Dir::West,
        }
    }
    pub fn is_horizontal(self) -> bool {
        matches!(self, Dir::North | Dir::South | Dir::West | Dir::East)
    }
}

/// `PlacementAxisMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisMode {
    Horizontal,
    Vertical,
    Both,
}

impl AxisMode {
    pub fn is_direction_enabled(self, d: Dir) -> bool {
        match self {
            AxisMode::Both => true,
            AxisMode::Horizontal => d.is_horizontal(),
            AxisMode::Vertical => !d.is_horizontal(),
        }
    }
    pub fn from_choice(index: u32) -> Option<Self> {
        match index {
            0 => Some(AxisMode::Both),
            1 => Some(AxisMode::Horizontal),
            2 => Some(AxisMode::Vertical),
            _ => None,
        }
    }
}

/// `PlacementAxisModeOverride` (the crouched case); `Fallback` keeps the base mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisOverride {
    Horizontal,
    Vertical,
    Both,
    Fallback,
}

impl AxisOverride {
    pub fn resolve(self, base: AxisMode) -> AxisMode {
        match self {
            AxisOverride::Fallback => base,
            AxisOverride::Horizontal => AxisMode::Horizontal,
            AxisOverride::Vertical => AxisMode::Vertical,
            AxisOverride::Both => AxisMode::Both,
        }
    }
    pub fn from_choice(index: u32) -> Option<Self> {
        match index {
            0 => Some(AxisOverride::Fallback),
            1 => Some(AxisOverride::Both),
            2 => Some(AxisOverride::Horizontal),
            3 => Some(AxisOverride::Vertical),
            _ => None,
        }
    }
}

/// `BridgingAdjacency`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adjacency {
    Disabled,
    Faces,
    Edges,
    Corners,
}

impl Adjacency {
    pub fn enabled(self) -> bool {
        self != Adjacency::Disabled
    }
    pub fn from_choice(index: u32) -> Option<Self> {
        match index {
            0 => Some(Adjacency::Disabled),
            1 => Some(Adjacency::Faces),
            2 => Some(Adjacency::Edges),
            3 => Some(Adjacency::Corners),
            _ => None,
        }
    }
}

/// The mod's settings, with upstream's defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    pub enabled: bool,
    /// Percent, 0..=100 (`minBridgeDistance`).
    pub min_bridge_distance: f64,
    pub only_bridge_when_crouched: bool,
    pub supported_axes: AxisMode,
    pub supported_axes_when_crouched: AxisOverride,
    pub delay_post_bridging: i32,
    pub show_crosshair: bool,
    pub show_outline: bool,
    pub show_outline_even_when_not_bridging: bool,
    pub non_bridge_respects_crouch_rules: bool,
    /// ARGB, upstream's `Color(0, 0, 0, 0.4f)`.
    pub outline_colour: u32,
    pub skip_torch_bridging: bool,
    pub enable_slab_assist: bool,
    pub enable_non_solid_replace: bool,
    pub bridging_snap_strength: f64,
    pub bridging_adjacency: Adjacency,
    pub show_debug_highlight: bool,
    pub show_non_bridging_debug_highlight: bool,
    pub show_debug_trace: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            min_bridge_distance: 20.0,
            only_bridge_when_crouched: false,
            supported_axes: AxisMode::Both,
            supported_axes_when_crouched: AxisOverride::Fallback,
            delay_post_bridging: 4,
            show_crosshair: true,
            show_outline: true,
            show_outline_even_when_not_bridging: false,
            non_bridge_respects_crouch_rules: true,
            outline_colour: 0x66_00_00_00,
            skip_torch_bridging: true,
            enable_slab_assist: true,
            enable_non_solid_replace: true,
            bridging_snap_strength: 1.0,
            bridging_adjacency: Adjacency::Corners,
            show_debug_highlight: true,
            show_non_bridging_debug_highlight: false,
            show_debug_trace: false,
        }
    }
}

/// A direction whose side may be built off of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Assist {
    /// Where the block goes.
    pub pos: Pos,
    /// The checked side: the support block is at `pos.offset(side)`.
    pub side: Dir,
}

/// A synthetic block hit, as the game's use-on-block path wants it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub pos: Pos,
    /// The clicked face; also the direction the item is used in.
    pub face: Dir,
    pub point: [f64; 3],
    /// Whether the synthetic hit started inside the target block's shape.
    pub inside: bool,
}

/// What the search asks the world.
pub trait World {
    fn is_air(&self, pos: Pos) -> bool;
    fn is_replaceable(&self, pos: Pos) -> bool;
    fn is_liquid(&self, pos: Pos) -> bool;
    fn player_overlaps_block(&self, pos: Pos) -> bool;
    fn holding_placeable(&self) -> bool;
    fn crouching(&self) -> bool;
}

/// What the slab assist needs.
pub trait SlabWorld {
    /// The block at `pos` is a slab; `Some(true)` its top half.
    fn slab_half(&self, pos: Pos) -> Option<bool>;
    fn holding_slab(&self) -> bool;
    fn holding_trapdoor(&self) -> bool;
}

/// One frame's/tick's answer, Y fraction, and search trace.
#[derive(Default)]
struct State {
    target: Option<Assist>,
    /// `BridgingStateTracker.lastKnownYFrac`.
    y_frac: f64,
    /// The last path, for the F3 trace.
    trace: Vec<Pos>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// The last computed target, if any.
pub fn target() -> Option<Assist> {
    STATE.with_borrow(|s| s.target)
}

/// The last path (for the F3 trace).
pub fn trace() -> Vec<Pos> {
    STATE.with_borrow(|s| s.trace.clone())
}

/// The player's last known Y fraction.
pub fn y_frac() -> f64 {
    STATE.with_borrow(|s| s.y_frac)
}

pub fn set_y_frac(v: f64) {
    STATE.with_borrow_mut(|s| s.y_frac = v);
}

/// Forgets the target and trace (disabled, left the world, or placed a block).
pub fn reset() {
    STATE.with_borrow_mut(|s| {
        s.target = None;
        s.trace.clear();
    });
}

/// The target for this frame: `None` unless a bridging target exists.
pub fn refresh<W: World>(
    world: &W,
    config: &Config,
    camera_pos: [f64; 3],
    camera_look: [f64; 3],
    player_eye: [f64; 3],
    player_look: [f64; 3],
    reach: f64,
) -> Option<Assist> {
    let found = closest_assist_target(
        world,
        config,
        camera_pos,
        camera_look,
        player_eye,
        player_look,
        reach,
    );
    let trace = if found.is_some() {
        view_block_path(config, camera_pos, camera_look, player_eye, player_look, reach)
    } else {
        Vec::new()
    };
    STATE.with_borrow_mut(|s| {
        s.target = found;
        s.trace = trace;
    });
    found
}

/// `PathTraversalHandler.getClosestAssistTarget`.
pub fn closest_assist_target<W: World>(
    world: &W,
    config: &Config,
    camera_pos: [f64; 3],
    camera_look: [f64; 3],
    player_eye: [f64; 3],
    player_look: [f64; 3],
    reach: f64,
) -> Option<Assist> {
    if !config.enabled || !world.holding_placeable() {
        return None;
    }
    let path = view_block_path(config, camera_pos, camera_look, player_eye, player_look, reach);
    let valid_sides = valid_assist_sides(camera_look);
    for pos in path {
        if !bridging_placement_allowed_at(world, config, pos) {
            continue;
        }
        if world.player_overlaps_block(pos) {
            continue;
        }
        for side in &valid_sides {
            if can_side_be_built_off_of(world, config, pos, *side) {
                return Some(Assist { pos, side: *side });
            }
        }
    }
    None
}

/// `PathTraversalHandler.getViewBlockPath`.
pub fn view_block_path(
    config: &Config,
    camera_pos: [f64; 3],
    camera_look: [f64; 3],
    player_eye: [f64; 3],
    player_look: [f64; 3],
    reach: f64,
) -> Vec<Pos> {
    let player_view = [
        player_eye[0] + player_look[0] * reach,
        player_eye[1] + player_look[1] * reach,
        player_eye[2] + player_look[2] * reach,
    ];
    let distance = dist(player_view, camera_pos);
    let min_distance = config.min_bridge_distance / 100.0;
    let far_vec = [
        camera_look[0] * distance,
        camera_look[1] * distance,
        camera_look[2] * distance,
    ];
    let near_vec = [
        far_vec[0] * min_distance,
        far_vec[1] * min_distance,
        far_vec[2] * min_distance,
    ];
    let start = Pos::containing([
        camera_pos[0] + near_vec[0],
        camera_pos[1] + near_vec[1],
        camera_pos[2] + near_vec[2],
    ]);
    let end = Pos::containing([
        camera_pos[0] + far_vec[0],
        camera_pos[1] + far_vec[1],
        camera_pos[2] + far_vec[2],
    ]);
    bresenham_voxels(config, start, end)
}

/// `Path.calculateBresenhamVoxels`.
pub fn bresenham_voxels(config: &Config, start: Pos, end: Pos) -> Vec<Pos> {
    let mut points = vec![start];
    let dx = (end.x - start.x).abs();
    let dy = (end.y - start.y).abs();
    let dz = (end.z - start.z).abs();
    let (x_step, y_step, z_step) = (
        if end.x >= start.x { 1 } else { -1 },
        if end.y >= start.y { 1 } else { -1 },
        if end.z >= start.z { 1 } else { -1 },
    );
    let mut working = start;
    let adjacency = config.bridging_adjacency;
    let push = |points: &mut Vec<Pos>, working: Pos| {
        if adjacency.enabled() {
            points.extend(missed_points(
                points,
                working,
                start,
                end,
                config.bridging_snap_strength,
                adjacency,
            ));
        }
        points.push(working);
    };
    if dx >= dy && dx >= dz {
        let (mut p1, mut p2) = (2 * dy - dx, 2 * dz - dx);
        for _ in 0..dx {
            working.x += x_step;
            if p1 >= 0 {
                working.y += y_step;
                p1 -= 2 * dx;
            }
            if p2 >= 0 {
                working.z += z_step;
                p2 -= 2 * dx;
            }
            p1 += 2 * dy;
            p2 += 2 * dz;
            push(&mut points, working);
        }
    } else if dy >= dx && dy >= dz {
        let (mut p1, mut p2) = (2 * dx - dy, 2 * dz - dy);
        for _ in 0..dy {
            working.y += y_step;
            if p1 >= 0 {
                working.x += x_step;
                p1 -= 2 * dy;
            }
            if p2 >= 0 {
                working.z += z_step;
                p2 -= 2 * dy;
            }
            p1 += 2 * dx;
            p2 += 2 * dz;
            push(&mut points, working);
        }
    } else {
        let (mut p1, mut p2) = (2 * dy - dz, 2 * dx - dz);
        for _ in 0..dz {
            working.z += z_step;
            if p1 >= 0 {
                working.y += y_step;
                p1 -= 2 * dz;
            }
            if p2 >= 0 {
                working.x += x_step;
                p2 -= 2 * dz;
            }
            p1 += 2 * dy;
            p2 += 2 * dx;
            push(&mut points, working);
        }
    }
    points
}

/// `Path.calculateMissedPoints`: blocks the line touches between two diagonal
/// steps, found with a box/line separating-axis test.
fn missed_points(
    points: &[Pos],
    new_point: Pos,
    line_start: Pos,
    line_end: Pos,
    extent_mult: f64,
    adjacency: Adjacency,
) -> Vec<Pos> {
    if matches!(adjacency, Adjacency::Disabled | Adjacency::Faces) {
        return Vec::new();
    }
    let Some(&last_point) = points.last() else {
        return Vec::new();
    };
    let point_delta = Pos::new(
        new_point.x - last_point.x,
        new_point.y - last_point.y,
        new_point.z - last_point.z,
    );
    let diff = new_point.dist_manhattan(last_point);
    if !(0..=3).contains(&diff) || diff <= 1 {
        return Vec::new();
    }
    let mut review = Vec::new();
    if diff == 2 {
        for d in [
            [point_delta.x, 0, 0],
            [0, point_delta.y, 0],
            [0, 0, point_delta.z],
        ] {
            if d == [0, 0, 0] {
                continue;
            }
            review.push(Pos::new(
                last_point.x + d[0],
                last_point.y + d[1],
                last_point.z + d[2],
            ));
        }
    }
    if diff == 3 && adjacency == Adjacency::Corners {
        for d in [
            [point_delta.x, 0, 0],
            [0, point_delta.y, 0],
            [0, 0, point_delta.z],
            [point_delta.x, point_delta.y, 0],
            [point_delta.x, 0, point_delta.z],
            [0, point_delta.y, point_delta.z],
        ] {
            if d == [0, 0, 0] {
                continue;
            }
            review.push(Pos::new(
                last_point.x + d[0],
                last_point.y + d[1],
                last_point.z + d[2],
            ));
        }
    }
    let extent = [
        CUBE_EXTENT[0] * extent_mult,
        CUBE_EXTENT[1] * extent_mult,
        CUBE_EXTENT[2] * extent_mult,
    ];
    let start = [
        f64::from(line_start.x),
        f64::from(line_start.y),
        f64::from(line_start.z),
    ];
    let end = [
        f64::from(line_end.x),
        f64::from(line_end.y),
        f64::from(line_end.z),
    ];
    review
        .into_iter()
        .filter(|pos| {
            let space = [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)];
            let line_start_d = [start[0] - space[0], start[1] - space[1], start[2] - space[2]];
            let line_end_d = [end[0] - space[0], end[1] - space[1], end[2] - space[2]];
            let mid = [
                (line_start_d[0] + line_end_d[0]) * 0.5,
                (line_start_d[1] + line_end_d[1]) * 0.5,
                (line_start_d[2] + line_end_d[2]) * 0.5,
            ];
            let line = [
                line_start_d[0] - mid[0],
                line_start_d[1] - mid[1],
                line_start_d[2] - mid[2],
            ];
            let line_ext = [line[0].abs(), line[1].abs(), line[2].abs()];
            if mid[0].abs() > extent[0] + line_ext[0] {
                return false;
            }
            if mid[1].abs() > extent[1] + line_ext[1] {
                return false;
            }
            if mid[2].abs() > extent[2] + line_ext[2] {
                return false;
            }
            if (mid[1] * line[2] - mid[2] * line[1]).abs()
                > extent[1] * line_ext[2] + extent[2] * line_ext[1]
            {
                return false;
            }
            if (mid[0] * line[2] - mid[2] * line[0]).abs()
                > extent[0] * line_ext[2] + extent[2] * line_ext[0]
            {
                return false;
            }
            if (mid[0] * line[1] - mid[1] * line[0]).abs()
                > extent[0] * line_ext[1] + extent[1] * line_ext[0]
            {
                return false;
            }
            true
        })
        .collect()
}

/// `PathTraversalHandler.getValidAssistSides`.
pub fn valid_assist_sides(look: [f64; 3]) -> Vec<Dir> {
    let mut sides = Vec::new();
    for d in Dir::ALL {
        let n = d.unit();
        let v = [f64::from(n[0]), f64::from(n[1]), f64::from(n[2])];
        let similarity = look[0] * v[0] + look[1] * v[1] + look[2] * v[2];
        if similarity < DIRECTION_SIMILARITY_THRESHOLD {
            continue;
        }
        sides.push(d.opposite());
    }
    sides
}

/// `PathTraversalHandler.canSideBeBuiltOffOf`.
pub fn can_side_be_built_off_of<W: World>(
    world: &W,
    config: &Config,
    placement_target: Pos,
    check_side: Dir,
) -> bool {
    let mut mode = config.supported_axes;
    if world.crouching() {
        mode = config.supported_axes_when_crouched.resolve(mode);
    }
    if !mode.is_direction_enabled(check_side) {
        return false;
    }
    let support = placement_target.offset(check_side);
    if world.is_air(support) || world.is_liquid(support) {
        return false;
    }
    !world.is_replaceable(support)
}

/// `PathTraversalHandler.isBridgingPlacementAllowedAt`.
pub fn bridging_placement_allowed_at<W: World>(world: &W, config: &Config, pos: Pos) -> bool {
    if config.enable_non_solid_replace {
        world.is_replaceable(pos)
    } else {
        world.is_air(pos)
    }
}

/// `Bridge.getDefaultPlaceAssistTarget`: the synthetic hit for an assist.
pub fn place_assist_target<S: SlabWorld>(
    slabs: &S,
    config: &Config,
    assist: Assist,
    y_frac: f64,
) -> Hit {
    let dir = assist.side.opposite();
    let pos = assist.pos;
    if config.enable_slab_assist && (slabs.holding_slab() || slabs.holding_trapdoor()) {
        if dir.is_horizontal() {
            if let Some(hit) = horizontal_slab_assist(pos, y_frac) {
                return hit;
            }
        } else if let Some(hit) = vertical_slab_assist(slabs, dir, pos) {
            return hit;
        }
    }
    Hit {
        pos,
        face: dir,
        point: pos.center(),
        inside: true,
    }
}

/// `Bridge.handleHorizontalSlabAssist`.
fn horizontal_slab_assist(pos: Pos, y_frac: f64) -> Option<Hit> {
    let lower = y_frac > TRAPDOOR_HEIGHT - NEAR_ZERO && y_frac < SLAB_HEIGHT + NEAR_ZERO;
    let c = pos.bottom_center();
    let point = if lower {
        [c[0], c[1] + 0.1, c[2]]
    } else {
        [c[0], c[1] + 0.9, c[2]]
    };
    Some(Hit {
        pos,
        face: if lower { Dir::Up } else { Dir::Down },
        point,
        inside: false,
    })
}

/// `Bridge.handleVerticalSlabAssist`: combine two slabs into a double slab.
fn vertical_slab_assist<S: SlabWorld>(slabs: &S, dir: Dir, pos: Pos) -> Option<Hit> {
    if !slabs.holding_slab() {
        return None;
    }
    let support = pos.offset(dir.opposite());
    let top = slabs.slab_half(support)?;
    if top && dir != Dir::Down {
        return None;
    }
    if !top && dir != Dir::Up {
        return None;
    }
    // The game is told the *existing* slab was clicked, with the target
    // block's centre as the point, so the two slabs combine.
    Some(Hit {
        pos: support,
        face: dir,
        point: pos.center(),
        inside: false,
    })
}

/// The indicator sprite for an assist: `PlacementAlignment.from(direction)`.
pub fn placement_alignment(side: Dir) -> &'static str {
    match side {
        Dir::Up => "down",
        Dir::Down => "up",
        _ => "horizontal",
    }
}

/// `Vec3.distanceTo`.
pub fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// `Mth.frac(player.getY())`.
pub fn fraction(y: f64) -> f64 {
    y - y.floor()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    struct TestWorld {
        solid: HashSet<Pos>,
        crouching: bool,
        holding: bool,
    }
    impl TestWorld {
        fn new(solid: &[Pos]) -> Self {
            Self {
                solid: solid.iter().copied().collect(),
                crouching: false,
                holding: true,
            }
        }
    }
    impl World for TestWorld {
        fn is_air(&self, pos: Pos) -> bool {
            !self.solid.contains(&pos)
        }
        fn is_replaceable(&self, pos: Pos) -> bool {
            self.is_air(pos)
        }
        fn is_liquid(&self, _pos: Pos) -> bool {
            false
        }
        fn player_overlaps_block(&self, _pos: Pos) -> bool {
            false
        }
        fn holding_placeable(&self) -> bool {
            self.holding
        }
        fn crouching(&self) -> bool {
            self.crouching
        }
    }
    impl SlabWorld for TestWorld {
        fn slab_half(&self, _pos: Pos) -> Option<bool> {
            None
        }
        fn holding_slab(&self) -> bool {
            false
        }
        fn holding_trapdoor(&self) -> bool {
            false
        }
    }

    #[test]
    fn bresenham_straight_line_is_exact() {
        let path = bresenham_voxels(&Config::default(), Pos::new(0, 0, 0), Pos::new(0, 0, -8));
        assert_eq!(path.len(), 9);
        for (i, p) in path.iter().enumerate() {
            assert_eq!(*p, Pos::new(0, 0, -(i as i32)));
        }
    }

    #[test]
    fn valid_sides() {
        assert_eq!(valid_assist_sides([0.0, 0.0, -1.0]), vec![Dir::South]);
        assert_eq!(valid_assist_sides([0.0, -1.0, 0.0]), vec![Dir::Up]);
        assert_eq!(valid_assist_sides([1.0, 0.0, 0.0]), vec![Dir::West]);
    }

    #[test]
    fn assist_beside_a_wall() {
        let config = Config::default();
        let mut solid = Vec::new();
        for x in -2..=2 {
            for y in 60..=64 {
                for z in -2..=2 {
                    if z == -2 {
                        solid.push(Pos::new(x, y, z));
                    }
                }
                solid.push(Pos::new(x, 60, -1));
                solid.push(Pos::new(x, 60, 0));
            }
        }
        let world = TestWorld::new(&solid);
        let assist = closest_assist_target(
            &world,
            &config,
            [0.5, 62.5, 0.5],
            [0.0, 0.0, -1.0],
            [0.5, 62.5, 0.5],
            [0.0, 0.0, -1.0],
            4.5,
        )
        .expect("an assist beside the wall");
        assert!(valid_assist_sides([0.0, 0.0, -1.0]).contains(&assist.side));
        assert!(world.solid.contains(&assist.pos.offset(assist.side)));
    }

    #[test]
    fn default_key_toggles() {
        // A fresh install: `native_mod_key` answers the default, which must act.
        assert!(toggles_on(DEFAULT_KEY, DEFAULT_KEY, true));
        assert_eq!(DEFAULT_KEY, "key.keyboard.comma");
        // Another key does not, and a release does not.
        assert!(!toggles_on(DEFAULT_KEY, "key.keyboard.z", true));
        assert!(!toggles_on(DEFAULT_KEY, DEFAULT_KEY, false));
    }

    #[test]
    fn rebound_and_unbound_keys() {
        // The player bound their own key: it acts, the old default does not.
        assert!(toggles_on("key.keyboard.g", "key.keyboard.g", true));
        assert!(!toggles_on("key.keyboard.g", DEFAULT_KEY, true));
        // Cleared key, or the empty name: nothing acts, even for that name.
        assert!(!toggles_on(UNBOUND_KEY, UNBOUND_KEY, true));
        assert!(!toggles_on("", "", true));
    }

    #[test]
    fn choice_conversions() {
        assert_eq!(AxisMode::from_choice(0), Some(AxisMode::Both));
        assert_eq!(AxisMode::from_choice(1), Some(AxisMode::Horizontal));
        assert_eq!(AxisMode::from_choice(2), Some(AxisMode::Vertical));
        assert_eq!(AxisMode::from_choice(3), None);

        assert_eq!(AxisOverride::from_choice(0), Some(AxisOverride::Fallback));
        assert_eq!(AxisOverride::from_choice(1), Some(AxisOverride::Both));
        assert_eq!(
            AxisOverride::from_choice(2),
            Some(AxisOverride::Horizontal)
        );
        assert_eq!(AxisOverride::from_choice(3), Some(AxisOverride::Vertical));
        assert_eq!(AxisOverride::from_choice(4), None);

        assert_eq!(Adjacency::from_choice(0), Some(Adjacency::Disabled));
        assert_eq!(Adjacency::from_choice(1), Some(Adjacency::Faces));
        assert_eq!(Adjacency::from_choice(2), Some(Adjacency::Edges));
        assert_eq!(Adjacency::from_choice(3), Some(Adjacency::Corners));
        assert_eq!(Adjacency::from_choice(4), None);
    }

    #[test]
    fn place_target_uses_opposite() {
        struct NoSlabs;
        impl SlabWorld for NoSlabs {
            fn slab_half(&self, _pos: Pos) -> Option<bool> {
                None
            }
            fn holding_slab(&self) -> bool {
                false
            }
            fn holding_trapdoor(&self) -> bool {
                false
            }
        }
        let hit = place_assist_target(
            &NoSlabs,
            &Config::default(),
            Assist {
                pos: Pos::new(3, 64, -2),
                side: Dir::Up,
            },
            0.0,
        );
        assert_eq!(hit.pos, Pos::new(3, 64, -2));
        assert_eq!(hit.face, Dir::Down);
        assert_eq!(hit.point, [3.5, 64.5, -1.5]);
        assert!(hit.inside);
    }

    #[test]
    fn horizontal_slab_assist_sets_inside_false_and_half() {
        struct SlabMock;
        impl SlabWorld for SlabMock {
            fn slab_half(&self, _pos: Pos) -> Option<bool> {
                None
            }
            fn holding_slab(&self) -> bool {
                true
            }
            fn holding_trapdoor(&self) -> bool {
                false
            }
        }
        let assist = Assist {
            pos: Pos::new(1, 64, 2),
            side: Dir::North,
        };
        let lower = place_assist_target(&SlabMock, &Config::default(), assist, 0.5);
        assert_eq!(lower.pos, Pos::new(1, 64, 2));
        assert_eq!(lower.face, Dir::Up);
        assert_eq!(lower.point, [1.5, 64.1, 2.5]);
        assert!(!lower.inside);

        let upper = place_assist_target(&SlabMock, &Config::default(), assist, 0.0);
        assert_eq!(upper.pos, Pos::new(1, 64, 2));
        assert_eq!(upper.face, Dir::Down);
        assert_eq!(upper.point, [1.5, 64.9, 2.5]);
        assert!(!upper.inside);
    }

    #[test]
    fn vertical_slab_assist_combines_slabs_but_not_trapdoors() {
        struct SlabMock {
            top: Option<bool>,
            slab: bool,
            trapdoor: bool,
        }
        impl SlabWorld for SlabMock {
            fn slab_half(&self, _pos: Pos) -> Option<bool> {
                self.top
            }
            fn holding_slab(&self) -> bool {
                self.slab
            }
            fn holding_trapdoor(&self) -> bool {
                self.trapdoor
            }
        }

        let bottom_slab_support = SlabMock {
            top: Some(false),
            slab: true,
            trapdoor: false,
        };
        let hit_up = place_assist_target(
            &bottom_slab_support,
            &Config::default(),
            Assist {
                pos: Pos::new(0, 65, 0),
                side: Dir::Down,
            },
            0.0,
        );
        assert_eq!(hit_up.pos, Pos::new(0, 64, 0));
        assert_eq!(hit_up.face, Dir::Up);
        assert_eq!(hit_up.point, [0.5, 65.5, 0.5]);
        assert!(!hit_up.inside);

        let top_slab_support = SlabMock {
            top: Some(true),
            slab: true,
            trapdoor: false,
        };
        let hit_down = place_assist_target(
            &top_slab_support,
            &Config::default(),
            Assist {
                pos: Pos::new(0, 63, 0),
                side: Dir::Up,
            },
            0.0,
        );
        assert_eq!(hit_down.pos, Pos::new(0, 64, 0));
        assert_eq!(hit_down.face, Dir::Down);
        assert_eq!(hit_down.point, [0.5, 63.5, 0.5]);
        assert!(!hit_down.inside);

        let trapdoor_off_slab = SlabMock {
            top: Some(false),
            slab: false,
            trapdoor: true,
        };
        let hit_trapdoor = place_assist_target(
            &trapdoor_off_slab,
            &Config::default(),
            Assist {
                pos: Pos::new(0, 65, 0),
                side: Dir::Down,
            },
            0.0,
        );
        assert_eq!(hit_trapdoor.pos, Pos::new(0, 65, 0));
        assert_eq!(hit_trapdoor.face, Dir::Up);
        assert!(hit_trapdoor.inside);
    }

    #[test]
    fn adjacency_controls_missed_points() {
        let mut cfg = Config::default();

        cfg.bridging_adjacency = Adjacency::Disabled;
        assert_eq!(
            bresenham_voxels(&cfg, Pos::new(0, 0, 0), Pos::new(1, 1, 0)).len(),
            2
        );

        cfg.bridging_adjacency = Adjacency::Faces;
        assert_eq!(
            bresenham_voxels(&cfg, Pos::new(0, 0, 0), Pos::new(1, 1, 0)).len(),
            2
        );

        cfg.bridging_adjacency = Adjacency::Edges;
        assert_eq!(
            bresenham_voxels(&cfg, Pos::new(0, 0, 0), Pos::new(1, 1, 0)).len(),
            4
        );
        assert_eq!(
            bresenham_voxels(&cfg, Pos::new(0, 0, 0), Pos::new(1, 1, 1)).len(),
            2
        );

        cfg.bridging_adjacency = Adjacency::Corners;
        assert_eq!(
            bresenham_voxels(&cfg, Pos::new(0, 0, 0), Pos::new(1, 1, 1)).len(),
            8
        );
    }

    #[test]
    fn fraction_of_y() {
        assert!((fraction(64.25) - 0.25).abs() < 1e-9);
    }
}