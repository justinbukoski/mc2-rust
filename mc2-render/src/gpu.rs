//! wgpu context (headless or for a window surface), render targets and texture upload.
//!
//! Device creation follows `mc2_gfx::renderer::Renderer::new` (same instance/adapter/device
//! options, same MSAA fallback loop) and texture handling goes through the `mc2-gfx` texture
//! helpers: mip chains, the stable replacement key and the hi-res replacement lookup.  The scene
//! pipelines themselves are not the `gos_*` state machine of `mc2-gfx` (that API takes pre-projected
//! vertices), so this module owns its own device.
//!
//! Headless and windowed runs share one render path: every drawing crate draws into a
//! [`RenderTarget`] with an encoder the application owns (`draw(gpu, encoder, target, ...)`), and
//! the application submits once per frame.  Headless frames target an [`OffscreenTarget`] (read
//! back with [`RenderTarget::encode_readback`]); windowed frames target the surface texture of a
//! [`GpuSurface`].  The persistent MSAA colour and depth buffers are [`FrameAttachments`].

use crate::error::{RenderError, Result};
use mc2_gfx::config::RenderConfig;
use mc2_gfx::texture as tx;
use mc2_gfx::texture::TextureClass;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// Format of every sampled texture and of headless colour targets.
pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// The platform's preferred graphics API: Metal on Apple platforms, DX12 (with Vulkan as the
/// alternative) on Windows, Vulkan elsewhere.  `WGPU_BACKEND` overrides it.
pub fn preferred_backends() -> wgpu::Backends {
    if let Some(b) = wgpu::Backends::from_env() {
        return b;
    }
    if cfg!(any(target_os = "macos", target_os = "ios")) {
        wgpu::Backends::METAL
    } else if cfg!(target_os = "windows") {
        wgpu::Backends::DX12 | wgpu::Backends::VULKAN
    } else if cfg!(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    )) {
        wgpu::Backends::VULKAN
    } else {
        wgpu::Backends::PRIMARY
    }
}

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_name: String,
    pub backend: String,
    /// MSAA sample count actually available (<= the configured one).
    pub samples: u32,
    /// Display format of the device's targets (the format drawing crates build pipelines for and
    /// MSAA attachments use): [`COLOR_FORMAT`] headless, the surface's format without sRGB for a
    /// window.
    pub target_format: wgpu::TextureFormat,
    pub cfg: RenderConfig,
    stats: Mutex<ReplacementStats>,
}

/// How many distinct pack replacements this device served, by production texture class.
#[derive(Clone, Debug, Default)]
pub struct ReplacementStats {
    served: HashMap<TextureClass, HashSet<String>>,
    /// `(name, reason)` for pack lookups that kept the original pixels (missing, corrupt, unusable).
    pub fallbacks: Vec<(String, String)>,
    pack_missing_logged: bool,
}

impl ReplacementStats {
    /// Distinct replacements served for `class`.
    pub fn count(&self, class: TextureClass) -> usize {
        self.served.get(&class).map(HashSet::len).unwrap_or(0)
    }
}

/// A configured window surface (see [`Gpu::for_surface`]).
pub struct GpuSurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// Reconfigure before the next acquire (the last texture was suboptimal; configuring while it
    /// is alive would panic).
    reconfigure: bool,
    /// The surface was lost: [`GpuSurface::recreate`] it.
    lost: bool,
}

