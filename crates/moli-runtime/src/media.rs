//! Images from devices (camera snapshots), documents to devices (printers).
//!
//! A driver that can produce an image for one of its devices registers a
//! [`SnapshotSource`] with [`crate::DriverCtx::provide_snapshots`]; the hub
//! calls it directly when a surface asks. No queue: an image nobody asked
//! for is never fetched. A printer registers a [`PrintSink`] the same way
//! ([`crate::DriverCtx::provide_printing`]).

use crate::BoxFuture;

/// One image, as the device produced it.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("content_type", &self.content_type)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// Produces the cover of what a device plays (a picture to show beside
/// the title, not a camera: nothing here makes the device a camera).
pub trait CoverSource: std::fmt::Debug + Send + Sync + 'static {
    fn cover(&self) -> BoxFuture<'_, anyhow::Result<Image>>;
}

/// Produces a fresh image on demand.
pub trait SnapshotSource: std::fmt::Debug + Send + Sync + 'static {
    fn snapshot(&self) -> BoxFuture<'_, anyhow::Result<Image>>;
}

/// A document to print (a page, a photo), in a format the printer takes.
#[derive(Clone, PartialEq, Eq)]
pub struct Document {
    /// What the person printed (a file name): shown while it prints.
    pub name: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub copies: u32,
    /// Colour, or black only.
    pub color: bool,
    /// Both sides of the sheet.
    pub two_sided: bool,
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("name", &self.name)
            .field("content_type", &self.content_type)
            .field("bytes", &self.bytes.len())
            .field("copies", &self.copies)
            .field("color", &self.color)
            .field("two_sided", &self.two_sided)
            .finish()
    }
}

/// Takes documents to print. Answers once the document is queued (a page
/// takes a minute to print; nobody waits on it): its progress shows in the
/// printer's points.
pub trait PrintSink: std::fmt::Debug + Send + Sync + 'static {
    fn print(&self, document: Document) -> BoxFuture<'_, anyhow::Result<()>>;
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PrintError {
    #[error("unknown device")]
    UnknownDevice,
    #[error("this device does not print")]
    NotAPrinter,
    #[error("{0}")]
    Refused(String),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SnapshotError {
    #[error("unknown device")]
    UnknownDevice,
    #[error("this device has no image")]
    NotACamera,
    #[error("no image in time")]
    Timeout,
    #[error("image unavailable: {0}")]
    Failed(String),
}

// ---- live video ------------------------------------------------------------

/// What a viewer wants of a live stream (a hint: the source picks the
/// closest quality it has).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveWant {
    pub width: u32,
    pub height: u32,
    /// Sound too, when the camera has some.
    pub audio: bool,
}

/// One video frame, as the camera encoded it (H.264, no start codes).
#[derive(Clone, PartialEq, Eq)]
pub struct VideoFrame {
    /// RTP clock of the stream, 90 kHz.
    pub timestamp: u32,
    /// The NAL units of the frame, in order.
    pub nals: Vec<Vec<u8>>,
    /// Starts with an IDR picture: a viewer can begin here.
    pub keyframe: bool,
}

impl std::fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoFrame")
            .field("timestamp", &self.timestamp)
            .field("nals", &self.nals.len())
            .field("keyframe", &self.keyframe)
            .finish()
    }
}

/// A little sound, decoded: 16-bit mono samples at the camera's rate.
#[derive(Clone, PartialEq, Eq)]
pub struct AudioChunk {
    pub rate: u32,
    pub samples: Vec<i16>,
}

impl std::fmt::Debug for AudioChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioChunk")
            .field("rate", &self.rate)
            .field("samples", &self.samples.len())
            .finish()
    }
}

/// What a live stream brings, in the order it comes.
#[derive(Debug)]
pub enum LiveItem {
    Video(VideoFrame),
    Audio(AudioChunk),
}

/// What an opened stream says of itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveInfo {
    pub width: u32,
    pub height: u32,
    /// Parameter sets known at the start (the SDP's), NAL units.
    pub sps: Vec<u8>,
    pub pps: Vec<u8>,
    /// The sample rate of the sound it brings, if it does.
    pub audio_rate: Option<u32>,
}

/// A live H.264 stream being read. Dropping it closes it.
pub trait LiveStream: Send {
    fn info(&self) -> &LiveInfo;
    /// The next frame or bit of sound; an error ends the stream.
    fn next(&mut self) -> BoxFuture<'_, anyhow::Result<LiveItem>>;
}

/// Opens live streams of a camera on demand. Nothing runs while nobody
/// watches.
pub trait LiveSource: std::fmt::Debug + Send + Sync + 'static {
    fn open(&self, want: LiveWant) -> BoxFuture<'_, anyhow::Result<Box<dyn LiveStream>>>;
}

#[derive(Debug, thiserror::Error)]
pub enum LiveError {
    #[error("unknown device")]
    UnknownDevice,
    #[error("this device has no live video")]
    NoLive,
    #[error("live video unavailable: {0}")]
    Failed(String),
}

// ---- talking back ------------------------------------------------------------

/// Where a person's voice goes: the open session with a camera's speaker.
/// Dropping it ends the session.
pub trait TalkSink: Send {
    /// The sample rate of the 16-bit mono samples it takes.
    fn rate(&self) -> u32;
    /// Some voice, at [`TalkSink::rate`].
    fn send(&mut self, samples: &[i16]) -> BoxFuture<'_, anyhow::Result<()>>;
}

/// Opens a session with a camera's speaker on demand. Nothing is opened (and
/// the speaker stays quiet) until somebody speaks.
pub trait TalkSource: std::fmt::Debug + Send + Sync + 'static {
    fn open(&self) -> BoxFuture<'_, anyhow::Result<Box<dyn TalkSink>>>;
}

#[derive(Debug, thiserror::Error)]
pub enum TalkError {
    #[error("unknown device")]
    UnknownDevice,
    #[error("this device has no speaker to talk to")]
    NoSpeaker,
    #[error("talking back unavailable: {0}")]
    Failed(String),
}
