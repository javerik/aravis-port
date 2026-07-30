use std::collections::BTreeMap;
use std::time::Instant;

use aravis_port_core::gvcp::PacketResend;
use aravis_port_core::gvsp::{ContentType, GvspHeader, GvspStatus, LeaderPayload};
use aravis_port_core::memory::{Buffer, BufferStatus, BufferPoolStreamSide, PayloadType};

use crate::config::StreamConfig;
use crate::ResendRequester;

const STATUS_LEN: usize = 2;
const STANDARD_HEADER_LEN: usize = 6;
const EXTENDED_HEADER_LEN: usize = 18;

/// A frame more than this many ids behind the newest seen is treated as stale and discarded,
/// matching the reference implementation's late-frame threshold. This is a simplified,
/// non-wraparound-aware distance check (documented limitation): correct for the lifetime of a
/// single streaming session well under 2^64 frames, but doesn't special-case 16-bit standard
/// frame-id wraparound at 65536 frames the way a long-running standard-id stream eventually
/// would need to.
const LATE_FRAME_THRESHOLD: u64 = 100;

/// `GevSCPSPacketSize` counts the whole datagram as it appears on the wire, so the IP and UDP
/// headers come out of the same budget as the GVSP payload — confirmed against the live
/// C5-2040-GigE, which at `GevSCPSPacketSize = 1400` sends 1364-byte payloads
/// (1400 - 20 - 8 - 8). Omitting these 28 bytes makes every payload offset drift by 28 per
/// packet, which silently interleaves zero gaps through the image rather than failing.
const IP_HEADER_LEN: usize = 20;
const UDP_HEADER_LEN: usize = 8;

fn per_packet_capacity(packet_size: u16, extended: bool) -> usize {
    let overhead = IP_HEADER_LEN
        + UDP_HEADER_LEN
        + STATUS_LEN
        + if extended { EXTENDED_HEADER_LEN } else { STANDARD_HEADER_LEN };
    (packet_size as usize).saturating_sub(overhead).max(1)
}

struct PacketSlot {
    received: bool,
    deadline: Instant,
}

struct FrameAssembly {
    extended: bool,
    buffer: Buffer,
    /// Stride between consecutive payload packets' offsets. Seeded from
    /// [`per_packet_capacity`] and then corrected to the observed length of packet 1, which is
    /// authoritative: every payload packet but the last is full-size, so packet 1 is full-size
    /// whenever the frame has more than one payload packet — and when it's the only one, the
    /// stride is never used.
    packet_capacity: usize,
    /// How many payload packets have been written into `buffer` so far. Used to tell whether a
    /// late stride correction can still be applied safely.
    payloads_placed: usize,
    /// Set when a stride correction arrived too late to apply, meaning the assembled bytes are
    /// misaligned. Sticky, and forced onto the buffer at close time even if the frame otherwise
    /// looks complete — the packets all arrived, they just can't be laid out correctly.
    size_mismatch: bool,
    /// Grows on demand as packet ids are observed; index == GVSP packet id (0 = leader).
    slots: Vec<PacketSlot>,
    trailer_packet_id: Option<usize>,
    last_valid_contiguous: i64,
    resend_requests_sent: usize,
    last_packet_time: Instant,
    disable_resend: bool,
}

impl FrameAssembly {
    fn new(frame_id: u64, extended: bool, mut buffer: Buffer, packet_capacity: usize, now: Instant) -> Self {
        buffer.reset_for_reuse();
        buffer.frame_id = frame_id;
        buffer.status = BufferStatus::Filling;
        Self {
            extended,
            buffer,
            packet_capacity,
            payloads_placed: 0,
            size_mismatch: false,
            slots: Vec::new(),
            trailer_packet_id: None,
            last_valid_contiguous: -1,
            resend_requests_sent: 0,
            last_packet_time: now,
            disable_resend: false,
        }
    }

    fn ensure_slot(&mut self, packet_id: usize, now: Instant, cfg: &StreamConfig) {
        while self.slots.len() <= packet_id {
            self.slots.push(PacketSlot {
                received: false,
                deadline: now + cfg.initial_packet_timeout,
            });
        }
    }