impl Gpu {
    /// Creates a device without any window or surface.
    pub fn headless(cfg: &RenderConfig) -> Result<Gpu> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        desc.backends = preferred_backends();
        let instance = wgpu::Instance::new(desc);
        match Self::create(cfg, instance, None, COLOR_FORMAT) {
            Ok(g) => Ok(g),
            // No adapter on the preferred API (a CI machine without Vulkan, say): any backend will do
            // for offscreen work.
            Err(_) if std::env::var_os("WGPU_BACKEND").is_none() => {
                let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
                desc.backends = wgpu::Backends::all();
                Self::create(cfg, wgpu::Instance::new(desc), None, COLOR_FORMAT)
            }
            Err(e) => Err(e),
        }
    }

    /// Creates a device that can present to `window` (any `raw-window-handle` window, e.g.
    /// `Arc<winit::window::Window>`), and the surface configured at `width` x `height` native
    /// pixels.  Uses [`preferred_backends`].
    pub fn for_surface<W>(
        cfg: &RenderConfig,
        window: W,
        width: u32,
        height: u32,
    ) -> Result<(Gpu, GpuSurface)>
    where
        W: wgpu::DisplayAndWindowHandle + Clone + std::fmt::Debug + Send + Sync + 'static,
    {
        let mut desc =
            wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(window.clone()));
        desc.backends = preferred_backends();
        let instance = wgpu::Instance::new(desc);
        let surface = instance
            .create_surface(window)
            .map_err(|e| RenderError::Gpu(format!("surface creation failed: {e}")))?;
        // The adapter decides the surface format, the format decides the pipelines: pick the
        // adapter first, then build the device for that format.
        let adapter = Self::select_adapter(&instance, Some(&surface), cfg.software_adapter)
            .map_err(|e| RenderError::Gpu(format!("no GPU adapter for the window: {e}")))?;
        let caps = surface.get_capabilities(&adapter);
        let format = choose_surface_format(&caps.formats)
            .ok_or_else(|| RenderError::Gpu("the surface reports no formats".into()))?;
        // Drawing goes through the surface's non-sRGB view (display values, blended like the
        // original); pipelines and MSAA buffers use that display format.
        let display = format.remove_srgb_suffix();
        let gpu = Self::finish(cfg, instance, adapter, display)?;
        let present_mode = if cfg.vsync {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: if display != format {
                vec![display]
            } else {
                Vec::new()
            },
            ..surface
                .get_default_config(&gpu.adapter, width.max(1), height.max(1))
                .ok_or_else(|| RenderError::Gpu("surface not supported by the adapter".into()))?
        };
        surface.configure(&gpu.device, &config);
        Ok((
            gpu,
            GpuSurface {
                surface,
                config,
                reconfigure: false,
                lost: false,
            },
        ))
    }

    /// The production adapter choice: Hardware Rasterizer off asks for the software adapter first, and falls back
    /// to the GPU when there is none.
    fn select_adapter(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
        software: bool,
    ) -> std::result::Result<wgpu::Adapter, wgpu::RequestAdapterError> {
        let request = |fallback: bool| {
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: fallback,
                compatible_surface: surface,
                apply_limit_buckets: false,
            }))
        };
        match software.then(|| request(true)) {
            Some(Ok(a)) => Ok(a),
            other => {
                if other.is_some() {
                    log::warn!(
                        "Hardware Rasterizer is off but this machine has no software adapter: using the GPU"
                    );
                }
                request(false)
            }
        }
    }

    fn create(
        cfg: &RenderConfig,
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
        target_format: wgpu::TextureFormat,
    ) -> Result<Gpu> {
        let adapter = Self::select_adapter(&instance, surface, cfg.software_adapter)
            .map_err(|e| RenderError::Gpu(format!("no suitable GPU adapter: {e}")))?;
        Self::finish(cfg, instance, adapter, target_format)
    }

    fn finish(
        cfg: &RenderConfig,
        instance: wgpu::Instance,
        adapter: wgpu::Adapter,
        target_format: wgpu::TextureFormat,
    ) -> Result<Gpu> {
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("mc2-render device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits {
                // road/pavement tiles live in one texture array: up to 1024 distinct bitmaps
                max_texture_array_layers: adapter.limits().max_texture_array_layers.min(1024),
                ..wgpu::Limits::default()
            },
            ..Default::default()
        }))
        .map_err(|e| RenderError::Gpu(format!("device creation failed: {e}")))?;
        device.on_uncaptured_error(std::sync::Arc::new(|e: wgpu::Error| {
            eprintln!("[mc2-render] wgpu error: {e}");
        }));

        let mut samples = cfg.msaa.max(1);
        while samples > 1 {
            let c = adapter.get_texture_format_features(target_format).flags;
            let d = adapter.get_texture_format_features(DEPTH_FORMAT).flags;
            if c.sample_count_supported(samples) && d.sample_count_supported(samples) {
                break;
            }
            samples /= 2;
        }
        let gpu = Gpu {
            instance,
            adapter,
            device,
            queue,
            adapter_name: info.name.clone(),
            backend: format!("{:?}", info.backend),
            samples,
            target_format,
            cfg: cfg.clone(),
            stats: Mutex::new(ReplacementStats::default()),
        };
        Ok(gpu)
    }

    /// Whether this machine offers a software (CPU) adapter at all, by the same request the production selection makes.
    pub fn software_adapter_available() -> bool {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        Self::select_adapter_exact(&instance)
            .is_some_and(|a| a.get_info().device_type == wgpu::DeviceType::Cpu)
    }

    fn select_adapter_exact(instance: &wgpu::Instance) -> Option<wgpu::Adapter> {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: true,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .ok()
    }

    /// Whether the adapter actually in use is a software (CPU) rasterizer, whatever was asked for.
    pub fn is_software_adapter(&self) -> bool {
        self.adapter.get_info().device_type == wgpu::DeviceType::Cpu
    }

    /// The start-up line naming the adapter the device really is on (what a packaged run prints).
    pub fn startup_line(&self) -> String {
        let info = self.adapter.get_info();
        format!(
            "mc2: gpu adapter: {} ({:?}, {:?}, software={})",
            info.name,
            info.backend,
            info.device_type,
            self.is_software_adapter()
        )
    }

    fn warn_if_pack_missing(&self) {
        let Some(dir) = &self.cfg.hires_dir else {
            return;
        };
        if dir.is_dir() {
            return;
        }
        let mut stats = self.stats.lock().unwrap_or_else(|p| p.into_inner());
        if stats.pack_missing_logged {
            return;
        }
        stats.pack_missing_logged = true;
        drop(stats);
        let msg = format!(
            "[mc2-render] hi-res pack {} is missing: using original textures",
            dir.display()
        );
        log::warn!("{msg}");
        eprintln!("{msg}");
    }

    fn note_served(&self, name: &str) {
        let class = TextureClass::from_name(name);
        let mut stats = self.stats.lock().unwrap_or_else(|p| p.into_inner());
        stats
            .served
            .entry(class)
            .or_default()
            .insert(name.to_string());
    }

    fn note_fallback(&self, name: &str, reason: &str) {
        let mut stats = self.stats.lock().unwrap_or_else(|p| p.into_inner());
        stats.fallbacks.push((name.to_string(), reason.to_string()));
    }

    /// Replacements this device has served (distinct names per class).
    pub fn replacement_stats(&self) -> ReplacementStats {
        self.stats.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Uploads an RGBA8 image as a sampled texture with an optional CPU-built mip chain.
    ///
    /// `name` feeds the hi-res replacement lookup (`MC2_HIRES_DIR`, key
    /// `n_<sanitised name>_<w>x<h>`): a `<key>.png` there replaces the pixels (and the size).
    pub fn texture_2d(
        &self,
        name: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
        mips: bool,
    ) -> wgpu::Texture {
        let (pixels, w, h) = self.replacement(name, width, height, rgba);
        self.texture_2d_raw(name, w, h, &pixels, mips)
    }

    /// The hi-res replacement lookup alone (see [`apply_replacement`]), for callers that still
    /// have to process the pixels (paint schemes, alpha rules) before the upload.  A replacement
    /// larger than the device's 2D texture limit is ignored: the pack never decides whether the
    /// game can run.
    pub fn replacement(&self, name: &str, w: u32, h: u32, rgba: &[u8]) -> (Vec<u8>, u32, u32) {
        self.warn_if_pack_missing();
        match lookup_replacement(&self.cfg, name, w, h, rgba) {
            ReplacementHit::Original => (rgba.to_vec(), w, h),
            ReplacementHit::Fallback { reason } => {
                self.note_fallback(name, &reason);
                (rgba.to_vec(), w, h)
            }
            ReplacementHit::Served {
                rgba: pixels,
                w: rw,
                h: rh,
            } => {
                let max = self.device.limits().max_texture_dimension_2d;
                if (rw, rh) != (w, h) && (rw > max || rh > max) {
                    let reason = format!("{rw}x{rh}, over the device limit {max}");
                    eprintln!(
                        "[mc2-render] replacement for {name} is {reason}: using the original"
                    );
                    self.note_fallback(name, &reason);
                    return (rgba.to_vec(), w, h);
                }
                self.note_served(name);
                (pixels, rw, rh)
            }
        }
    }

    /// Uploads RGBA8 pixels as they are (no replacement lookup).
    pub fn texture_2d_raw(
        &self,
        label: &str,
        w: u32,
        h: u32,
        pixels: &[u8],
        mips: bool,
    ) -> wgpu::Texture {
        let levels = if mips && self.cfg.mipmaps {
            tx::mip_level_count(w, h)
        } else {
            1
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR_FORMAT,
            // COPY_SRC lets a test read an uploaded texture back (`tests/paint_upload.rs`)
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        self.write_levels(&texture, 0, pixels, w, h, levels);
        texture
    }

    /// Like [`Gpu::texture_array`], with the hi-res replacement lookup per layer.
    ///
    /// `names[i]` lists the names layer `i` is known by (identical bitmaps share a layer); the
    /// first name that has a `<key>.png` of the layer's size in `MC2_HIRES_DIR` wins.  Every layer
    /// of an array has one size, so the array is as large as the largest replacement (capped at
    /// the device limit) and the other layers are resampled up to it.  With no replacement at all
    /// this is `texture_array`.
    pub fn texture_array_named(
        &self,
        name: &str,
        size: u32,
        layers: &[Vec<u8>],
        names: &[Vec<String>],
    ) -> wgpu::Texture {
        if self.cfg.hires_dir.is_none() {
            return self.texture_array(name, size, layers);
        }
        self.warn_if_pack_missing();
        let max = self.device.limits().max_texture_dimension_2d;
        let mut found: Vec<Option<(Vec<u8>, u32)>> = Vec::with_capacity(layers.len());
        let mut target = size;
        for (i, layer) in layers.iter().enumerate() {
            let mut hit = None;
            for n in names.get(i).into_iter().flatten() {
                match lookup_replacement(&self.cfg, n, size, size, layer) {
                    ReplacementHit::Served { rgba: px, w, h } => {
                        if (w, h) != (size, size) {
                            // an array layer is square and within the device limit, or it is ignored
                            if w == h && w <= max {
                                self.note_served(n);
                                hit = Some((px, w));
                                break;
                            }
                            let reason = format!("{w}x{h}: not a square layer");
                            eprintln!(
                                "[mc2-render] replacement for {n} is {w}x{h}: using the original"
                            );
                            self.note_fallback(n, &reason);
                        } else {
                            // same size as the original layer: the pixels still come from the pack
                            self.note_served(n);
                            hit = Some((px, size));
                            break;
                        }
                    }
                    ReplacementHit::Fallback { reason } => self.note_fallback(n, &reason),
                    ReplacementHit::Original => {}
                }
            }
            if let Some((_, w)) = &hit {
                target = target.max(*w);
            }
            found.push(hit);
        }
        if found.iter().all(Option::is_none) {
            return self.texture_array(name, size, layers);
        }
        if target == size {
            let replaced: Vec<Vec<u8>> = layers
                .iter()
                .zip(&found)
                .map(|(layer, hit)| match hit {
                    Some((px, _)) => px.clone(),
                    None => layer.clone(),
                })
                .collect();
            return self.texture_array(name, size, &replaced);
        }
        let resized: Vec<Vec<u8>> = layers
            .iter()
            .zip(found)
            .map(|(layer, hit)| match hit {
                Some((px, w)) if w == target => px,
                Some((px, w)) => resample_bilinear(&px, w, w, target, target),
                None => resample_bilinear(layer, size, size, target, target),
            })
            .collect();
        self.texture_array(name, target, &resized)
    }

    /// 2D texture array (all layers `size` x `size`), each layer with its own mip chain.
    pub fn texture_array(&self, name: &str, size: u32, layers: &[Vec<u8>]) -> wgpu::Texture {
        let count = layers.len().max(1) as u32;
        let levels = if self.cfg.mipmaps {
            tx::mip_level_count(size, size)
        } else {
            1
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(name),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: count,
            },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        if layers.is_empty() {
            self.write_levels(
                &texture,
                0,
                &vec![0u8; (size * size * 4) as usize],
                size,
                size,
                levels,
            );
        }
        for (i, layer) in layers.iter().enumerate() {
            self.write_levels(&texture, i as u32, layer, size, size, levels);
        }
        texture
    }

    fn write_levels(
        &self,
        texture: &wgpu::Texture,
        layer: u32,
        pixels: &[u8],
        w: u32,
        h: u32,
        levels: u32,
    ) {
        let chain = tx::build_mip_chain(pixels.to_vec(), w, h, 4);
        for (level, (data, lw, lh)) in chain.iter().take(levels as usize).enumerate() {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(lw * 4),
                    rows_per_image: Some(*lh),
                },
                wgpu::Extent3d {
                    width: *lw,
                    height: *lh,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    pub fn buffer(&self, label: &str, usage: wgpu::BufferUsages, data: &[u8]) -> wgpu::Buffer {
        // Zero-sized buffers are not allowed; pad to a minimal size so empty lists still bind.
        let size = data.len().max(16).next_multiple_of(4) as u64;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if !data.is_empty() {
            self.queue.write_buffer(&buf, 0, data);
        }
        buf
    }
}

/// The surface format to present with: an 8-bit sRGB format first (the plan's on-screen format;
/// see [`RenderTarget`] for how display values reach it), then plain 8-bit, then anything
/// non-sRGB, then whatever the surface offers.
pub fn choose_surface_format(formats: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    use wgpu::TextureFormat as F;
    [
        F::Bgra8UnormSrgb,
        F::Rgba8UnormSrgb,
        F::Bgra8Unorm,
        F::Rgba8Unorm,
    ]
    .into_iter()
    .find(|f| formats.contains(f))
    .or_else(|| formats.iter().copied().find(|f| !f.is_srgb()))
    .or_else(|| formats.first().copied())
}

impl GpuSurface {
    /// Size in native pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// The configured surface format.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// Reconfigures for a new window size; zero sizes (minimised windows) are ignored.
    /// Returns whether the size changed.
    pub fn resize(&mut self, gpu: &Gpu, width: u32, height: u32) -> bool {
        if width == 0 || height == 0 || (width, height) == self.size() {
            return false;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&gpu.device, &self.config);
        true
    }

    /// The next texture to draw into, or `None` when this frame should be skipped (occluded,
    /// timed out, or the surface had to be reconfigured).
    /// A lost surface returns `None` until it is [`Self::recreate`]d (see [`Self::is_lost`]).
    pub fn acquire(&mut self, gpu: &Gpu) -> Option<wgpu::SurfaceTexture> {
        if self.lost {
            return None;
        }
        if std::mem::take(&mut self.reconfigure) {
            self.surface.configure(&gpu.device, &self.config);
        }
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => Some(t),
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                // Draw this one; reconfigure before the next acquire, once it is presented.
                self.reconfigure = true;
                Some(t)
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&gpu.device, &self.config);
                None
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.lost = true;
                None
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => None,
        }
    }

    /// Whether the surface was lost and must be [`Self::recreate`]d.
    pub fn is_lost(&self) -> bool {
        self.lost
    }

    /// Creates the surface again for `window` (after it was lost) with the same configuration.
    pub fn recreate<W>(&mut self, gpu: &Gpu, window: W) -> Result<()>
    where
        W: wgpu::DisplayAndWindowHandle + 'static,
    {
        let surface = gpu
            .instance
            .create_surface(window)
            .map_err(|e| RenderError::Gpu(format!("surface creation failed: {e}")))?;
        surface.configure(&gpu.device, &self.config);
        self.surface = surface;
        self.lost = false;
        self.reconfigure = false;
        Ok(())
    }

    /// The render target for an acquired surface texture: `view` in the surface's format,
    /// `display_view` without sRGB.
    pub fn target(
        &self,
        texture: &wgpu::SurfaceTexture,
        attachments: &FrameAttachments,
    ) -> RenderTarget {
        let display = self
            .config
            .view_formats
            .first()
            .copied()
            .unwrap_or(self.config.format);
        let view = texture.texture.create_view(&Default::default());
        let display_view = texture.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(display),
            ..Default::default()
        });
        RenderTarget::assemble(
            (view, self.config.format),
            (display_view, display),
            attachments,
            None,
        )
    }
}

