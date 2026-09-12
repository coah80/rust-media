use rust_media::{Player, Status};
use std::{thread, time::Duration};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = std::env::args().nth(1).ok_or("Usage: play <file or URL>")?;
    let player = Player::new();
    player.load(input);
    loop {
        let snapshot = player.snapshot();
        match snapshot.status {
            Status::Failed => return Err(snapshot.error.into()),
            Status::Ended => return Ok(()),
            _ => thread::sleep(Duration::from_millis(16)),
        }
    }
}
