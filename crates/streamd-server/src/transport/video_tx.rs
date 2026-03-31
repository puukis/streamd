//! UDP video sender: fragments encoded NAL slices and sends them to the client.

use anyhow::{Context, Result};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use streamd_proto::packets::{VideoFlags, MTU_LAN, MTU_WAN};
use tracing::debug;

/// A handle for sending encoded video frames over UDP.
pub struct VideoSender {
    socket: std::net::UdpSocket,
    #[allow(dead_code)]
    remote_addr: SocketAddr,
    frame_seq: Arc<AtomicU32>,
    mtu: usize,
}

impl VideoSender {
    /// Bind to `local_port` and target `remote_addr`.
    /// Pass `jumbo=true` on LAN for larger MTU (reduces fragment count).
    pub fn new(local_port: u16, remote_addr: SocketAddr, jumbo: bool) -> Result<Self> {
        let bind_addr: SocketAddr = format!("0.0.0.0:{local_port}").parse()?;
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
            .context("create UDP socket")?;

        socket.set_reuse_address(true)?;
        // Low-delay DSCP marking
        socket.set_tos(0x10)?; // IPTOS_LOWDELAY
                               // Large send buffer to absorb bursts
        socket.set_send_buffer_size(4 * 1024 * 1024)?;
        socket.bind(&bind_addr.into())?;

        let udp: std::net::UdpSocket = socket.into();
        udp.connect(remote_addr)?;

        let mtu = if jumbo { MTU_LAN } else { MTU_WAN };

        Ok(Self {
            socket: udp,
            remote_addr,
            frame_seq: Arc::new(AtomicU32::new(0)),
            mtu,
        })
    }

    /// Send one encoded frame consisting of one or more slices.
    ///
    /// `slices` is a vec of NAL-unit byte ranges; each slice will be
    /// fragmented independently so the receiver can decode slice 0
    /// while slice 1 is still in flight.
    pub fn send_frame(&self, slices: &[Vec<u8>], is_keyframe: bool, timestamp_us: u64) {
        let frame_seq = self.frame_seq.fetch_add(1, Ordering::Relaxed);
        let num_slices = slices.len();

        for (slice_idx, slice_data) in slices.iter().enumerate() {
            let is_last_slice = slice_idx == num_slices - 1;
            self.send_slice(
                frame_seq,
                timestamp_us,
                slice_idx as u8,
                is_last_slice,
                is_keyframe && slice_idx == 0,
                slice_data,
            );
        }
    }

    fn send_slice(
        &self,
        frame_seq: u32,
        timestamp_us: u64,
        slice_idx: u8,
        is_last_slice: bool,
        is_keyframe: bool,
        data: &[u8],
    ) {
        // Header is serialized as fixed bytes inline (not via bincode to avoid overhead)
        let header_size = 18;
        let payload_size = self.mtu - header_size;
        let total_frags = data.len().div_ceil(payload_size) as u16;

        for (frag_idx, chunk) in data.chunks(payload_size).enumerate() {
            let frag_idx = frag_idx as u16;
            let mut flags = VideoFlags::empty();
            if is_keyframe && frag_idx == 0 {
                flags |= VideoFlags::KEY_FRAME;
            }
            if is_last_slice && frag_idx == total_frags - 1 {
                flags |= VideoFlags::LAST_SLICE;
            }

            let mut packet = Vec::with_capacity(header_size + chunk.len());
            // Manual little-endian encoding for the header (zero-alloc hot path)
            packet.extend_from_slice(&frame_seq.to_le_bytes()); // 4
            packet.extend_from_slice(&timestamp_us.to_le_bytes()); // 8
            packet.push(slice_idx); // 1
            packet.push(flags.bits()); // 1
            packet.extend_from_slice(&frag_idx.to_le_bytes()); // 2
            packet.extend_from_slice(&total_frags.to_le_bytes()); // 2
            packet.extend_from_slice(chunk);

            if let Err(e) = self.socket.send(&packet) {
                debug!("UDP send error: {e}");
            }
        }
    }

    #[allow(dead_code)]
    pub fn set_remote(&mut self, addr: SocketAddr) -> Result<()> {
        self.socket.connect(addr)?;
        self.remote_addr = addr;
        Ok(())
    }
}
