//! Windows desktop capture via DXGI Desktop Duplication.

use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::Sender;
use std::time::{SystemTime, UNIX_EPOCH};
use streamd_proto::packets::DisplayInfo;
use tracing::{info, warn};
use windows::{
    core::Interface,
    Win32::Graphics::{
        Direct3D::{
            D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_0,
            D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
        },
        Direct3D11::{
            D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
            D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
            D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
        },
        Dxgi::{
            Common::{DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8X8_UNORM},
            CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication,
            IDXGIResource, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_WAIT_TIMEOUT,
            DXGI_OUTDUPL_DESC, DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTPUT_DESC,
        },
    },
};

use crate::capture::{CaptureFrame, ShmPixelFormat};

const FRAME_TIMEOUT_MS: u32 = 500;

pub struct WindowsCapture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutput1,
    duplication: IDXGIOutputDuplication,
    staging_texture: Option<ID3D11Texture2D>,
    staging_resource: Option<ID3D11Resource>,
    frame_tx: Sender<CaptureFrame>,
    output_name: String,
}

impl WindowsCapture {
    pub fn new(display_id: Option<&str>, frame_tx: Sender<CaptureFrame>) -> Result<Self> {
        let selected = select_output(display_id).context("find a desktop output for capture")?;
        let (device, context) =
            create_device(&selected.adapter).context("create D3D11 device for capture")?;
        let duplication = duplicate_output(&selected.output, &device)
            .context("create DXGI desktop duplication session")?;

        info!(
            "Windows desktop duplication initialised on output {} ({})",
            selected.info.name, selected.info.id
        );

        Ok(Self {
            device,
            context,
            output: selected.output,
            duplication,
            staging_texture: None,
            staging_resource: None,
            frame_tx,
            output_name: selected.info.name,
        })
    }

    pub fn pump(&mut self) -> Result<()> {
        loop {
            let mut frame_info: DXGI_OUTDUPL_FRAME_INFO = unsafe { std::mem::zeroed() };
            let mut resource: Option<IDXGIResource> = None;
            match unsafe {
                self.duplication
                    .AcquireNextFrame(FRAME_TIMEOUT_MS, &mut frame_info, &mut resource)
            } {
                Ok(()) => {
                    let _release = ReleaseFrameGuard::new(&self.duplication);
                    let resource = resource.context("desktop duplication returned no frame")?;
                    let texture: ID3D11Texture2D =
                        resource.cast().context("cast frame to ID3D11Texture2D")?;
                    let texture_desc = get_texture_desc(&texture);
                    let format =
                        pixel_format_from_dxgi(texture_desc.Format).with_context(|| {
                            format!(
                                "unsupported desktop duplication format {:?} on output {}",
                                texture_desc.Format, self.output_name
                            )
                        })?;

                    self.ensure_staging_texture(&texture_desc)
                        .context("prepare D3D11 staging texture")?;

                    self.staging_texture
                        .as_ref()
                        .context("staging texture missing after allocation")?;
                    let staging_resource = self
                        .staging_resource
                        .as_ref()
                        .context("staging resource missing after allocation")?;
                    let source_resource: ID3D11Resource = texture
                        .cast()
                        .context("cast desktop texture to ID3D11Resource")?;

                    unsafe {
                        self.context
                            .CopyResource(staging_resource, &source_resource);
                    }

                    let mut mapped: D3D11_MAPPED_SUBRESOURCE = unsafe { std::mem::zeroed() };
                    unsafe {
                        self.context
                            .Map(staging_resource, 0, D3D11_MAP_READ, 0, &mut mapped)
                    }
                    .context("map staging texture for CPU readback")?;

                    let copy_result =
                        copy_mapped_frame(&mapped, texture_desc.Width, texture_desc.Height, format);
                    unsafe {
                        self.context.Unmap(staging_resource, 0);
                    }
                    let (data, stride) = copy_result?;

                    self.frame_tx
                        .send(CaptureFrame::Shm {
                            data,
                            width: texture_desc.Width,
                            height: texture_desc.Height,
                            stride,
                            format,
                            timestamp_us: capture_timestamp_us(),
                        })
                        .context("capture frame receiver dropped")?;
                    return Ok(());
                }
                Err(err) if err.code() == DXGI_ERROR_WAIT_TIMEOUT => continue,
                Err(err) if err.code() == DXGI_ERROR_ACCESS_LOST => {
                    warn!(
                        "desktop duplication access lost on output {}: {}; recreating session",
                        self.output_name, err
                    );
                    self.recreate_duplication()
                        .context("recreate DXGI desktop duplication session")?;
                }
                Err(err) => {
                    return Err(anyhow!(
                        "AcquireNextFrame failed on {}: {err}",
                        self.output_name
                    ));
                }
            }
        }
    }