/// The per-size attachments that outlive a frame: the multisampled colour buffer (when MSAA is
/// on, in the display format) and the depth buffer.  Keep one per window or offscreen target and
/// [`Self::resize`] it with them.
pub struct FrameAttachments {
    width: u32,
    height: u32,
    samples: u32,
    format: wgpu::TextureFormat,
    ms_view: Option<wgpu::TextureView>,
    depth_view: wgpu::TextureView,
}

impl FrameAttachments {
    pub fn new(gpu: &Gpu, width: u32, height: u32) -> Self {
        Self::with_format(gpu, width, height, gpu.target_format)
    }

    /// Attachments for colour targets of `format` (the MSAA buffer must match it).
    pub fn with_format(gpu: &Gpu, width: u32, height: u32, format: wgpu::TextureFormat) -> Self {
        let (width, height) = (width.max(1), height.max(1));
        let samples = gpu.samples;
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let ms_view = (samples > 1).then(|| {
            gpu.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("scene msaa"),
                    size,
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        });
        let depth_view = gpu
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("scene depth"),
                size,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());
        Self {
            width,
            height,
            samples,
            format,
            ms_view,
            depth_view,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Recreates the attachments when the size changed; returns whether anything changed.
    pub fn resize(&mut self, gpu: &Gpu, width: u32, height: u32) -> bool {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) == (self.width, self.height) && gpu.samples == self.samples {
            return false;
        }
        *self = Self::with_format(gpu, width, height, self.format);
        true
    }
}

