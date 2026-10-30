use std::sync::{Arc, Mutex};

use crate::pattern;

/// What the camera asks a [`FrameSource`] for: the region and format its registers hold when a
/// frame is due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRequest<'a> {
    pub width: u32,
    pub height: u32,
    pub offset_x: u32,
    pub offset_y: u32,
    /// The `PixelFormat` register value (this fake's own values, see
    /// [`crate::pixel_format`], not PFNC codes).
    pub pixel_format: u32,
    /// The symbolic name of [`Self::pixel_format`] (`Mono8`, `Coord3D_C16`, …), if it has one.
    pub pixel_format_name: Option<&'a str>,
    /// The frame id the frame goes out with.
    pub frame_id: u64,
    /// The leader's timestamp, in nanoseconds since the Unix epoch.
    pub timestamp_ns: u64,
}

/// Produces the image payload of each streamed frame. The camera calls it on its GVSP thread
/// once per frame period while acquisition is active.
///
/// The returned bytes are the image as the pixel format lays it out (`width * height` bytes
/// for Mono8, little-endian `u16`s for the 16-bit formats), so they should be `PayloadSize`
/// long. In chunk mode the camera wraps them in chunks itself. `None` skips the frame: nothing
/// is sent and the frame id is not used up.
pub trait FrameSource: Send {
    fn next_frame(&mut self, request: &FrameRequest<'_>) -> Option<Vec<u8>>;
}

impl<F> FrameSource for F
where
    F: FnMut(&FrameRequest<'_>) -> Option<Vec<u8>> + Send,
{
    fn next_frame(&mut self, request: &FrameRequest<'_>) -> Option<Vec<u8>> {
        self(request)
    }
}

/// The built-in test patterns, picked by the `PixelFormat` register: a Mono8 gradient, a
/// 16-bit gradient for Mono10/Mono16, and the Linescan3D profile scene for `Coord3D_C16`.
/// This is the source a camera starts with.
#[derive(Debug, Clone, Copy, Default)]
pub struct PatternSource;

impl FrameSource for PatternSource {
    fn next_frame(&mut self, request: &FrameRequest<'_>) -> Option<Vec<u8>> {
        Some(pattern::generate_for_format(
            request.width,
            request.height,
            request.frame_id,
            request.pixel_format,
            request.pixel_format_name,
        ))
    }
}

/// The source shared by a [`crate::FakeCamera`] and its GVSP thread.
pub(crate) type SharedSource = Arc<Mutex<Box<dyn FrameSource>>>;
