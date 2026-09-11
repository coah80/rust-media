mod page;

use anyrender_vello_cpu::VelloCpuWindowRenderer;
use blitz_shell::{BlitzApplication, BlitzShellProxy, WindowConfig, create_default_event_loop};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1);
    let html = match path {
        Some(path) => std::fs::read_to_string(path)?,
        None => include_str!("../fixture.html").into(),
    };
    let page = page::Page::new(&html)?;
    let event_loop = create_default_event_loop();
    let (proxy, receiver) = BlitzShellProxy::new(event_loop.create_proxy());
    let mut app = BlitzApplication::new(proxy, receiver);
    app.add_window(WindowConfig::new(
        Box::new(page),
        VelloCpuWindowRenderer::new(),
    ));
    println!("Rust DOM/CSS/JavaScript prototype. Video and YouTube are not implemented.");
    event_loop.run_app(app)?;
    Ok(())
}
