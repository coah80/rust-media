pub mod decode;
pub mod http;
pub mod player;
pub mod providers;

#[derive(Debug)]
pub struct Pixels {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}