/// Where a frame is drawn: what every drawing crate's `draw(gpu, encoder, target, ...)` takes.
///
/// `view` is the final single-sample colour view in `format`: `Rgba8Unorm` offscreen, the
/// surface's preferred format on screen (8-bit sRGB when it has one).  `display_view` is the same
/// texture viewed in `display_format`, the format without sRGB: drawing crates compute
/// display-referred colour, as the original's gamma-space renderer did, and draw (and blend)
/// through `display_view`, so every frame, blended pixels included, is byte-identical to the
/// `Rgba8Unorm` path.  Build pipelines for `(display_format, samples)` (a small cache per crate).
/// 3D passes use [`Self::color_attachment`] (the MSAA buffer resolving into `display_view`, or
/// `display_view` itself) with `depth_view`; 2D passes (HUD, screens, cursor) draw into
/// `display_view` single-sample without depth, after the scene.  (Should a surface ever lack a
/// non-sRGB view, `display_format` is sRGB and the pipelines decode in the shader, `SRGB_TARGET`.)
#[derive(Clone, Debug)]
pub struct RenderTarget {
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
    pub view: wgpu::TextureView,
    pub display_format: wgpu::TextureFormat,
    pub display_view: wgpu::TextureView,
    pub depth_view: wgpu::TextureView,
    pub samples: u32,
    /// The multisampled colour view (in `display_format`) when `samples > 1`.
    pub msaa_view: Option<wgpu::TextureView>,
    /// The texture behind the views, when it can be read back (offscreen targets).
    readback: Option<wgpu::Texture>,
}

