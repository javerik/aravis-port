use std::net::{SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use aravis_port_core::gvcp::{
    self, next_packet_id, Command, GvcpHeader, GvcpPayload, PacketFlags, PacketType, PendingAck,
    ReadMemoryAck, ReadMemoryCmd, ReadRegisterAck, ReadRegisterCmd, WriteMemoryAck, WriteMemoryCmd,
    WriteRegisterAck, WriteRegisterCmd, GVCP_DATA_SIZE_MAX, HEADER_LEN,
};
use aravis_port_core::gvcp::PacketResend;
use aravis_port_core::{Error, Result};

/// Retry/timeout configuration for a [`GvcpTransaction`].
#[derive(Debug, Clone, Copy)]
pub struct TransactionConfig {
    pub retries: u32,
    pub timeout: Duration,
}

impl Default for TransactionConfig {
    fn default() -> Self {
        Self {
            retries: 5,
            timeout: Duration::from_millis(500),
        }
    }
}

/// A GVCP request/response session with a single device. Enforces the single-in-flight-request
/// rule statically via `&mut self`.
pub struct GvcpTransaction {
    socket: UdpSocket,
    next_id: u16,
    cfg: TransactionConfig,
}

impl GvcpTransaction {
    /// Bind an ephemeral local UDP port and `connect()` it to `peer`, so the kernel filters out
    /// datagrams from anyone else and we can use `send`/`recv` instead of `send_to`/`recv_from`.
    pub fn connect(peer: SocketAddrV4, cfg: TransactionConfig) -> std::io::Result<Self> {
        let socket = UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0))?;
        socket.connect(peer)?;
        Ok(Self {
            socket,
            next_id: 1,
            cfg,
        })
    }

    fn take_id(&mut self) -> u16 {
        let id = self.next_id;
        self.next_id = next_packet_id(self.next_id);
        id
    }

    /// Send `command`/`payload`, retrying up to `cfg.retries` times, and return the matching
    /// ack's header and raw body. A `PENDING_ACK` extends the deadline without consuming a retry.
    pub fn request_raw(&mut self, command: Command, ack_command: Command, payload: &[u8]) -> Result<Vec<u8>> {
        let id = self.take_id();
        let header = GvcpHeader {
            packet_type: PacketType::Cmd,
            raw_flags: PacketFlags::ACK_REQUIRED.bits(),
            command,
            size: payload.len() as u16,
            id,
        };
        let mut packet = header.to_bytes().to_vec();
        packet.extend_from_slice(payload);

        let mut retries_left = self.cfg.retries;
        loop {
            self.socket.send(&packet)?;
            let mut deadline = Instant::now() + self.cfg.timeout;

            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                self.socket.set_read_timeout(Some(remaining))?;
                let mut recv_buf = [0u8; 1024];
                match self.socket.recv(&mut recv_buf) {
                    Ok(n) => {
                        let Ok(resp) = GvcpHeader::from_bytes(&recv_buf[..n]) else {
                            continue;
                        };
                        if resp.id != id {
                            continue;
                        }
                        if matches!(resp.packet_type, PacketType::Error | PacketType::UnknownError) {
                            return Err(Error::GvcpError(resp.error_code().unwrap_or(0xff)));
                        }
                        if resp.packet_type != PacketType::Ack {
                            continue;
                        }
                        if resp.command == Command::PendingAck {
                            if let Ok(pending) = PendingAck::decode(&recv_buf[HEADER_LEN..n]) {
                                deadline = Instant::now() + Duration::from_millis(pending.timeout_ms as u64);
                            }
                            continue;
                        }
                        if resp.command == ack_command {
                            return Ok(recv_buf[HEADER_LEN..n].to_vec());
                        }
                        // Ack for a different command with a matching id: ignore and keep waiting.
                    }
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        break;
                    }
                    Err(e) => return Err(Error::Io(e)),
                }
            }

            if retries_left == 0 {
                return Err(Error::Timeout { command, id });
            }
            retries_left -= 1;
        }
    }

    fn request<P: GvcpPayload>(&mut self, command: Command, payload: &[u8]) -> Result<P> {
        let body = self.request_raw(command, P::ACK_COMMAND, payload)?;
        P::decode(&body)
    }

    pub fn read_register(&mut self, address: u32) -> Result<u32> {
        let ack: ReadRegisterAck = self.request(Command::ReadRegisterCmd, &ReadRegisterCmd { address }.encode())?;
        Ok(ack.value)
    }

    pub fn write_register(&mut self, address: u32, value: u32) -> Result<()> {
        let _: WriteRegisterAck =
            self.request(Command::WriteRegisterCmd, &WriteRegisterCmd { address, value }.encode())?;
        Ok(())
    }

    /// Read `len` bytes starting at `address`, transparently chunking into
    /// `GVCP_DATA_SIZE_MAX`-byte requests.
    pub fn read_memory(&mut self, address: u32, len: usize) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(len);
        let mut offset = 0usize;
        while offset < len {
            let wanted = (len - offset).min(GVCP_DATA_SIZE_MAX);
            // GVCP requires READ_MEMORY sizes to be a multiple of 4 bytes (word-aligned); a
            // final, non-full chunk (e.g. reading the tail of a zipped XML blob) commonly isn't,
            // so round up and discard the padding — confirmed necessary against the live
            // AT-Automation Technology C5-2040-GigE camera, which rejects unaligned reads.
            let aligned = wanted.div_ceil(4) * 4;
            let ack: ReadMemoryAck = self.request(
                Command::ReadMemoryCmd,
                &ReadMemoryCmd {
                    address: address + offset as u32,
                    size: aligned as u16,
                }
                .encode(),
            )?;
            out.extend_from_slice(&ack.data[..wanted.min(ack.data.len())]);
            offset += wanted;
        }
        out.truncate(len);
        Ok(out)
    }

    pub fn write_memory(&mut self, address: u32, data: &[u8]) -> Result<()> {
        let mut offset = 0usize;
        while offset < data.len() {
            let chunk_len = (data.len() - offset).min(GVCP_DATA_SIZE_MAX);
            let _: WriteMemoryAck = self.request(
                Command::WriteMemoryCmd,
                &WriteMemoryCmd {
                    address: address + offset as u32,
                    data: data[offset..offset + chunk_len].to_vec(),
                }
                .encode(),
            )?;
            offset += chunk_len;
        }
        Ok(())
    }

    /// Fire-and-forget a packet-resend request; the device never acks this over GVCP, it just
    /// re-emits the requested GVSP packets.
    pub fn send_packet_resend(&mut self, resend: PacketResend) -> Result<()> {
        let id = self.take_id();
        let flags = if resend.is_extended() {
            PacketFlags::EXTENDED_IDS.bits()
        } else {
            0
        };
        let payload = resend.encode();
        let header = GvcpHeader {
            packet_type: PacketType::Cmd,
            raw_flags: flags,
            command: gvcp::Command::PacketResendCmd,
            size: payload.len() as u16,
            id,
        };
        let mut packet = header.to_bytes().to_vec();
        packet.extend_from_slice(&payload);
        self.socket.send(&packet)?;
        Ok(())
    }
}
