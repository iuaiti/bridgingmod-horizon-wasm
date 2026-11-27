// The bridged click.
//
// The 1.0.1 `.hnmod` swapped the tick's crosshair for one use and let the
// game's own use-on-block path place the block. A mod cannot reach that path
// from the SDK, so this builds the same vanilla `use_item_on` packet the
// game would have sent for such a click and submits it through
// `ClientHost::send_packet`.
//
// What the server then does is exactly what it does for a player's click on
// that block face: the same reach, survival, `can_place_on`, item and
// protection checks. A refused click is refused by the server, so nothing is
// rolled back here.

use mod_api::{ClientHost, Hand, Identifier, PlayerRef, ServerboundPlayPacket};

use crate::runtime::{Dir, Hit};

/// The vanilla `use_item_on` packet id, as the protocol spells it.
const USE_ITEM_ON: &str = "minecraft:use_item_on";

/// A packet body writer for the fields vanilla reads, in its order.
struct Body {
    bytes: Vec<u8>,
}

impl Body {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    /// A VarInt: seven bits a byte, low group first, the top bit continuing.
    fn varint(&mut self, value: i32) {
        let mut v = value as u32;
        loop {
            let byte = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                self.bytes.push(byte);
                break;
            }
            self.bytes.push(byte | 0x80);
        }
    }

    fn bool(&mut self, value: bool) {
        self.bytes.push(u8::from(value));
    }

    fn f32(&mut self, value: f32) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    /// `BlockPos.asLong`: x and z in 26 bits each, y in 12, packed in a long.
    fn pos(&mut self, pos: [i32; 3]) {
        let [x, y, z] = pos;
        let packed = ((x as i64 & 0x3FF_FFFF) << 38)
            | ((z as i64 & 0x3FF_FFFF) << 12)
            | (y as i64 & 0xFFF);
        self.bytes.extend_from_slice(&packed.to_be_bytes());
    }
}

/// The face's number, as the protocol writes it: down, up, north, south, west, east.
fn face_id(face: Dir) -> i32 {
    match face {
        Dir::Down => 0,
        Dir::Up => 1,
        Dir::North => 2,
        Dir::South => 3,
        Dir::West => 4,
        Dir::East => 5,
    }
}

/// Encode a `minecraft:use_item_on` play packet body in network byte order.
fn encode_use_item_on_body(hand: Hand, hit: &Hit) -> Vec<u8> {
    let mut body = Body::new();
    // `hand`: 0 main, 1 off.
    body.varint(if matches!(hand, Hand::Main) { 0 } else { 1 });
    body.pos([hit.pos.x, hit.pos.y, hit.pos.z]);
    body.varint(face_id(hit.face));
    // The cursor, relative to the clicked block.
    let cursor = [
        (hit.point[0] - f64::from(hit.pos.x)) as f32,
        (hit.point[1] - f64::from(hit.pos.y)) as f32,
        (hit.point[2] - f64::from(hit.pos.z)) as f32,
    ];
    for c in cursor {
        body.f32(c);
    }
    // `inside`: whether the ray started inside the clicked block's shape.
    body.bool(hit.inside);
    // `world_border`: not at the border.
    body.bool(false);
    // `sequence`: the prediction sequence the server acknowledges with.
    body.varint(0);
    body.bytes
}

/// Send one bridged click at `hit`, using `hand`.
///
/// The click is checked with the game's own `may_use_item_at` first, the read
/// the 1.0.1 bundle used to refuse a click it knew the server would reject.
pub fn place(host: &mut dyn ClientHost, player: PlayerRef, hand: Hand, hit: &Hit) -> bool {
    let Some(stack) = host
        .living(player.entity())
        .and_then(|living| living.hand_item(host, hand))
    else {
        return false;
    };
    // The game's own placement rules for that position and face.
    if player.may_use_item_at(
        host,
        [hit.pos.x, hit.pos.y, hit.pos.z],
        face(hit.face),
        &stack,
    ) == Some(false)
    {
        return false;
    }

    let Ok(id) = Identifier::parse(USE_ITEM_ON) else {
        return false;
    };
    let bytes = encode_use_item_on_body(hand, hit);
    host.send_packet(ServerboundPlayPacket::new(id, bytes))
        .is_ok()
}

/// The `BlockFace` the protocol's number names, for `may_use_item_at`.
fn face(face: Dir) -> mod_api::BlockFace {
    match face {
        Dir::Down => mod_api::BlockFace::Down,
        Dir::Up => mod_api::BlockFace::Up,
        Dir::North => mod_api::BlockFace::North,
        Dir::South => mod_api::BlockFace::South,
        Dir::West => mod_api::BlockFace::West,
        Dir::East => mod_api::BlockFace::East,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::Pos;

    #[test]
    fn varint_encodes_single_and_multi_byte() {
        let encode = |v| {
            let mut b = Body::new();
            b.varint(v);
            b.bytes
        };
        assert_eq!(encode(0), [0x00]);
        assert_eq!(encode(1), [0x01]);
        assert_eq!(encode(127), [0x7F]);
        assert_eq!(encode(128), [0x80, 0x01]);
        assert_eq!(encode(300), [0xAC, 0x02]);
    }

    #[test]
    fn pos_packs_negative_coordinates() {
        let mut b = Body::new();
        b.pos([-1, -64, -1]);
        assert_eq!(b.bytes, 0xFFFF_FFFF_FFFF_FFC0u64.to_be_bytes());
    }

    #[test]
    fn f32_uses_big_endian_wire_order() {
        let hit = Hit {
            pos: Pos::new(3, 64, -2),
            face: Dir::Up,
            point: [3.5, 64.1, -1.1],
            inside: false,
        };
        let bytes = encode_use_item_on_body(Hand::Off, &hit);
        assert_eq!(bytes.len(), 25);
        assert_eq!(bytes[0], 1); // off-hand
        let mut expected_pos = Body::new();
        expected_pos.pos([3, 64, -2]);
        assert_eq!(&bytes[1..9], expected_pos.bytes.as_slice());
        assert_eq!(bytes[9], 1); // Dir::Up
        assert_eq!(&bytes[10..14], ((3.5 - 3.0) as f32).to_be_bytes());
        assert_eq!(&bytes[14..18], ((64.1 - 64.0) as f32).to_be_bytes());
        assert_eq!(&bytes[18..22], ((-1.1 - (-2.0)) as f32).to_be_bytes());
        assert_eq!(bytes[22], 0); // inside
        assert_eq!(bytes[23], 0); // world_border
        assert_eq!(bytes[24], 0); // sequence
    }
}