impl RenderTarget {
    fn assemble(
        view: (wgpu::TextureView, wgpu::TextureFormat),
        display: (wgpu::TextureView, wgpu::TextureFormat),
        attachments: &FrameAttachments,
        readback: Option<wgpu::Texture>,
    ) -> Self {
        // A resolve needs matching formats: attachments made for another format are not used.
        let msaa_view = if attachments.format == display.1 {
            attachments.ms_view.clone()
        } else {
            None
        };
        Self {
            width: attachments.width,
            height: attachments.height,
            format: view.1,
            view: view.0,
            display_format: display.1,
            display_view: display.0,
            depth_view: attachments.depth_view.clone(),
            samples: if msaa_view.is_some() {
                attachments.samples
            } else {
                1
            },
            msaa_view,
            readback,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Colour attachment and resolve target of the scene pass.
    pub fn color_attachment(&self) -> (&wgpu::TextureView, Option<&wgpu::TextureView>) {
        match &self.msaa_view {
            Some(ms) => (ms, Some(&self.display_view)),
            None => (&self.display_view, None),
        }
    }

    /// The resolve view of 3D passes (`None` without MSAA).
    pub fn resolve_view(&self) -> Option<&wgpu::TextureView> {
        self.msaa_view.as_ref().map(|_| &self.display_view)
    }

    /// Whether [`Self::encode_readback`] works on this target.
    pub fn can_read_back(&self) -> bool {
        self.readback.is_some()
    }

    /// Records a copy of the image into a mappable buffer.  Submit `encoder` (once, with the rest
    /// of the frame), then call [`Readback::finish`].
    pub fn encode_readback(
        &self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<Readback> {
        let texture = self
            .readback
            .as_ref()
            .ok_or_else(|| RenderError::Other("this render target cannot be read back".into()))?;
        let (w, h) = (self.width as usize, self.height as usize);
        let unpadded = w * 4;
        let padded = unpadded.next_multiple_of(256);
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded as u32),
                    rows_per_image: Some(h as u32),
                },
            },
            wgpu::Extent3d {
                width: w as u32,
                height: h as u32,
                depth_or_array_layers: 1,
            },
        );
        Ok(Readback {
            buffer,
            width: w,
            height: h,
            padded,
            bgra: matches!(
                texture.format(),
                wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
            ),
        })
    }

    /// Convenience for tools: records the readback into `encoder`, submits it and returns the
    /// pixels (tightly packed RGBA8).
    pub fn read_rgba(&self, gpu: &Gpu, encoder: wgpu::CommandEncoder) -> Result<Vec<u8>> {
        let mut enc = encoder;
        let rb = self.encode_readback(gpu, &mut enc)?;
        gpu.queue.submit(Some(enc.finish()));
        rb.finish(gpu)
    }
}

/// A pending image copy from [`RenderTarget::encode_readback`].
pub struct Readback {
    buffer: wgpu::Buffer,
    width: usize,
    height: usize,
    padded: usize,
    bgra: bool,
}

impl Readback {
    /// Waits for the submitted copy and returns tightly packed RGBA8 (BGRA images swizzled).
    pub fn finish(self, gpu: &Gpu) -> Result<Vec<u8>> {
        let slice = self.buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RenderError::Gpu(format!("poll failed: {e}")))?;
        rx.recv()
            .map_err(|e| RenderError::Gpu(format!("map channel: {e}")))?
            .map_err(|e| RenderError::Gpu(format!("map failed: {e}")))?;
        let data = slice
            .get_mapped_range()
            .map_err(|e| RenderError::Gpu(format!("mapped range: {e}")))?;
        let unpadded = self.width * 4;
        let mut out = Vec::with_capacity(unpadded * self.height);
        for r in 0..self.height {
            out.extend_from_slice(&data[r * self.padded..r * self.padded + unpadded]);
        }
        drop(data);
        self.buffer.unmap();
        if self.bgra {
            for px in out.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }
        Ok(out)
    }
}

/// An offscreen frame: a readable colour texture, `Rgba8Unorm` (or [`Self::with_format`]), plus its
/// attachments.  Headless runs and screenshots draw here, on any device.
pub struct OffscreenTarget {
    texture: wgpu::Texture,
    format: wgpu::TextureFormat,
    attachments: FrameAttachments,
}

impl OffscreenTarget {
    pub fn new(gpu: &Gpu, width: u32, height: u32) -> Self {
        Self::with_format(gpu, width, height, COLOR_FORMAT)
    }

    /// An offscreen target in another 8-bit RGBA/BGRA format (e.g. `Rgba8UnormSrgb`, to check
    /// that an sRGB surface shows the same picture).
    pub fn with_format(gpu: &Gpu, width: u32, height: u32, format: wgpu::TextureFormat) -> Self {
        let display = format.remove_srgb_suffix();
        let attachments = FrameAttachments::with_format(gpu, width, height, display);
        let (width, height) = attachments.size();
        let extra = [display];
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen colour"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: if display != format { &extra } else { &[] },
        });
        Self {
            texture,
            format,
            attachments,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.attachments.size()
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Recreates the target for a new size; returns whether it changed.
    pub fn resize(&mut self, gpu: &Gpu, width: u32, height: u32) -> bool {
        if (width.max(1), height.max(1)) == self.size() && gpu.samples == self.attachments.samples {
            return false;
        }
        *self = Self::with_format(gpu, width, height, self.format);
        true
    }

    /// A sampleable view of the image in the display format (for [`crate::present::ScaledPresenter`]).
    pub fn sampled_view(&self) -> wgpu::TextureView {
        self.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.format.remove_srgb_suffix()),
            ..Default::default()
        })
    }

    /// The target of this frame.
    pub fn target(&self) -> RenderTarget {
        let display = self.format.remove_srgb_suffix();
        let view = self.texture.create_view(&Default::default());
        let display_view = self.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(display),
            ..Default::default()
        });
        RenderTarget::assemble(
            (view, self.format),
            (display_view, display),
            &self.attachments,
            Some(self.texture.clone()),
        )
    }
}

