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

#[derive(Debug)]
pub struct Pixels {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}
