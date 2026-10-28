//! Constants of the binary `.rofl` format.
//!
//! General structure of the container (stable for years):
//!
//! ```text
//! offset 0x00 : [u8; 4]  magic b"RIOT"
//! offset 0x04 : u8       header_length
//! offset 0x05 : u64 LE   footer_offset (absolute offset of the footer in the file)
//! offset 0x0D : u32 LE   length of the metadata JSON string
//! offset 0x11 : [u8; N]  metadata (UTF-8 JSON)
//! offset ..   : u32 LE   number of chunks
//! offset ..   : chunks   (12-byte header + data)
//! offset ..   : footer   (end-of-game JSON)
//! ```
//!
//! Each chunk:
//!
//! ```text
//! [u32 LE] compressed_len
//! [u32 LE] uncompressed_len
//! [u8]     compression method (0 = raw, 1 = zlib)
//! [u8; 3]  unknown / padding
//! [u8; compressed_len] data
//! ```
//!
//! WARNING: the *payload* section (type 3 frames and type 4 event blocks) is
//! reverse engineering based on community tools. It may vary between patches;
//! all the constants live here so they can be adjusted in a single place.

/// File magic: `RIOT`.
pub const MAGIC: &[u8; 4] = b"RIOT";

/// Size of each chunk header (12 bytes).
pub const CHUNK_HEADER_SIZE: usize = 12;

/// Uncompressed chunk.
pub const COMPRESSION_NONE: u8 = 0;
/// Chunk compressed with zlib (RFC 1950) — legacy format only.
pub const COMPRESSION_ZLIB: u8 = 1;

// ---------------------------------------------------------------------------
// Modern format (zstd + footer). See `crate::modern` for the complete layout.
// ---------------------------------------------------------------------------

/// Magic of a Zstandard frame (modern format).
pub const ZSTD_MAGIC: &[u8; 4] = &[0x28, 0xB5, 0x2F, 0xFD];

/// Limit of payload sections (each real replay has ~10^6).
pub const MAX_SECTIONS: u64 = 50_000_000;

/// Fixed size of the 8 "unknown" bytes that surround the footer JSON.
pub const FOOTER_KNOWN_SIZE: usize = 8;

// ---------------------------------------------------------------------------
// Payload elements (decompressed data chunk)
// ---------------------------------------------------------------------------
// Each element: [u8 type] [u32 LE timestamp_ms] [body]

/// Keyframe marker (keyframe number: u32).
pub const PAYLOAD_KEYFRAME: u8 = 1;
/// Gameflow state change (state: u32).
pub const PAYLOAD_GAMEFLOW: u8 = 2;
/// Telemetry frame (10 participants with position/health).
pub const PAYLOAD_FRAME: u8 = 3;
/// Event block (deaths, wards, purchases, skills...).
pub const PAYLOAD_EVENT_BLOCK: u8 = 4;

/// Number of players (always 10 in normal/classic matches).
pub const MAX_PLAYERS: usize = 10;

/// Header of a frame (2 unknown bytes).
pub const FRAME_HEADER_SIZE: usize = 2;

/// Unknown bytes before the coordinates inside each participant.
pub const FRAME_PARTICIPANT_SKIP_BEFORE: usize = 5;
/// Unknown bytes between the position and the health/mana.
pub const FRAME_PARTICIPANT_SKIP_MID: usize = 33;
/// Total FIXED size of a participant's block inside a frame.
///
/// By treating it as a fixed-size block, even if the internal layout changes
/// in a future patch we stay aligned on the next participant.
pub const FRAME_PARTICIPANT_BLOCK: usize = 108;

// ---------------------------------------------------------------------------
// Event IDs (reversed from community tools / LeagueToolkit)
// Only the vision ones are interpreted today; the rest stays documented here.
// ---------------------------------------------------------------------------
#[allow(dead_code)]
pub const EVENT_ID_SKILL_LEVEL_UP: u16 = 5001;
#[allow(dead_code)]
pub const EVENT_ID_ITEM_PURCHASE: u16 = 5002;
#[allow(dead_code)]
pub const EVENT_ID_ITEM_SELL: u16 = 5003;
#[allow(dead_code)]
pub const EVENT_ID_ITEM_UNDO: u16 = 5004;
#[allow(dead_code)]
pub const EVENT_ID_SKILL_SLOT: u16 = 5005;
pub const EVENT_ID_WARD_PLACED: u16 = 5006;
pub const EVENT_ID_WARD_KILL: u16 = 5007;
#[allow(dead_code)]
pub const EVENT_ID_LEVEL_UP: u16 = 5008;
#[allow(dead_code)]
pub const EVENT_ID_PARTICIPANT_DEATH: u16 = 5009;

// ---------------------------------------------------------------------------
// Defensive limits (prevent giant allocations on corrupt files)
// ---------------------------------------------------------------------------

/// Limit of the metadata JSON string.
pub const MAX_METADATA_LEN: u64 = 16 * 1024 * 1024; // 16 MiB
/// Limit of a decompressed chunk.
pub const MAX_CHUNK_UNCOMPRESSED: usize = 64 * 1024 * 1024; // 64 MiB
/// Limit of the full decompressed payload (~12 min of matches at ~30 fps).
pub const MAX_PAYLOAD_LEN: usize = 128 * 1024 * 1024; // 128 MiB
/// Limit of chunks (protects against a corrupt count).
pub const MAX_CHUNKS: u32 = 20_000;
/// Limit of a ward type string.
pub const MAX_WARD_TYPE_LEN: usize = 64;
