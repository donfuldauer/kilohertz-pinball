//! Synchronous texture readback for tests and tools (bounded waits).

use crate::gpu::{GpuContext, GpuError};
use crate::pbr::HdrImage;

/// IEEE half → single.
pub(crate) fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0f32 } else { 1.0 };
    let exp = u32::from((h >> 10) & 0x1f);
    let frac = u32::from(h & 0x3ff);
    match exp {
        0 => sign * frac as f32 * (1.0 / 16_777_216.0),
        31 if frac == 0 => sign * f32::INFINITY,
        31 => f32::NAN,
        _ => f32::from_bits((u32::from(h & 0x8000) << 16) | ((exp + 112) << 23) | (frac << 13)),
    }
}

/// Copies array layer `layer` of mip 0 of `texture` (`bytes_per_pixel` per
/// texel) to the CPU: tightly packed rows, row 0 at the top. Submits its own
/// command buffer and waits (bounded).
pub(crate) fn read_layer(
    ctx: &GpuContext,
    texture: &wgpu::Texture,
    layer: u32,
    bytes_per_pixel: u32,
) -> Result<Vec<u8>, GpuError> {
    let device = ctx.device();
    let (width, height) = (texture.width(), texture.height());
    let row = (width * bytes_per_pixel) as usize;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
    let padded = row.div_ceil(align) * align;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("post readback"),
        size: crate::gpu::readback_buffer_size(device, padded, height)?,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("post readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: layer,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded as u32),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue().submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    crate::gpu::finish_map(device, &rx, crate::gpu::READBACK_WAIT_TIMEOUT)?;
    let mut out = Vec::with_capacity(row * height as usize);
    {
        let mapped = buffer
            .get_mapped_range(..)
            .map_err(|e| GpuError::Readback(format!("get_mapped_range: {e:?}")))?;
        for y in 0..height as usize {
            out.extend_from_slice(&mapped[y * padded..y * padded + row]);
        }
    }
    buffer.unmap();
    Ok(out)
}

/// Reads layer `layer` of an `Rgba16Float` texture as linear `f32` RGBA.
pub(crate) fn read_hdr_layer(
    ctx: &GpuContext,
    texture: &wgpu::Texture,
    layer: u32,
) -> Result<HdrImage, GpuError> {
    let bytes = read_layer(ctx, texture, layer, 8)?;
    let pixels = bytes
        .chunks_exact(8)
        .map(|px| {
            let h = |k: usize| f16_to_f32(u16::from_le_bytes([px[2 * k], px[2 * k + 1]]));
            [h(0), h(1), h(2), h(3)]
        })
        .collect();
    Ok(HdrImage {
        width: texture.width(),
        height: texture.height(),
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_decode() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert_eq!(f16_to_f32(0x7bff), 65504.0);
        assert_eq!(f16_to_f32(0x0001), 1.0 / 16_777_216.0);
        assert_eq!(f16_to_f32(0x7c00), f32::INFINITY);
        assert!(f16_to_f32(0x7e00).is_nan());
    }
}
