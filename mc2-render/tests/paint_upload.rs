// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
#![allow(clippy::chunks_exact_to_as_chunks)]

//! The paint and the alpha rule through a real texture upload (`Gpu::texture_2d_raw`, the call
//! `SceneRenderer::add_model` makes): a translucent vehicle mask keeps its alpha
//! (`gvactor.cpp:1143`, `1168`), a mech's painted pixels are opaque (`mech3d.cpp:1619-1662`).
//! Needs a GPU adapter (a missing one fails unless `MC2_SKIP_GPU_TESTS=1`).

#[macro_use]
mod common;

use mc2_gfx::config::RenderConfig;
use mc2_render::gpu::Gpu;
use mc2_render::scene::prepare_texture_pixels;
use mc2_render::shape::PaintRoutine;

/// A 2x2 mask: pure red and pure green at alpha 100 and 30, a mixed pixel at alpha 50, and pure
/// blue at alpha 200.
const MASK: [u8; 16] = [
    255, 0, 0, 100, //
    0, 128, 0, 30, //
    10, 20, 30, 50, //
    0, 0, 255, 200,
];

/// The uploaded texture, read back as tightly packed RGBA8.
fn read_back(gpu: &Gpu, tex: &wgpu::Texture, w: u32, h: u32) -> Vec<u8> {
    let row = (w * 4).next_multiple_of(256);
    let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("paint readback"),
        size: (row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit(Some(enc.finish()));
    let slice = buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    rx.recv().expect("map channel").expect("map");
    let data = slice.get_mapped_range().expect("mapped range");
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for r in 0..h as usize {
        let at = r * row as usize;
        out.extend_from_slice(&data[at..at + (w * 4) as usize]);
    }
    drop(data);
    buf.unmap();
    out
}

/// The paint through `prepare_texture_pixels` and then the upload, read back.
fn upload_painted(gpu: &Gpu, routine: PaintRoutine, texture_alpha: bool) -> Vec<u8> {
    let mut px = MASK.to_vec();
    // white paint: every painted channel is 255 scaled by k(v) (see `apply_paint_scheme`)
    prepare_texture_pixels(
        &mut px,
        Some((0x00ff_ffff, 0x00ff_ffff, 0x00ff_ffff)),
        routine,
        texture_alpha,
    );
    let tex = gpu.texture_2d_raw("paint mask", 2, 2, &px, false);
    read_back(gpu, &tex, 2, 2)
}

fn alphas(px: &[u8]) -> Vec<u8> {
    px.chunks_exact(4).map(|p| p[3]).collect()
}

#[test]
fn a_translucent_vehicle_mask_keeps_its_alpha_through_the_upload() {
    let cfg = RenderConfig {
        msaa: 1,
        ..RenderConfig::default()
    };
    let Some(gpu) = common::headless_or_skip("paint_upload", &cfg) else {
        return;
    };
    // the vehicle routine keeps every pixel's own alpha; the painted ones keep it too
    let vehicle = upload_painted(&gpu, PaintRoutine::Vehicle, true);
    assert_eq!(alphas(&vehicle), vec![100, 30, 50, 200]);
    assert_eq!(
        &vehicle[0..4],
        &[254, 254, 254, 100],
        "painted red keeps its alpha"
    );
    // the mech routine makes the painted pixels opaque; the mixed pixel stays as it was
    let mech = upload_painted(&gpu, PaintRoutine::Mech, true);
    assert_eq!(alphas(&mech), vec![255, 255, 50, 255]);
    // a texture without the alpha flag is opaque whatever the routine
    let flat = upload_painted(&gpu, PaintRoutine::Vehicle, false);
    assert_eq!(alphas(&flat), vec![255, 255, 255, 255]);
}
