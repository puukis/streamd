#[cfg(target_os = "linux")]
use std::os::fd::OwnedFd;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
use anyhow::bail;
use anyhow::Result;
use streamd_proto::packets::DisplayInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShmPixelFormat {
    Xrgb8888,
    Argb8888,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmabufPixelFormat {
    Xrgb8888,
    Argb8888,
}

/// A captured frame ready for encoding.
pub enum CaptureFrame {
    /// Frame data in a CPU-accessible shared memory buffer.
    /// Layout is little-endian XRGB8888 or ARGB8888.
    Shm {
        data: Vec<u8>,
        width: u32,
        height: u32,
        stride: u32,
        format: ShmPixelFormat,
        timestamp_us: u64,
    },
    /// Frame as a DMA-BUF file descriptor pointing at GPU memory.
    #[cfg(target_os = "linux")]
    DmaBuf {
        fd: OwnedFd,
        buffer_id: u64,
        width: u32,
        height: u32,
        pitch: u32,
        offset: u32,
        allocation_size: u64,
        format: DmabufPixelFormat,
        modifier: u64,
        timestamp_us: u64,
    },
}

#[cfg(target_os = "linux")]
pub mod wayland;

#[cfg(target_os = "windows")]
pub mod windows;

pub fn list_displays() -> Result<Vec<DisplayInfo>> {
    #[cfg(target_os = "linux")]
    {
        return wayland::list_displays();
    }

    #[cfg(target_os = "windows")]
    {
        return windows::list_displays();
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        bail!("display enumeration is only implemented on Linux and Windows");
    }
}
