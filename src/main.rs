use rust_media::{
    decode::Video,
    player::{MediaReader, Player, Status},
    providers,
};
use slint::ComponentHandle;
use std::{
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
slint::include_modules!();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--probe") {
        let input = args
            .get(1)
            .ok_or("Usage: rust-media --probe <video URL or file>")?;
        let start = Instant::now();
        let resolved = providers::resolve(input)?;
        let source = MediaReader::open(&resolved.video, Arc::default())?;
        let size = source.size();
        let mut decoder = Video::new(source, size)?;
        let mut count = 0;
        let mut first = None;
        let mut last = 0.;
        let mut checksum = 0u64;
        while count < 60 {
            let Some((pts, frame)) = decoder.frame()? else {
                break;
            };
            if pts < last {
                return Err("Frame timestamps are out of order".into());
            }
            last = pts;
            first.get_or_insert((frame.width, frame.height));
            checksum = frame.rgba.iter().fold(checksum, |hash, value| {
                hash.wrapping_mul(31).wrapping_add(u64::from(*value))
            });
            count += 1;
        }
        if count == 0 {
            return Err("Video contains no decodable frames".into());
        }
        println!(
            "provider={} frames={} dimensions={:?} duration={:.3}s last_pts={:.3}s decode_wall={:.3}s checksum={checksum:016x}",
            resolved.provider,
            count,
            first.unwrap(),
            decoder.duration,
            last,
            start.elapsed().as_secs_f64()
        );
        return Ok(());
    }
    let ui = PlayerWindow::new()?;
    let player = Rc::new(Player::new());
    let weak = ui.as_weak();
    let controller = player.clone();
    ui.on_load(move |input| {
        if input.trim().is_empty() {
            return;
        }
        controller.load(input.trim().into());
        if let Some(ui) = weak.upgrade() {
            ui.set_frame(Default::default());
            ui.set_error("".into());
            ui.set_status(1);
        }
    });
    let weak = ui.as_weak();
    let controller = player.clone();
    ui.on_toggle(move || {
        if let Some(ui) = weak.upgrade() {
            match ui.get_status() {
                0 | 5 => ui.invoke_load(ui.get_address()),
                4 => {
                    controller.seek(0.);
                    controller.pause(false);
                }
                _ => controller.pause(ui.get_status() == 2),
            }
        }
    });
    let controller = player.clone();
    ui.on_seek(move |seconds| controller.seek(f64::from(seconds)));
    let controller = player.clone();
    ui.on_set_volume(move |volume| controller.set_volume(volume));
    let weak = ui.as_weak();
    ui.on_expand(move || {
        if let Some(ui) = weak.upgrade() {
            let expanded = !ui.get_expanded();
            ui.set_expanded(expanded);
            ui.window().set_fullscreen(expanded);
        }
    });
    let timer = slint::Timer::default();
    let weak = ui.as_weak();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(16),
        move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let state = player.snapshot();
            ui.set_media_title(state.title.into());
            ui.set_error(state.error.into());
            ui.set_position(state.position as f32);
            ui.set_duration(state.duration as f32);
            ui.set_status(match state.status {
                Status::Idle => 0,
                Status::Loading => 1,
                Status::Playing => 2,
                Status::Paused => 3,
                Status::Ended => 4,
                Status::Failed => 5,
            });
            if let Some(frame) = state.pixels {
                ui.set_frame(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
                    slint::Rgba8Pixel,
                >::clone_from_slice(
                    &frame.rgba,
                    frame.width,
                    frame.height,
                )));
            }
        },
    );
    if let Some(input) = args.first() {
        ui.set_address(input.into());
        ui.invoke_load(input.into());
    }
    ui.run()?;
    Ok(())
}