    fn recreate_duplication(&mut self) -> Result<()> {
        self.duplication = duplicate_output(&self.output, &self.device)
            .context("duplicate output after access loss")?;
        self.staging_texture = None;
        self.staging_resource = None;
        Ok(())
    }

    fn ensure_staging_texture(&mut self, source_desc: &D3D11_TEXTURE2D_DESC) -> Result<()> {
        let recreate = match self.staging_texture.as_ref() {
            Some(existing) => {
                let desc = get_texture_desc(existing);
                desc.Width != source_desc.Width
                    || desc.Height != source_desc.Height
                    || desc.Format != source_desc.Format
            }
            None => true,
        };

        if !recreate {
            return Ok(());
        }

        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: source_desc.Width,
            Height: source_desc.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: source_desc.Format,
            SampleDesc: source_desc.SampleDesc,
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };

        let mut staging = None;
        unsafe {
            self.device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
        }
        .context("CreateTexture2D for staging readback")?;
        let staging = staging.context("CreateTexture2D returned no staging texture")?;
        let staging_resource: ID3D11Resource = staging
            .cast()
            .context("cast staging texture to ID3D11Resource")?;

        self.staging_texture = Some(staging);
        self.staging_resource = Some(staging_resource);
        Ok(())
    }
}

struct SelectedOutput {
    adapter: IDXGIAdapter1,
    output: IDXGIOutput1,
    info: DisplayInfo,
}

struct ReleaseFrameGuard<'a> {
    duplication: &'a IDXGIOutputDuplication,
    active: bool,
}

impl<'a> ReleaseFrameGuard<'a> {
    fn new(duplication: &'a IDXGIOutputDuplication) -> Self {
        Self {
            duplication,
            active: true,
        }
    }
}

impl Drop for ReleaseFrameGuard<'_> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let _ = unsafe { self.duplication.ReleaseFrame() };
    }
}

pub fn list_displays() -> Result<Vec<DisplayInfo>> {
    Ok(enumerate_outputs()?
        .into_iter()
        .map(|output| output.info)
        .collect())
}

fn select_output(display_id: Option<&str>) -> Result<SelectedOutput> {
    let outputs = enumerate_outputs()?;
    if outputs.is_empty() {
        bail!("no attached desktop output was found for capture");
    }

    if let Some(display_id) = display_id {
        return outputs
            .into_iter()
            .find(|output| output.info.id == display_id)
            .with_context(|| format!("Windows display {display_id:?} is not available"));
    }

    Ok(outputs
        .into_iter()
        .next()
        .expect("checked non-empty output list"))
}

fn enumerate_outputs() -> Result<Vec<SelectedOutput>> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.context("CreateDXGIFactory1")?;
    let mut displays = Vec::new();
    let mut display_index = 0u32;

    let mut adapter_index = 0;
    loop {
        let adapter = match unsafe { factory.EnumAdapters1(adapter_index) } {
            Ok(adapter) => adapter,
            Err(err) if err.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(err) => return Err(anyhow!("EnumAdapters1({adapter_index}) failed: {err}")),
        };
        let adapter_desc = unsafe { adapter.GetDesc1() }.context("IDXGIAdapter1::GetDesc1")?;
        let adapter_name = wide_string(&adapter_desc.Description);

        let mut output_index = 0;
        loop {
            let output = match unsafe { adapter.EnumOutputs(output_index) } {
                Ok(output) => output,
                Err(err) if err.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(err) => {
                    return Err(anyhow!(
                        "EnumOutputs({adapter_index}, {output_index}) failed: {err}"
                    ))
                }
            };

            let desc = unsafe { output.GetDesc() }.context("IDXGIOutput::GetDesc")?;
            if desc.AttachedToDesktop.as_bool() {
                let output1: IDXGIOutput1 = output.cast().context("cast output to IDXGIOutput1")?;
                let name = output_name(&desc);
                let description = windows_display_description(&adapter_name, &name);
                displays.push(SelectedOutput {
                    adapter: adapter.clone(),
                    output: output1,
                    info: DisplayInfo {
                        id: windows_display_id(adapter_index as u32, output_index as u32),
                        index: display_index,
                        name,
                        description,
                        width: display_width(&desc),
                        height: display_height(&desc),
                    },
                });
                display_index += 1;
            }

            output_index += 1;
        }

        adapter_index += 1;
    }

    if displays.is_empty() {
        bail!("no attached desktop output was found for capture");
    }

    Ok(displays)
}