    fn recompute_last_valid_contiguous(&mut self) {
        let mut i = (self.last_valid_contiguous + 1) as usize;
        while i < self.slots.len() && self.slots[i].received {
            i += 1;
        }
        self.last_valid_contiguous = i as i64 - 1;
    }

    fn is_complete(&self) -> bool {
        match self.trailer_packet_id {
            Some(t) => self.last_valid_contiguous >= t as i64,
            None => false,
        }
    }

    fn ingest(&mut self, status: GvspStatus, header: &GvspHeader, payload: &[u8], now: Instant, cfg: &StreamConfig) {
        self.last_packet_time = now;
        if status.is_error() {
            // Some error statuses (packet already evicted from the device's send buffer, etc.)
            // mean requesting a resend would never succeed — stop trying for this frame.
            self.disable_resend = true;
        }

        let packet_id = header.packet_id() as usize;
        self.ensure_slot(packet_id, now, cfg);

        match header.content_type() {
            ContentType::Leader => {
                if let Ok(leader) = LeaderPayload::decode(payload) {
                    self.buffer.payload_type = match leader.kind() {
                        aravis_port_core::gvsp::PayloadKind::Image => PayloadType::Image,
                        aravis_port_core::gvsp::PayloadKind::ChunkData => PayloadType::ChunkData,
                        aravis_port_core::gvsp::PayloadKind::RawData => PayloadType::RawData,
                        _ => PayloadType::Unknown,
                    };
                    self.buffer.timestamp_ns = leader.timestamp;
                    if let Some(img) = leader.image {
                        self.buffer.image = Some(aravis_port_core::memory::ImageInfo {
                            pixel_format: img.pixel_format,
                            width: img.width,
                            height: img.height,
                            x_offset: img.x_offset,
                            y_offset: img.y_offset,
                        });
                    }
                }
                self.slots[packet_id].received = true;
            }
            ContentType::Trailer => {
                self.trailer_packet_id = Some(packet_id);
                self.slots[packet_id].received = true;
            }
            ContentType::Payload => {
                if packet_id >= 1 {
                    // Packet 1 is authoritative for the stride (see `packet_capacity`). Cameras
                    // vary in how they account for header overhead against
                    // `GevSCPSPacketSize`, so trust what actually arrived over the computed
                    // estimate. In the normal in-order case nothing has been placed yet and the
                    // correction is free; if packets got reordered ahead of packet 1 and the
                    // stride was wrong, the already-written offsets cannot be fixed after the
                    // fact, so fail the frame loudly instead of returning interleaved zeros.
                    if packet_id == 1 && payload.len() != self.packet_capacity {
                        if self.payloads_placed == 0 {
                            log::debug!(
                                "correcting payload stride from {} to {} (observed on packet 1)",
                                self.packet_capacity,
                                payload.len()
                            );
                            self.packet_capacity = payload.len();
                        } else {
                            log::warn!(
                                "payload stride {} disagrees with packet 1's {} after {} packet(s) already placed; \
                                 failing frame {}",
                                self.packet_capacity,
                                payload.len(),
                                self.payloads_placed,
                                self.buffer.frame_id
                            );
                            self.size_mismatch = true;
                        }
                    }
                    let offset = (packet_id - 1) * self.packet_capacity;
                    let end = offset + payload.len();
                    if self.buffer.data().len() < end {
                        self.buffer.data_mut().resize(end, 0);
                    }
                    self.buffer.data_mut()[offset..end].copy_from_slice(payload);
                    self.payloads_placed += 1;
                }
                self.slots[packet_id].received = true;
            }
            _ => {}
        }
        self.recompute_last_valid_contiguous();
    }

