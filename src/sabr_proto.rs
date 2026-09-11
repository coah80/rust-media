use prost::Message;

#[derive(Clone, PartialEq, Message)]
pub struct FormatId {
    #[prost(uint32, optional, tag = "1")]
    pub itag: Option<u32>,
    #[prost(uint64, optional, tag = "2")]
    pub last_modified: Option<u64>,
    #[prost(string, optional, tag = "3")]
    pub xtags: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AbrState {
    #[prost(int64, optional, tag = "23")]
    pub bandwidth: Option<i64>,
    #[prost(int64, optional, tag = "28")]
    pub time: Option<i64>,
    #[prost(int32, optional, tag = "34")]
    pub visibility: Option<i32>,
    #[prost(float, optional, tag = "35")]
    pub rate: Option<f32>,
    #[prost(int32, optional, tag = "40")]
    pub tracks: Option<i32>,
    #[prost(int64, optional, tag = "44")]
    pub state: Option<i64>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ClientInfo {
    #[prost(int32, optional, tag = "16")]
    pub name: Option<i32>,
    #[prost(string, optional, tag = "17")]
    pub version: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ContextValue {
    #[prost(int32, optional, tag = "1")]
    pub kind: Option<i32>,
    #[prost(bytes, optional, tag = "2")]
    pub value: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Streamer {
    #[prost(message, optional, tag = "1")]
    pub client: Option<ClientInfo>,
    #[prost(bytes, optional, tag = "3")]
    pub cookie: Option<Vec<u8>>,
    #[prost(message, repeated, tag = "5")]
    pub contexts: Vec<ContextValue>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Range {
    #[prost(message, optional, tag = "1")]
    pub format: Option<FormatId>,
    #[prost(int64, optional, tag = "2")]
    pub start: Option<i64>,
    #[prost(int64, optional, tag = "3")]
    pub duration: Option<i64>,
    #[prost(int32, optional, tag = "4")]
    pub first: Option<i32>,
    #[prost(int32, optional, tag = "5")]
    pub last: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Request {
    #[prost(message, optional, tag = "1")]
    pub abr: Option<AbrState>,
    #[prost(message, repeated, tag = "2")]
    pub selected: Vec<FormatId>,
    #[prost(message, repeated, tag = "3")]
    pub ranges: Vec<Range>,
    #[prost(bytes, optional, tag = "5")]
    pub config: Option<Vec<u8>>,
    #[prost(message, repeated, tag = "16")]
    pub audio: Vec<FormatId>,
    #[prost(message, repeated, tag = "17")]
    pub video: Vec<FormatId>,
    #[prost(message, optional, tag = "19")]
    pub streamer: Option<Streamer>,
}

#[derive(Clone, PartialEq, Message)]
pub struct TimeRange {
    #[prost(int64, optional, tag = "1")]
    pub start_ticks: Option<i64>,
    #[prost(int64, optional, tag = "2")]
    pub duration_ticks: Option<i64>,
    #[prost(int32, optional, tag = "3")]
    pub timescale: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Header {
    #[prost(uint32, optional, tag = "1")]
    pub id: Option<u32>,
    #[prost(uint32, optional, tag = "3")]
    pub itag: Option<u32>,
    #[prost(int32, optional, tag = "7")]
    pub compression: Option<i32>,
    #[prost(bool, optional, tag = "8")]
    pub init: Option<bool>,
    #[prost(int32, optional, tag = "9")]
    pub sequence: Option<i32>,
    #[prost(int64, optional, tag = "11")]
    pub start: Option<i64>,
    #[prost(int64, optional, tag = "12")]
    pub duration: Option<i64>,
    #[prost(int64, optional, tag = "14")]
    pub length: Option<i64>,
    #[prost(message, optional, tag = "15")]
    pub time_range: Option<TimeRange>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Init {
    #[prost(message, optional, tag = "2")]
    pub format: Option<FormatId>,
    #[prost(int64, optional, tag = "3")]
    pub end_time: Option<i64>,
    #[prost(int64, optional, tag = "4")]
    pub end_segment: Option<i64>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Policy {
    #[prost(int32, optional, tag = "4")]
    pub backoff: Option<i32>,
    #[prost(bytes, optional, tag = "7")]
    pub cookie: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Redirect {
    #[prost(string, optional, tag = "1")]
    pub url: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ContextUpdate {
    #[prost(int32, optional, tag = "1")]
    pub kind: Option<i32>,
    #[prost(bytes, optional, tag = "3")]
    pub value: Option<Vec<u8>>,
    #[prost(bool, optional, tag = "4")]
    pub send: Option<bool>,
    #[prost(int32, optional, tag = "5")]
    pub write_policy: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ContextPolicy {
    #[prost(int32, repeated, tag = "1")]
    pub start: Vec<i32>,
    #[prost(int32, repeated, tag = "2")]
    pub stop: Vec<i32>,
    #[prost(int32, repeated, tag = "3")]
    pub discard: Vec<i32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Protection {
    #[prost(int32, optional, tag = "1")]
    pub status: Option<i32>,
}