fn create_device(adapter: &IDXGIAdapter1) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let feature_levels: [D3D_FEATURE_LEVEL; 4] = [
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
    ];

    let mut device = None;
    let mut context = None;
    let mut selected_feature = D3D_FEATURE_LEVEL_11_0;
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            Default::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&feature_levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut selected_feature),
            Some(&mut context),
        )
    }
    .context("D3D11CreateDevice")?;

    let device = device.context("D3D11CreateDevice returned no device")?;
    let context = context.context("D3D11CreateDevice returned no device context")?;
    info!("D3D11 capture device initialised at feature level {selected_feature:?}");
    Ok((device, context))
}

fn duplicate_output(
    output: &IDXGIOutput1,
    device: &ID3D11Device,
) -> Result<IDXGIOutputDuplication> {
    let duplication =
        unsafe { output.DuplicateOutput(device) }.context("IDXGIOutput1::DuplicateOutput")?;
    let desc: DXGI_OUTDUPL_DESC = unsafe { duplication.GetDesc() };
    let format = pixel_format_from_dxgi(desc.ModeDesc.Format).with_context(|| {
        format!(
            "desktop duplication reported unsupported format {:?}",
            desc.ModeDesc.Format
        )
    })?;
    info!(
        "desktop duplication ready: {}x{} {:?}",
        desc.ModeDesc.Width, desc.ModeDesc.Height, format
    );
    Ok(duplication)
}

fn get_texture_desc(texture: &ID3D11Texture2D) -> D3D11_TEXTURE2D_DESC {
    let mut desc: D3D11_TEXTURE2D_DESC = unsafe { std::mem::zeroed() };
    unsafe {
        texture.GetDesc(&mut desc);
    }
    desc
}

fn copy_mapped_frame(
    mapped: &D3D11_MAPPED_SUBRESOURCE,
    width: u32,
    height: u32,
    format: ShmPixelFormat,
) -> Result<(Vec<u8>, u32)> {
    let bytes_per_row = usize::try_from(width)
        .context("capture width overflow")?
        .checked_mul(4)
        .context("capture row size overflow")?;
    let src_stride = usize::try_from(mapped.RowPitch).context("mapped row pitch overflow")?;
    let height = usize::try_from(height).context("capture height overflow")?;
    if mapped.pData.is_null() {
        bail!("desktop duplication returned a null staging mapping");
    }
    if src_stride < bytes_per_row {
        bail!("desktop duplication row pitch {src_stride} is smaller than {bytes_per_row}");
    }

    let dst_stride = u32::try_from(bytes_per_row).context("tight capture stride overflow")?;
    let mut data = vec![
        0u8;
        bytes_per_row
            .checked_mul(height)
            .context("capture buffer size overflow")?
    ];
    let src_base = mapped.pData.cast::<u8>();

    for row in 0..height {
        let src_offset = row * src_stride;
        let dst_offset = row * bytes_per_row;
        unsafe {
            std::ptr::copy_nonoverlapping(
                src_base.add(src_offset),
                data.as_mut_ptr().add(dst_offset),
                bytes_per_row,
            );
        }
    }

    // BGRA / BGRX byte order in memory matches little-endian ARGB / XRGB.
    let format = match format {
        ShmPixelFormat::Argb8888 => ShmPixelFormat::Argb8888,
        ShmPixelFormat::Xrgb8888 => ShmPixelFormat::Xrgb8888,
    };

    Ok((data, dst_stride))
}

fn pixel_format_from_dxgi(format: DXGI_FORMAT) -> Option<ShmPixelFormat> {
    match format {
        DXGI_FORMAT_B8G8R8A8_UNORM => Some(ShmPixelFormat::Argb8888),
        DXGI_FORMAT_B8G8R8X8_UNORM => Some(ShmPixelFormat::Xrgb8888),
        _ => None,
    }
}

fn output_name(desc: &DXGI_OUTPUT_DESC) -> String {
    wide_string(&desc.DeviceName)
}

fn wide_string(chars: &[u16]) -> String {
    let end = chars.iter().position(|&ch| ch == 0).unwrap_or(chars.len());
    String::from_utf16_lossy(&chars[..end])
}

fn windows_display_id(adapter_index: u32, output_index: u32) -> String {
    format!("windows:{adapter_index}:{output_index}")
}

fn windows_display_description(adapter_name: &str, output_name: &str) -> Option<String> {
    let adapter_name = adapter_name.trim();
    if adapter_name.is_empty() || adapter_name == output_name {
        None
    } else {
        Some(format!("{adapter_name} / {output_name}"))
    }
}

fn display_width(desc: &DXGI_OUTPUT_DESC) -> u32 {
    (desc.DesktopCoordinates.right - desc.DesktopCoordinates.left).max(0) as u32
}

fn display_height(desc: &DXGI_OUTPUT_DESC) -> u32 {
    (desc.DesktopCoordinates.bottom - desc.DesktopCoordinates.top).max(0) as u32
}

fn capture_timestamp_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}