    fn missing_check(&mut self, frame_id: u64, requester: &mut dyn ResendRequester, cfg: &StreamConfig, now: Instant) {
        if self.disable_resend {
            return;
        }
        // A ratio of exactly 0 means "never resend" and must floor to a hard 0 cap; any positive
        // ratio must allow at least one resend attempt even on a frame with only a handful of
        // tracked slots — naively flooring `n_slots * ratio` would otherwise round tiny frames
        // down to 0 and silently suppress all resends.
        let cap = if cfg.packet_request_ratio <= 0.0 {
            0
        } else {
            ((self.slots.len() as f32 * cfg.packet_request_ratio) as usize).max(1)
        };
        let start = (self.last_valid_contiguous + 1) as usize;
        let mut run_start: Option<usize> = None;
        let mut i = start;
        while i < self.slots.len() {
            let missing = !self.slots[i].received && now > self.slots[i].deadline;
            if missing {
                run_start.get_or_insert(i);
            } else if let Some(rs) = run_start.take() {
                self.request_resend_run(frame_id, rs, i - 1, requester, cfg, cap, now);
            }
            i += 1;
        }
        if let Some(rs) = run_start {
            self.request_resend_run(frame_id, rs, self.slots.len() - 1, requester, cfg, cap, now);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn request_resend_run(
        &mut self,
        frame_id: u64,
        first: usize,
        last: usize,
        requester: &mut dyn ResendRequester,
        cfg: &StreamConfig,
        cap: usize,
        now: Instant,
    ) {
        if self.resend_requests_sent >= cap {
            return;
        }
        let resend = if self.extended {
            PacketResend::Extended {
                frame_id,
                first_block: first as u32,
                last_block: last as u32,
            }
        } else {
            PacketResend::Standard {
                frame_id: frame_id as u32,
                first_block: first as u32,
                last_block: last as u32,
            }
        };
        requester.request_resend(resend);
        let new_deadline = now + cfg.packet_timeout;
        for slot in &mut self.slots[first..=last] {
            slot.deadline = new_deadline;
        }
        self.resend_requests_sent += last - first + 1;
    }
}

/// Tracks in-flight frames across a GVSP stream, reassembling out-of-order packets, requesting
/// resends for gaps, and closing frames as complete/timed-out/superseded.
pub struct Reassembler {
    frames: BTreeMap<u64, FrameAssembly>,
    cfg: StreamConfig,
    newest_seen: u64,
}

impl Reassembler {
    pub fn new(cfg: StreamConfig) -> Self {
        Self {
            frames: BTreeMap::new(),
            cfg,
            newest_seen: 0,
        }
    }

    fn is_late(&self, frame_id: u64) -> bool {
        self.newest_seen > frame_id && self.newest_seen - frame_id > LATE_FRAME_THRESHOLD
    }

    /// Feed one parsed GVSP datagram in. Returns any buffers that closed as a direct result
    /// (usually at most one, but resend-triggered supersession can close more).
    pub fn process_packet(
        &mut self,
        status: GvspStatus,
        header: GvspHeader,
        payload: &[u8],
        pool: &BufferPoolStreamSide,
        requester: &mut dyn ResendRequester,
        now: Instant,
    ) -> Vec<Buffer> {
        let frame_id = header.frame_id();
        if frame_id > self.newest_seen {
            self.newest_seen = frame_id;
        }
        if self.is_late(frame_id) && !self.frames.contains_key(&frame_id) {
            return Vec::new();
        }

        if !self.frames.contains_key(&frame_id) {
            let Some(buffer) = pool.pop_input_buffer() else {
                return Vec::new(); // buffer-pool underrun: drop this frame's data
            };
            let extended = header.is_extended();
            let capacity = per_packet_capacity(self.cfg.packet_size, extended);
            self.frames.insert(frame_id, FrameAssembly::new(frame_id, extended, buffer, capacity, now));
        }

        if let Some(frame) = self.frames.get_mut(&frame_id) {
            frame.ingest(status, &header, payload, now, &self.cfg);
            frame.missing_check(frame_id, requester, &self.cfg, now);
        }

        self.close_ready_frames(now)
    }

    /// Age out stalled frames even without new packet arrivals; call periodically (e.g. on a
    /// receive-timeout tick).
    pub fn tick(&mut self, requester: &mut dyn ResendRequester, now: Instant) -> Vec<Buffer> {
        let ids: Vec<u64> = self.frames.keys().copied().collect();
        for id in ids {
            if let Some(frame) = self.frames.get_mut(&id) {
                frame.missing_check(id, requester, &self.cfg, now);
            }
        }
        self.close_ready_frames(now)
    }

    fn close_ready_frames(&mut self, now: Instant) -> Vec<Buffer> {
        let ids: Vec<u64> = self.frames.keys().copied().collect();
        let mut to_close = Vec::new();
        for (rank, &id) in ids.iter().enumerate() {
            let frame = self.frames.get(&id).unwrap();
            let newer_count = ids.len() - 1 - rank;
            if frame.is_complete() {
                to_close.push((id, BufferStatus::Success));
            } else if now.saturating_duration_since(frame.last_packet_time) >= self.cfg.frame_retention {
                // Only the leader (or nothing) ever arrived -> Timeout (the transfer never
                // really started); some payload arrived but the frame is still incomplete ->
                // MissingPackets (an actual mid-transfer loss).
                let status = if frame.last_valid_contiguous <= 0 {
                    BufferStatus::Timeout
                } else {
                    BufferStatus::MissingPackets
                };
                to_close.push((id, status));
            } else if newer_count >= 2 {
                to_close.push((id, BufferStatus::MissingPackets));
            }
        }

        let mut closed = Vec::with_capacity(to_close.len());
        for (id, status) in to_close {
            if let Some(mut frame) = self.frames.remove(&id) {
                frame.buffer.status = if frame.size_mismatch {
                    BufferStatus::SizeMismatch
                } else {
                    status
                };
                closed.push(frame.buffer);
            }
        }
        closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aravis_port_core::gvsp::{ContentType, GvspHeader, GvspStatus, ImageInfos, PayloadKind};
    use aravis_port_core::memory::new_buffer_pool;

    struct MockRequester {
        requests: Vec<PacketResend>,
    }
    impl ResendRequester for MockRequester {
        fn request_resend(&mut self, r: PacketResend) {
            self.requests.push(r);
        }
    }

    fn leader_bytes(image: bool) -> Vec<u8> {
        let payload = LeaderPayload {
            flags: 0,
            payload_type: PayloadKind::Image.to_u16(),
            timestamp: 1,
            image: image.then_some(ImageInfos {
                pixel_format: 0x0101_0001,
                width: 4,
                height: 1,
                x_offset: 0,
                y_offset: 0,
                x_padding: 0,
                y_padding: 0,
            }),
        };
        payload.encode()
    }

    fn trailer_bytes() -> Vec<u8> {
        aravis_port_core::gvsp::TrailerPayload {
            payload_type: 1,
            data0: 1,
        }
        .encode()
    }

    fn header(frame_id: u64, content_type: ContentType, packet_id: u32) -> GvspHeader {
        GvspHeader::Standard {
            frame_id: frame_id as u16,
            content_type,
            packet_id,
        }
    }

    #[test]
    fn payload_stride_accounts_for_ip_and_udp_headers() {
        // `GevSCPSPacketSize` budgets the whole wire datagram, so IP (20) and UDP (8) come out
        // of it alongside the GVSP header. Verified against the live C5-2040-GigE: at 1400 it
        // sends 1364-byte payloads.
        assert_eq!(per_packet_capacity(1400, false), 1364);
        assert_eq!(per_packet_capacity(1500, false), 1464);
        assert_eq!(per_packet_capacity(1400, true), 1352);
        // Degenerate sizes that can't even cover the headers must still yield a usable stride.
        assert_eq!(per_packet_capacity(12, false), 1);
    }

    #[test]
    fn stride_is_corrected_from_packet_one_when_the_estimate_is_wrong() {
        // Estimate says 4 bytes/packet, but the camera actually sends 6. Packet 1 is
        // authoritative, so the frame must still assemble contiguously with no zero gaps.
        let cfg = StreamConfig {
            packet_size: 40,
            ..StreamConfig::default()
        };
        let mut reassembler = Reassembler::new(cfg);
        let (_user, stream) = new_buffer_pool(2, 64);
        let mut requester = MockRequester { requests: Vec::new() };
        let now = Instant::now();

        let leader = leader_bytes(true);
        reassembler.process_packet(GvspStatus::Success, header(1, ContentType::Leader, 0), &leader, &stream, &mut requester, now);
        for (packet_id, payload) in [(1u32, b"AAAAAA".to_vec()), (2, b"BBBBBB".to_vec())] {
            reassembler.process_packet(
                GvspStatus::Success,
                header(1, ContentType::Payload, packet_id),
                &payload,
                &stream,
                &mut requester,
                now,
            );
        }
        let trailer = trailer_bytes();
        let closed = reassembler.process_packet(GvspStatus::Success, header(1, ContentType::Trailer, 3), &trailer, &stream, &mut requester, now);

        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].status, aravis_port_core::memory::BufferStatus::Success);
        assert_eq!(closed[0].data(), b"AAAAAABBBBBB");
    }

    #[test]
    fn a_late_stride_correction_fails_the_frame_rather_than_misaligning_it() {
        // Same wrong estimate, but packet 2 lands before packet 1, so by the time the true
        // stride is known packet 2 is already at the wrong offset and can't be moved. The frame
        // must report SizeMismatch rather than hand back interleaved zeros.
        let cfg = StreamConfig {
            packet_size: 40,
            ..StreamConfig::default()
        };
        let mut reassembler = Reassembler::new(cfg);
        let (_user, stream) = new_buffer_pool(2, 64);
        let mut requester = MockRequester { requests: Vec::new() };
        let now = Instant::now();

        let leader = leader_bytes(true);
        reassembler.process_packet(GvspStatus::Success, header(1, ContentType::Leader, 0), &leader, &stream, &mut requester, now);
        for (packet_id, payload) in [(2u32, b"BBBBBB".to_vec()), (1, b"AAAAAA".to_vec())] {
            reassembler.process_packet(
                GvspStatus::Success,
                header(1, ContentType::Payload, packet_id),
                &payload,
                &stream,
                &mut requester,
                now,
            );
        }
        let trailer = trailer_bytes();
        let closed = reassembler.process_packet(GvspStatus::Success, header(1, ContentType::Trailer, 3), &trailer, &stream, &mut requester, now);

        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].status, aravis_port_core::memory::BufferStatus::SizeMismatch);
    }

    #[test]
    fn out_of_order_packets_still_complete_the_frame() {
        // packet_size chosen so the per-payload-packet capacity is exactly 4, matching this
        // test's 4-byte payload chunks: 20 (IP) + 8 (UDP) + 8 (GVSP status+header) + 4.
        let cfg = StreamConfig {
            packet_size: 40,
            ..StreamConfig::default()
        };
        let mut reassembler = Reassembler::new(cfg);
        let (_user, stream) = new_buffer_pool(2, 64);
        let mut requester = MockRequester { requests: Vec::new() };
        let now = Instant::now();

        // Trailer (packet 3) arrives before leader (packet 0) and payloads (1, 2).
        let trailer = trailer_bytes();
        reassembler.process_packet(GvspStatus::Success, header(1, ContentType::Trailer, 3), &trailer, &stream, &mut requester, now);
        let payload2 = b"BBBB".to_vec();
        reassembler.process_packet(GvspStatus::Success, header(1, ContentType::Payload, 2), &payload2, &stream, &mut requester, now);
        let leader = leader_bytes(true);
        reassembler.process_packet(GvspStatus::Success, header(1, ContentType::Leader, 0), &leader, &stream, &mut requester, now);
        let payload1 = b"AAAA".to_vec();
        let closed = reassembler.process_packet(GvspStatus::Success, header(1, ContentType::Payload, 1), &payload1, &stream, &mut requester, now);

        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].status, aravis_port_core::memory::BufferStatus::Success);
        assert_eq!(closed[0].data(), b"AAAABBBB");
        assert_eq!(closed[0].image.unwrap().width, 4);
    }

    #[test]
    fn a_withheld_middle_packet_triggers_exactly_one_resend_request() {
        let cfg = StreamConfig {
            initial_packet_timeout: std::time::Duration::from_millis(0),
            ..StreamConfig::default()
        };
        let mut reassembler = Reassembler::new(cfg);
        let (_user, stream) = new_buffer_pool(2, 64);
        let mut requester = MockRequester { requests: Vec::new() };
        let now = Instant::now();

        reassembler.process_packet(GvspStatus::Success, header(1, ContentType::Leader, 0), &leader_bytes(false), &stream, &mut requester, now);
        // Packet 1 withheld; packet 2 arrives, creating a gap at slot 1.
        reassembler.process_packet(GvspStatus::Success, header(1, ContentType::Payload, 2), b"BBBB", &stream, &mut requester, now);

        let later = now + std::time::Duration::from_millis(5);
        reassembler.tick(&mut requester, later);

        assert_eq!(requester.requests.len(), 1);
        match requester.requests[0] {
            PacketResend::Standard { first_block, last_block, .. } => {
                assert_eq!(first_block, 1);
                assert_eq!(last_block, 1);
            }
            _ => panic!("expected standard resend"),
        }

        // A second tick before the packet_timeout elapses must not re-request.
        reassembler.tick(&mut requester, later + std::time::Duration::from_micros(1));
        assert_eq!(requester.requests.len(), 1);
    }

    #[test]
    fn exceeding_the_resend_ratio_stops_further_requests() {
        let cfg = StreamConfig {
            initial_packet_timeout: std::time::Duration::from_millis(0),
            packet_request_ratio: 0.0,
            ..StreamConfig::default()
        };
        let mut reassembler = Reassembler::new(cfg);
        let (_user, stream) = new_buffer_pool(2, 64);
        let mut requester = MockRequester { requests: Vec::new() };
        let now = Instant::now();

        reassembler.process_packet(GvspStatus::Success, header(1, ContentType::Leader, 0), &leader_bytes(false), &stream, &mut requester, now);
        reassembler.process_packet(GvspStatus::Success, header(1, ContentType::Payload, 2), b"BBBB", &stream, &mut requester, now);
        reassembler.tick(&mut requester, now + std::time::Duration::from_millis(5));

        assert!(requester.requests.is_empty(), "ratio of 0 must suppress all resend requests");
    }

    #[test]
    fn frame_with_only_a_leader_ages_out_as_timeout() {
        // Nothing beyond the leader ever arrived — the transfer never really started, so this
        // is a Timeout rather than a MissingPackets (a genuine mid-transfer loss).
        let cfg = StreamConfig {
            frame_retention: std::time::Duration::from_millis(10),
            ..StreamConfig::default()
        };
        let mut reassembler = Reassembler::new(cfg);
        let mut requester = MockRequester { requests: Vec::new() };
        let now = Instant::now();
        let (_user, stream) = new_buffer_pool(2, 64);

        reassembler.process_packet(GvspStatus::Success, header(1, ContentType::Leader, 0), &leader_bytes(false), &stream, &mut requester, now);

        let closed = reassembler.tick(&mut requester, now + std::time::Duration::from_millis(20));
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].status, aravis_port_core::memory::BufferStatus::Timeout);
    }

    #[test]
    fn frame_missing_the_trailer_after_receiving_payload_ages_out_as_missing_packets() {
        let cfg = StreamConfig {
            frame_retention: std::time::Duration::from_millis(10),
            ..StreamConfig::default()
        };
        let mut reassembler = Reassembler::new(cfg);
        let mut requester = MockRequester { requests: Vec::new() };
        let now = Instant::now();
        let (_user, stream) = new_buffer_pool(2, 64);

        reassembler.process_packet(GvspStatus::Success, header(1, ContentType::Leader, 0), &leader_bytes(false), &stream, &mut requester, now);
        reassembler.process_packet(GvspStatus::Success, header(1, ContentType::Payload, 1), b"AAAA", &stream, &mut requester, now);
        // Trailer never arrives.

        let closed = reassembler.tick(&mut requester, now + std::time::Duration::from_millis(20));
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].status, aravis_port_core::memory::BufferStatus::MissingPackets);
    }
}
