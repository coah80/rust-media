//! Native H.264/AAC playback with bounded progressive MP4 reads.
//!
//! [`Player`] owns a playback worker and exposes snapshots for a host event loop.
//! [`Player::snapshot`] consumes the latest available pixel buffer; retain the
//! previous image when a snapshot has no new pixels. Position and duration are
//! measured in seconds, and buffered progress is a fraction from zero to one.
//! Dropping the player requests cancellation; it does not synchronously join
//! ongoing network work. Direct range requests may finish at their timeout.
//!
//! The default build has no windowing dependency. Enable `native` to build the
//! optional Slint player. The API is experimental and may change before 1.0.
//!
//! ```no_run
//! use rust_media::{Player, Status};
//!
//! let player = Player::new();
//! player.load("clip.mp4".into());
//! let snapshot = player.snapshot();
//! if snapshot.status == Status::Failed {
//!     eprintln!("{}", snapshot.error);
//! }
//! player.stop();
//! ```

pub mod audio;
pub mod decode;
pub mod fragment;
pub mod http;
pub mod player;
pub mod providers;
mod request;
mod sabr_proto;
mod script;
mod youtube;

pub use player::{Player, Snapshot, Status};

#[derive(Debug)]
pub struct Pixels {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}