/// The clear value for a display-referred colour on a target of `format`: decoded to linear
/// light for sRGB formats (the hardware re-encodes it on store), unchanged otherwise.
pub fn clear_color(display: [f32; 3], alpha: f64, format: wgpu::TextureFormat) -> wgpu::Color {
    let f = |c: f32| -> f64 {
        let c = c as f64;
        if !format.is_srgb() {
            c
        } else if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    wgpu::Color {
        r: f(display[0]),
        g: f(display[1]),
        b: f(display[2]),
        a: alpha,
    }
}

/// Writes RGBA8 pixels as a PNG (through `mc2-gfx`'s writer).
pub fn write_png(path: &std::path::Path, rgba: &[u8], w: u32, h: u32) -> std::io::Result<()> {
    tx::write_png(path, rgba, w, h, mc2_gfx::MC2GFX_TEXFMT_RGBA8)
}

/// Bilinear resize of an RGBA8 image (clamped edges), for layers that share an array with a
/// larger replacement.
pub fn resample_bilinear(src: &[u8], w: u32, h: u32, nw: u32, nh: u32) -> Vec<u8> {
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    let fx = w as f32 / nw as f32;
    let fy = h as f32 / nh as f32;
    for y in 0..nh {
        let sy = ((y as f32 + 0.5) * fy - 0.5).clamp(0.0, (h - 1) as f32);
        let (y0, ty) = (sy.floor() as u32, sy.fract());
        let y1 = (y0 + 1).min(h - 1);
        for x in 0..nw {
            let sx = ((x as f32 + 0.5) * fx - 0.5).clamp(0.0, (w - 1) as f32);
            let (x0, tx_) = (sx.floor() as u32, sx.fract());
            let x1 = (x0 + 1).min(w - 1);
            for c in 0..4usize {
                let at = |xx: u32, yy: u32| src[((yy * w + xx) * 4) as usize + c] as f32;
                let top = at(x0, y0) * (1.0 - tx_) + at(x1, y0) * tx_;
                let bot = at(x0, y1) * (1.0 - tx_) + at(x1, y1) * tx_;
                out[((y * nw + x) * 4) as usize + c] =
                    (top * (1.0 - ty) + bot * ty + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// Outcome of one hi-res lookup: a replacement, a logged fallback, or the original pixels.
#[derive(Debug)]
pub enum ReplacementHit {
    Original,
    Served { rgba: Vec<u8>, w: u32, h: u32 },
    Fallback { reason: String },
}

impl ReplacementHit {
    fn pixels(self, original: &[u8], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
        match self {
            ReplacementHit::Served { rgba, w, h } => (rgba, w, h),
            ReplacementHit::Original | ReplacementHit::Fallback { .. } => (original.to_vec(), w, h),
        }
    }
}

/// The hi-res replacement hook (`MC2_HIRES_DIR`) and the optional original dump
/// (`MC2_HIRES_DUMP`), with the key scheme of `mc2-gfx` so one folder serves both renderers.
///
/// A missing or corrupt pack entry keeps the original pixels (never a black texture) and logs a
/// line naming the file when the pack directory exists.
pub fn lookup_replacement(
    cfg: &RenderConfig,
    name: &str,
    w: u32,
    h: u32,
    rgba: &[u8],
) -> ReplacementHit {
    let Some(dir) = &cfg.hires_dir else {
        return ReplacementHit::Original;
    };
    let key = tx::texture_key(Some(name), rgba, w, h, mc2_gfx::MC2GFX_TEXFMT_RGBA8);
    match tx::try_load_replacement(dir, &key) {
        tx::ReplacementLookup::Loaded(r) => ReplacementHit::Served {
            rgba: r.rgba,
            w: r.w,
            h: r.h,
        },
        tx::ReplacementLookup::Failed { reason, .. } => ReplacementHit::Fallback { reason },
        tx::ReplacementLookup::Missing => {
            if cfg.hires_dump {
                let p = dir.join("_dump").join(format!("{key}.png"));
                if !p.exists() {
                    if let Err(e) = tx::write_png(&p, rgba, w, h, mc2_gfx::MC2GFX_TEXFMT_RGBA8) {
                        eprintln!("[mc2-render] hires dump failed for {}: {e}", p.display());
                    }
                }
            }
            ReplacementHit::Fallback {
                reason: tx::MISSING_PACK_ENTRY.into(),
            }
        }
    }
}

/// Returns the pixels to upload and their size: the replacement's when `<key>.png` exists in the
/// folder and decodes, else the original.
pub fn apply_replacement(
    cfg: &RenderConfig,
    name: &str,
    w: u32,
    h: u32,
    rgba: &[u8],
) -> (Vec<u8>, u32, u32) {
    lookup_replacement(cfg, name, w, h, rgba).pixels(rgba, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let root = option_env!("CARGO_TARGET_TMPDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("target"));
        let d = root.join(format!("mc2_render_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// The start-up line names the adapter the device is really on, `software=` matching it, for either choice.
    #[test]
    fn the_startup_line_reports_the_actual_adapter() {
        for ask_software in [false, true] {
            let cfg = RenderConfig {
                software_adapter: ask_software,
                ..Default::default()
            };
            let Some(gpu) = headless_or_skip(&cfg) else {
                return;
            };
            let info = gpu.adapter.get_info();
            let expect = format!(
                "mc2: gpu adapter: {} ({:?}, {:?}, software={})",
                info.name,
                info.backend,
                info.device_type,
                info.device_type == wgpu::DeviceType::Cpu
            );
            assert_eq!(gpu.startup_line(), expect);
            if ask_software && Gpu::software_adapter_available() {
                assert!(gpu.startup_line().ends_with("software=true)"));
            }
            if !ask_software {
                assert_eq!(
                    gpu.is_software_adapter(),
                    info.device_type == wgpu::DeviceType::Cpu
                );
            }
        }
    }

    /// The GPU opt-out decision: only exactly `1` skips.
    fn skip_decision(var: Option<&std::ffi::OsStr>) -> bool {
        var.is_some_and(|v| v == "1")
    }

    /// Headless `Gpu`; with no adapter this fails the test unless `MC2_SKIP_GPU_TESTS` is `1`.
    fn headless_or_skip(cfg: &RenderConfig) -> Option<Gpu> {
        gpu_or_skip(
            Gpu::headless(cfg).map_err(|e| e.to_string()),
            skip_decision(std::env::var_os("MC2_SKIP_GPU_TESTS").as_deref()),
        )
    }

    fn gpu_or_skip(gpu: std::result::Result<Gpu, String>, skip: bool) -> Option<Gpu> {
        match gpu {
            Ok(g) => Some(g),
            Err(e) if skip && e.contains("adapter") => {
                eprintln!("no GPU adapter ({e}); MC2_SKIP_GPU_TESTS=1, skipping");
                None
            }
            Err(e) => panic!("no usable GPU adapter ({e}); set MC2_SKIP_GPU_TESTS=1 to skip"),
        }
    }

    #[test]
    fn gpu_opt_out_needs_exactly_one() {
        use std::ffi::OsStr;
        assert!(skip_decision(Some(OsStr::new("1"))));
        for v in ["", "0", "true", " 1", "11"] {
            assert!(!skip_decision(Some(OsStr::new(v))), "{v:?}");
        }
        assert!(!skip_decision(None));
        // no adapter: skips only when the decision says so
        assert!(gpu_or_skip(Err("no suitable GPU adapter".into()), true).is_none());
    }

    #[test]
    #[should_panic(expected = "no usable GPU adapter")]
    fn no_adapter_without_the_opt_out_fails() {
        let _ = gpu_or_skip(Err("no suitable GPU adapter".into()), false);
    }

    #[test]
    fn gpu_opt_out_does_not_hide_other_initialization_errors() {
        let panic = std::panic::catch_unwind(|| {
            gpu_or_skip(Err("device initialization failed".into()), true)
        });
        assert!(panic.is_err());
    }

    #[test]
    fn surface_format_prefers_8_bit_srgb() {
        use wgpu::TextureFormat as F;
        assert_eq!(
            choose_surface_format(&[F::Bgra8Unorm, F::Bgra8UnormSrgb]),
            Some(F::Bgra8UnormSrgb)
        );
        assert_eq!(
            choose_surface_format(&[F::Rgba16Float, F::Rgba8Unorm]),
            Some(F::Rgba8Unorm)
        );
        assert_eq!(
            choose_surface_format(&[F::Rgb10a2Unorm, F::Rgba16Float]),
            Some(F::Rgb10a2Unorm)
        );
        assert_eq!(choose_surface_format(&[]), None);
    }

    #[test]
    fn clear_colours_round_trip_through_srgb() {
        let c = [0.2f32, 0.5, 0.8];
        let u = clear_color(c, 1.0, wgpu::TextureFormat::Rgba8Unorm);
        assert_eq!(
            (u.r, u.g, u.b),
            (0.2f32 as f64, 0.5f32 as f64, 0.8f32 as f64)
        );
        let s = clear_color(c, 1.0, wgpu::TextureFormat::Bgra8UnormSrgb);
        // the store-side encode brings the decoded value back to the display value
        let enc = |l: f64| {
            if l <= 0.0031308 {
                l * 12.92
            } else {
                1.055 * l.powf(1.0 / 2.4) - 0.055
            }
        };
        for (lin, disp) in [(s.r, c[0]), (s.g, c[1]), (s.b, c[2])] {
            assert!((enc(lin) - disp as f64).abs() < 1e-6);
        }
    }

    #[test]
    fn preferred_backend_is_the_native_api() {
        if std::env::var_os("WGPU_BACKEND").is_some() {
            return;
        }
        let b = preferred_backends();
        if cfg!(target_os = "macos") {
            assert_eq!(b, wgpu::Backends::METAL);
        } else if cfg!(target_os = "windows") {
            assert!(b.contains(wgpu::Backends::DX12) && b.contains(wgpu::Backends::VULKAN));
        } else if cfg!(target_os = "linux") {
            assert_eq!(b, wgpu::Backends::VULKAN);
        }
    }

    #[test]
    fn replacement_hook_swaps_pixels_and_size() {
        let dir = temp_dir("hires");
        let cfg = RenderConfig {
            hires_dir: Some(dir.clone()),
            ..RenderConfig::default()
        };
        let original = vec![1u8, 2, 3, 255, 4, 5, 6, 255];
        // nothing in the folder: the original comes back untouched
        assert_eq!(
            apply_replacement(&cfg, "data/tgl/128/a.tga", 2, 1, &original),
            (original.clone(), 2, 1)
        );
        // a 1x2 replacement under the documented key wins
        let key = tx::texture_key(Some("data/tgl/128/a.tga"), &original, 2, 1, 0);
        assert_eq!(key, "n_data_tgl_128_a.tga_2x1");
        let hi = [9u8, 9, 9, 255, 8, 8, 8, 255];
        tx::write_png(&tx::replacement_path(&dir, &key), &hi, 1, 2, 0).unwrap();
        let (px, w, h) = apply_replacement(&cfg, "data/tgl/128/a.tga", 2, 1, &original);
        assert_eq!((w, h), (1, 2));
        assert_eq!(px, hi.to_vec());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_pack_entry_keeps_the_original_pixels_and_logs() {
        let dir = temp_dir("missing_key");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = RenderConfig {
            hires_dir: Some(dir.clone()),
            ..RenderConfig::default()
        };
        let original = vec![1u8, 2, 3, 255, 4, 5, 6, 255];
        let name = "data/tgl/128/a.tga";
        let key = tx::texture_key(Some(name), &original, 2, 1, 0);
        let path = tx::replacement_path(&dir, &key);
        tx::write_png(&path, &[9u8, 9, 9, 255, 8, 8, 8, 255], 1, 2, 0).unwrap();
        let (px, w, h) = apply_replacement(&cfg, name, 2, 1, &original);
        assert_eq!((w, h), (1, 2));
        assert_eq!(px, vec![9u8, 9, 9, 255, 8, 8, 8, 255]);
        std::fs::remove_file(&path).unwrap();

        let (hit, logs) = tx::with_fallback_log(|| lookup_replacement(&cfg, name, 2, 1, &original));
        match hit {
            ReplacementHit::Fallback { reason } => {
                assert_eq!(reason, tx::MISSING_PACK_ENTRY);
            }
            other => panic!("expected Fallback, got {other:?}"),
        }
        let (px, w, h) = apply_replacement(&cfg, name, 2, 1, &original);
        assert_eq!((w, h, px), (2, 1, original));
        assert!(
            logs.iter().any(|l| l.contains("missing")
                && l.contains("using the original")
                && l.contains(&path.display().to_string())),
            "expected a missing-entry log for {}, got {logs:?}",
            path.display()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_pack_entry_keeps_the_original_pixels() {
        let dir = temp_dir("corrupt");
        let cfg = RenderConfig {
            hires_dir: Some(dir.clone()),
            ..RenderConfig::default()
        };
        let original = vec![1u8, 2, 3, 255, 4, 5, 6, 255];
        let key = tx::texture_key(Some("data/tgl/128/a.tga"), &original, 2, 1, 0);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(tx::replacement_path(&dir, &key), b"not a png").unwrap();
        let (px, w, h) = apply_replacement(&cfg, "data/tgl/128/a.tga", 2, 1, &original);
        assert_eq!((w, h, px), (2, 1, original));
        match lookup_replacement(
            &cfg,
            "data/tgl/128/a.tga",
            2,
            1,
            &[1u8, 2, 3, 255, 4, 5, 6, 255],
        ) {
            ReplacementHit::Fallback { reason } => assert!(!reason.is_empty()),
            other => panic!("expected Fallback, got a served or original hit: {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bilinear_resize_keeps_flat_colour_and_doubles_the_size() {
        let src: Vec<u8> = (0..4).flat_map(|_| [10u8, 20, 30, 255]).collect();
        let big = resample_bilinear(&src, 2, 2, 4, 4);
        assert_eq!(big.len(), 4 * 4 * 4);
        assert!(big.chunks_exact(4).all(|p| p == [10, 20, 30, 255]));
        // identity size is a copy
        let ramp = [0u8, 0, 0, 255, 255, 255, 255, 255];
        assert_eq!(resample_bilinear(&ramp, 2, 1, 2, 1), ramp.to_vec());
    }

    #[test]
    fn named_tile_array_grows_to_the_largest_replacement() {
        let dir = temp_dir("tiles");
        let cfg = RenderConfig {
            hires_dir: Some(dir.clone()),
            ..RenderConfig::default()
        };
        let Some(gpu) = headless_or_skip(&cfg) else {
            return;
        };
        let layer = vec![50u8; 4 * 4 * 4];
        let layers = vec![layer.clone(), layer.clone()];
        let names = vec![
            vec!["a.tga".to_string(), "alias.tga".to_string()],
            vec!["b.tga".to_string()],
        ];
        // no replacement: the original size
        assert_eq!(gpu.texture_array_named("t", 4, &layers, &names).width(), 4);
        // the second name of layer 0 has a 16x16 replacement
        let key = tx::texture_key(Some("alias.tga"), &layer, 4, 4, 0);
        tx::write_png(
            &tx::replacement_path(&dir, &key),
            &vec![9u8; 16 * 16 * 4],
            16,
            16,
            0,
        )
        .unwrap();
        let t = gpu.texture_array_named("t", 4, &layers, &names);
        assert_eq!(
            (t.width(), t.height(), t.depth_or_array_layers()),
            (16, 16, 2)
        );
        // a non-square replacement is ignored
        let key = tx::texture_key(Some("b.tga"), &layer, 4, 4, 0);
        tx::write_png(
            &tx::replacement_path(&dir, &key),
            &vec![9u8; 32 * 8 * 4],
            32,
            8,
            0,
        )
        .unwrap();
        assert_eq!(gpu.texture_array_named("t", 4, &layers, &names).width(), 16);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_pack_entry_on_named_tile_array_keeps_original_and_logs() {
        let dir = temp_dir("tiles_missing");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = RenderConfig {
            hires_dir: Some(dir.clone()),
            ..RenderConfig::default()
        };
        let Some(gpu) = headless_or_skip(&cfg) else {
            return;
        };
        let layer = vec![50u8; 4 * 4 * 4];
        let layers = vec![layer.clone()];
        let names = vec![vec!["a.tga".to_string()]];
        let key = tx::texture_key(Some("a.tga"), &layer, 4, 4, 0);
        let path = tx::replacement_path(&dir, &key);
        tx::write_png(&path, &vec![9u8; 16 * 16 * 4], 16, 16, 0).unwrap();
        assert_eq!(gpu.texture_array_named("t", 4, &layers, &names).width(), 16);
        std::fs::remove_file(&path).unwrap();

        let (tex, logs) =
            tx::with_fallback_log(|| gpu.texture_array_named("t", 4, &layers, &names));
        assert_eq!(
            tex.width(),
            4,
            "missing tile replacement must keep original size"
        );
        let (px, w, h) = apply_replacement(&cfg, "a.tga", 4, 4, &layer);
        assert_eq!(
            (w, h, px.as_slice()),
            (4, 4, layer.as_slice()),
            "named tile-array lookup keeps original pixels"
        );
        match lookup_replacement(&cfg, "a.tga", 4, 4, &layer) {
            ReplacementHit::Fallback { reason } => {
                assert_eq!(reason, tx::MISSING_PACK_ENTRY);
            }
            other => panic!("expected Fallback, got {other:?}"),
        }
        assert!(
            gpu.replacement_stats()
                .fallbacks
                .iter()
                .any(|(n, r)| n == "a.tga" && r == tx::MISSING_PACK_ENTRY),
            "named tile-array route must record the missing fallback"
        );
        assert!(
            logs.iter().any(|l| l.contains("missing")
                && l.contains("using the original")
                && l.contains(&path.display().to_string())),
            "expected a missing-entry log on the named tile-array route for {}, got {logs:?}",
            path.display()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_replacement_falls_back_to_the_original() {
        let dir = temp_dir("big");
        let cfg = RenderConfig {
            hires_dir: Some(dir.clone()),
            ..RenderConfig::default()
        };
        let Some(gpu) = headless_or_skip(&cfg) else {
            return;
        };
        let max = gpu.device.limits().max_texture_dimension_2d;
        let original = vec![1u8; 2 * 2 * 4];
        let key = tx::texture_key(Some("huge.tga"), &original, 2, 2, 0);
        tx::write_png(
            &tx::replacement_path(&dir, &key),
            &vec![5u8; ((max + 1) * 2 * 4) as usize],
            max + 1,
            2,
            0,
        )
        .unwrap();
        let (px, w, h) = gpu.replacement("huge.tga", 2, 2, &original);
        assert_eq!((w, h, px), (2, 2, original));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dump_writes_the_original_once() {
        let dir = temp_dir("dump");
        let cfg = RenderConfig {
            hires_dir: Some(dir.clone()),
            hires_dump: true,
            ..RenderConfig::default()
        };
        let px = vec![7u8; 16];
        apply_replacement(&cfg, "x.tga", 2, 2, &px);
        let key = tx::texture_key(Some("x.tga"), &px, 2, 2, 0);
        assert!(dir.join("_dump").join(format!("{key}.png")).is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
