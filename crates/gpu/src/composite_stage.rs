//! Ordered operations on the lower-track accumulator, preserving replacement semantics.
use super::*;
use filmcraft_geom::Affine;
use filmcraft_render::plan::LayerFx;

/// Three RGBA32 working textures (the largest effect job) plus the RGBA16 original.
/// This admission bound covers one adjustment's additional canvas storage, not total GPU RSS.
const ADJUSTMENT_WORKING_BYTES: u64 = 512 * 1024 * 1024;

pub(super) fn adjustment_fits(size: (u32, u32)) -> bool {
    size.0 != 0
        && size.1 != 0
        && u64::from(size.0).checked_mul(u64::from(size.1)).and_then(|pixels| pixels.checked_mul(56)).is_some_and(|bytes| bytes <= ADJUSTMENT_WORKING_BYTES)
}

pub(super) fn adjustment_pipeline(device: &wgpu::Device, shader: &wgpu::ShaderModule, layout: &wgpu::PipelineLayout) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("adjustment-replace"),
        layout: Some(layout),
        vertex: wgpu::VertexState { module: shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_adjustment"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format: ACCUM_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

impl GpuCompositor {
    pub(super) fn adjust_composite(&mut self, canvas: &wgpu::TextureView, dims: (u32, u32), fx: &LayerFx, opacity: f32) -> bool {
        let original = Self::target(&self.device, &mut self.backdrop, dims.0, dims.1, ACCUM_FORMAT, wgpu::TextureUsages::COPY_DST);
        let src = SrcInfo { kind: 1, code_scale: 1.0, code_levels: 1.0, alpha: 0.0, chroma: dims, size: dims, color: Default::default(), input: None };
        let mut uniforms = Self::uniforms(&src, &Affine::IDENTITY, 1.0, Blend::Normal, dims);
        uniforms[26] = 1.0;
        let source_buffer = self.uniform_buffer(&uniforms);
        let source = self.texture_binding("adjustment-source", &source_buffer, &original, None);
        let Some(stage) = self.fx.as_mut() else { return false };
        let Some(job) = stage.job(&self.device, &self.queue, fx, source, &self.dummy) else { return false };
        let uniform_buffer = self.uniform_buffer(&Self::uniforms(&src, &Affine::IDENTITY, opacity, Blend::Normal, dims));
        let replace = self.texture_binding("adjustment-replace", &uniform_buffer, &job.result, Some(&original));
        let (Some(accum), Some(backdrop)) = (&self.accum, &self.backdrop) else { return false };
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("adjustment-composite") });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { texture: &accum.0, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyTextureInfo { texture: &backdrop.0, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::Extent3d { width: dims.0, height: dims.1, depth_or_array_layers: 1 },
        );
        if let Some(stage) = &self.fx {
            stage.record(&mut encoder, &job);
        } else {
            return false;
        }
        {
            let mut pass = accum_pass(&mut encoder, canvas, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
            pass.set_pipeline(&self.adjustment_pipeline);
            pass.set_bind_group(0, &replace, &[]);
            pass.draw(0..6, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        self.gpu_adjustments = self.gpu_adjustments.saturating_add(1);
        true
    }

    fn texture_binding(
        &self,
        label: &'static str,
        uniforms: &wgpu::Buffer,
        texture: &wgpu::TextureView,
        backdrop: Option<&wgpu::TextureView>,
    ) -> wgpu::BindGroup {
        let mut entries = vec![
            wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(texture) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.dummy) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.dummy) },
            wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&self.dummy) },
        ];
        if let Some(backdrop) = backdrop {
            entries.push(wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(backdrop) });
        }
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: if backdrop.is_some() { &self.blend_bgl } else { &self.layer_bgl },
            entries: &entries,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_render::gpufx::FxOp;

    #[test]
    fn adjustment_canvases_have_a_checked_storage_budget() {
        assert!(adjustment_fits((1920, 1080)));
        assert!(adjustment_fits((3840, 2160)));
        for size in [(0, 1), (1, 0), (7680, 4320), (u32::MAX, u32::MAX)] {
            assert!(!adjustment_fits(size), "{size:?}");
        }
    }

    #[test]
    fn ordered_adjustments_replace_transparent_pixels_and_match_prepared_cpu_reference() {
        let Some((device, queue)) = crate::blend_tests::device() else {
            eprintln!("no adapter; adjustment GPU qualification skipped");
            return;
        };
        let mut gpu = GpuCompositor::new(&device, &queue);
        if gpu.fx.is_none() {
            eprintln!("GPU effect stage unavailable; skipping adjustment GPU qualification");
            return;
        }
        let (width, height) = (32usize, 16usize);
        let layer = |w: u32, h: u32, color: [f32; 4], matrix| {
            PlanLayer::new(Arc::new(VideoFrame::rgba_f32(w, h, (0..w * h).flat_map(|_| color).collect())), matrix, 1.0, Blend::Normal)
        };
        for opacity in [0.0, 0.375, 1.0] {
            let plan = FramePlan::Composite {
                width,
                height,
                steps: vec![
                    PlanStep::Layers(vec![layer(32, 16, [0.5, 0.25, 0.125, 0.75], Affine::IDENTITY)]),
                    PlanStep::Adjustment {
                        fx: LayerFx {
                            size: (32, 16),
                            decimation: 1,
                            ops: vec![
                                FxOp::Invert { channel: 0, blend: 0.5 },
                                FxOp::Noise { amount: 0.03, grain: 1.0, colored: false, clip: true, seed: 123456789 },
                            ],
                        },
                        opacity: 0.5,
                    },
                    PlanStep::Layers(vec![layer(8, 8, [0.0, 0.25, 0.5, 0.5], Affine::translate(8.0, 4.0))]),
                    PlanStep::Adjustment {
                        fx: LayerFx { size: (32, 16), decimation: 1, ops: vec![FxOp::Crop { x0: 8.0, x1: 24.0, y0: 0.0, y1: 16.0, feather: 0.0 }] },
                        opacity,
                    },
                    PlanStep::Layers(vec![layer(2, 2, [0.5, 0.5, 0.0, 0.5], Affine::IDENTITY)]),
                ],
            };
            let cpu = filmcraft_render::plan::execute_cpu(&plan);
            gpu.composite(&plan);
            assert!(!gpu.fx.as_ref().unwrap().is_idle(), "the final resolve must retain this frame's effect textures for reuse");
            let (w, h, actual) = gpu.read_accumulator().unwrap();
            assert_eq!((w as usize, h as usize), (width, height));
            let error = actual.iter().zip(&cpu.px).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            assert!(error < 0.004, "opacity={opacity}: error={error}");
            if opacity == 1.0 {
                assert_eq!(actual[(3 * width + 3) * 4 + 3], 0.0, "cleared pixels must not retain the original composite");
            }
            let prepared = prepare(&plan);
            assert!(prepared.bytes() > 0);
            gpu.composite_prepared(&plan, Some(&prepared));
            assert_eq!(gpu.read_accumulator().unwrap().2, actual, "prepared child alignment");
            // Unsupported devices preserve the complete CPU picture rather than skipping effects.
            let stage = gpu.fx.take();
            gpu.composite(&plan);
            let fallback = gpu.read_accumulator().unwrap().2;
            let error = fallback.iter().zip(&cpu.px).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            assert!(error < 0.004, "CPU fallback error={error}");
            gpu.fx = stage;
        }
        assert_eq!(gpu.gpu_adjustments, 12, "two adjustments in inline/prepared plans for three opacity cases; CPU fallbacks do not count");
        gpu.composite(&FramePlan::Layers { width, height, layers: Vec::new() });
        assert!(gpu.fx.as_ref().unwrap().is_idle(), "an effects-free frame releases unused working textures");
        eprintln!("ordered adjustment GPU parity executed: alpha replacement, order, opacity, prepared inputs and CPU fallback passed");
    }
}
