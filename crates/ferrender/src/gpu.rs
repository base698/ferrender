//! The 3D viewport on the GPU (Metal on macOS, Vulkan or GL on Linux).
//!
//! Bodies are drawn into an offscreen buffer holding each pixel's face
//! normal, plane and body. A second pass, inside egui's own render pass,
//! shades that and outlines the edges where faces turn, step or change body.

use std::sync::Arc;

use eframe::egui_wgpu::{self, wgpu};
use fr_core::Body;
use fr_core::render::{BODY, BODY_SELECTED, Camera, EDGE};
use wgpu::util::DeviceExt;

const SHADER: &str = r#"
struct U {
    right: vec4f,
    up: vec4f,
    depth: vec4f,
    light: vec4f,
    eye: vec4f,
    // Viewport origin in pixels, the selected body, and the edge width in pixels.
    misc: vec4f,
    body: vec4f,
    selected: vec4f,
    edge: vec4f,
    // The section plane (normal, offset) and, in x, whether it is on.
    section: vec4f,
    cut: vec4f,
};
@group(0) @binding(0) var<uniform> u: U;

struct V {
    @builtin(position) pos: vec4f,
    @location(0) @interpolate(flat) plane: vec4f,
    // The body's id, 1 where the triangle belongs to the selected face, and the
    // exact face it is part of (-1 for meshes, which have none).
    @location(1) @interpolate(flat) id: vec3f,
    @location(2) world: vec3f,
};

@vertex
fn vs_geom(@location(0) p: vec3f, @location(1) plane: vec4f, @location(2) id: vec3f) -> V {
    var o: V;
    o.pos = vec4f(dot(u.right.xyz, p) + u.right.w, dot(u.up.xyz, p) + u.up.w, dot(u.depth.xyz, p) + u.depth.w, 1.0);
    o.plane = plane;
    o.id = id;
    o.world = p;
    return o;
}

struct G {
    @location(0) plane: vec4f,
    @location(1) id: vec4f,
};

@fragment
fn fs_geom(v: V) -> G {
    if (u.cut.x > 0.5) {
        if (dot(u.section.xyz, v.world) > u.section.w) {
            discard;
        }
        // With the near side cut away, an inside-out surface means we are looking into
        // a solid: draw that as the flat cut face.
        if (dot(v.plane.xyz, u.eye.xyz) < 0.0) {
            return G(u.section, vec4f(v.id.x, 2.0, -2.0, 0.0));
        }
    }
    return G(v.plane, vec4f(v.id, 0.0));
}

@group(0) @binding(1) var t_plane: texture_2d<f32>;
@group(0) @binding(2) var t_id: texture_2d<f32>;

@vertex
fn vs_show(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    return vec4f(f32(i32(i & 1u) * 4 - 1), f32(i32(i >> 1u) * 4 - 1), 0.0, 1.0);
}

fn differ(pa: vec4f, ia: vec3f, at: vec2i, size: vec2i) -> bool {
    if (at.x < 0 || at.y < 0 || at.x >= size.x || at.y >= size.y) {
        return false;
    }
    let pb = textureLoad(t_plane, at, 0);
    let ib = textureLoad(t_id, at, 0).xyz;
    if (ia.x != ib.x || ia.y != ib.y) {
        return true;
    }
    if (ia.x == 0.0) {
        return false;
    }
    // Exact bodies are outlined where one face meets another, however gently.
    if (ia.z > -0.5 && ib.z > -0.5) {
        return ia.z != ib.z;
    }
    let d = dot(pa.xyz, pb.xyz);
    return d < 0.94 || (d > 0.9999 && abs(pa.w - pb.w) > 0.02);
}

@fragment
fn fs_show(@builtin(position) frag: vec4f) -> @location(0) vec4f {
    let size = vec2i(textureDimensions(t_plane));
    let at = vec2i(frag.xy - u.misc.xy);
    if (at.x < 0 || at.y < 0 || at.x >= size.x || at.y >= size.y) {
        discard;
    }
    let plane = textureLoad(t_plane, at, 0);
    let both = textureLoad(t_id, at, 0).xyz;
    let id = both.x;
    var edge = differ(plane, both, at + vec2i(1, 0), size) || differ(plane, both, at + vec2i(0, 1), size);
    if (u.misc.w > 1.5) {
        edge = edge || differ(plane, both, at - vec2i(1, 0), size) || differ(plane, both, at - vec2i(0, 1), size);
    }
    if (id == 0.0 && !edge) {
        discard;
    }
    var color = u.edge.rgb;
    var alpha = 0.85;
    if (id != 0.0) {
        var n = plane.xyz;
        if (dot(n, u.eye.xyz) < 0.0) {
            n = -n;
        }
        let shade = 0.5 + 0.5 * max(dot(n, u.light.xyz), 0.0);
        var base = u.body.rgb;
        if (id == u.misc.z || (both.y > 0.5 && both.y < 1.5)) {
            base = u.selected.rgb;
        }
        color = base * shade;
        if (both.y > 1.5) {
            // Cut faces are hatched, as on a drawing.
            let stripe = fract((frag.x + frag.y) / (7.0 * u.misc.w));
            color = mix(vec3f(0.93, 0.74, 0.47), vec3f(0.80, 0.55, 0.26), step(0.5, stripe));
        }
        alpha = 1.0;
        if (edge) {
            color = mix(color, u.edge.rgb, 0.85);
        }
    }
    return vec4f(color * alpha, alpha);
}
"#;

const PLANE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
const ID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const UNIFORM_FLOATS: usize = 44;

/// Floats per vertex: position, face normal and plane offset, body id, selected flag, exact face.
const STRIDE: usize = 10;

struct Targets {
    size: [u32; 2],
    plane: wgpu::TextureView,
    id: wgpu::TextureView,
    depth: wgpu::TextureView,
    show_bind: wgpu::BindGroup,
}

/// Lives in egui's callback resources for the life of the window.
pub struct Gpu {
    geom: wgpu::RenderPipeline,
    show: wgpu::RenderPipeline,
    show_layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    geom_bind: wgpu::BindGroup,
    targets: Option<Targets>,
    verts: Option<(wgpu::Buffer, u32)>,
    rev: u64,
}

impl Gpu {
    pub fn new(device: &wgpu::Device, target: wgpu::TextureFormat) -> Gpu {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("ferrender"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ferrender uniforms"),
            size: (UNIFORM_FLOATS * 4) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_entry = wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { multisampled: false, sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2 },
            count: None,
        };
        let geom_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("ferrender geometry"), entries: &[uniform_entry] });
        let show_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("ferrender show"), entries: &[uniform_entry, texture_entry(1), texture_entry(2)] });
        let geom_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ferrender geometry"),
            layout: &geom_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        let primitive = wgpu::PrimitiveState { cull_mode: None, ..Default::default() };
        let layout = |l: &wgpu::BindGroupLayout| device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(l)], immediate_size: 0 });
        let geom = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ferrender geometry"),
            layout: Some(&layout(&geom_layout)),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_geom"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: (STRIDE * 4) as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4, 2 => Float32x3],
                })],
                compilation_options: Default::default(),
            },
            primitive,
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_geom"),
                targets: &[Some(PLANE_FORMAT.into()), Some(ID_FORMAT.into())],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let show = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ferrender show"),
            layout: Some(&layout(&show_layout)),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_show"), buffers: &[], compilation_options: Default::default() },
            primitive,
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_show"),
                targets: &[Some(wgpu::ColorTargetState { format: target, blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        Gpu { geom, show, show_layout, uniforms, geom_bind, targets: None, verts: None, rev: 0 }
    }

    fn resize(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let view = |format, label| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let (plane, id, depth) = (view(PLANE_FORMAT, "ferrender planes"), view(ID_FORMAT, "ferrender ids"), view(DEPTH_FORMAT, "ferrender depth"));
        let show_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ferrender show"),
            layout: &self.show_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&plane) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&id) },
            ],
        });
        self.targets = Some(Targets { size, plane, id, depth, show_bind });
    }
}

/// Vertex data for a set of bodies. `face` marks one body's selected triangles.
pub fn vertices<'a>(bodies: impl IntoIterator<Item = &'a Body>, face: Option<(u32, &[usize])>) -> Vec<f32> {
    let mut out = Vec::new();
    for b in bodies {
        let picked = face.filter(|f| f.0 == b.id).map_or(&[][..], |f| f.1);
        let groups = b.mesh.groups();
        for (i, t) in b.mesh.tris.iter().enumerate() {
            let n = b.mesh.normal(i);
            let group = groups.as_ref().map_or(-1.0, |g| g[i] as f32);
            let w = n.dot(t[0]);
            let sel = if picked.binary_search(&i).is_ok() { 1.0 } else { 0.0 };
            for v in t {
                out.extend([v.x as f32, v.y as f32, v.z as f32, n.x as f32, n.y as f32, n.z as f32, w as f32, b.id as f32, sel, group]);
            }
        }
    }
    out
}

/// One frame of the viewport, handed to egui as a paint callback.
pub struct Frame {
    /// Changes when `verts` does.
    pub rev: u64,
    pub verts: Arc<Vec<f32>>,
    pub cam: Camera,
    /// Top-left corner and size of the viewport in physical pixels.
    pub origin: [f32; 2],
    pub size: [u32; 2],
    pub selected: Option<u32>,
    pub pixels_per_point: f32,
    /// How far the scene reaches from the camera target, in millimetres.
    pub reach: f64,
    /// Hide everything where `normal . p > offset`.
    pub section: Option<(glam::DVec3, f64)>,
}

impl Frame {
    fn uniforms(&self) -> [f32; UNIFORM_FLOATS] {
        let (eye, right, up) = self.cam.basis();
        // Camera scale is in points per millimetre; clip space spans the viewport.
        let sx = self.cam.scale * self.pixels_per_point as f64 * 2.0 / self.size[0] as f64;
        let sy = self.cam.scale * self.pixels_per_point as f64 * 2.0 / self.size[1] as f64;
        let sz = -0.5 / self.reach;
        let row = |v: glam::DVec3, s: f64, bias: f64| [(v.x * s) as f32, (v.y * s) as f32, (v.z * s) as f32, (bias - v.dot(self.cam.target) * s) as f32];
        let light = (eye + right * 0.35 + up * 0.55).normalize();
        let rgb = |c: [u8; 3]| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0];
        let mut u = [0.0; UNIFORM_FLOATS];
        let rows = [
            row(right, sx, 0.0),
            row(up, sy, 0.0),
            row(eye, sz, 0.5),
            [light.x as f32, light.y as f32, light.z as f32, 0.0],
            [eye.x as f32, eye.y as f32, eye.z as f32, 0.0],
            [self.origin[0], self.origin[1], self.selected.map_or(-1.0, |s| s as f32), self.pixels_per_point],
            rgb(BODY),
            rgb(BODY_SELECTED),
            rgb(EDGE),
            self.section.map_or([0.0; 4], |(n, w)| [n.x as f32, n.y as f32, n.z as f32, w as f32]),
            [if self.section.is_some() { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
        ];
        for (i, r) in rows.iter().enumerate() {
            u[i * 4..i * 4 + 4].copy_from_slice(r);
        }
        u
    }
}

fn bytes(f: &[f32]) -> Vec<u8> {
    f.iter().flat_map(|v| v.to_le_bytes()).collect()
}

impl egui_wgpu::CallbackTrait for Frame {
    fn prepare(&self, device: &wgpu::Device, queue: &wgpu::Queue, _screen: &egui_wgpu::ScreenDescriptor, encoder: &mut wgpu::CommandEncoder, resources: &mut egui_wgpu::CallbackResources) -> Vec<wgpu::CommandBuffer> {
        let Some(gpu) = resources.get_mut::<Gpu>() else { return Vec::new() };
        if self.size[0] == 0 || self.size[1] == 0 {
            return Vec::new();
        }
        gpu.resize(device, self.size);
        if gpu.rev != self.rev {
            gpu.rev = self.rev;
            gpu.verts = (!self.verts.is_empty()).then(|| {
                let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("ferrender bodies"), contents: &bytes(&self.verts), usage: wgpu::BufferUsages::VERTEX });
                (buf, (self.verts.len() / STRIDE) as u32)
            });
        }
        queue.write_buffer(&gpu.uniforms, 0, &bytes(&self.uniforms()));
        let t = gpu.targets.as_ref().unwrap();
        let clear = wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ferrender geometry"),
            color_attachments: &[
                Some(wgpu::RenderPassColorAttachment { view: &t.plane, resolve_target: None, ops: clear, depth_slice: None }),
                Some(wgpu::RenderPassColorAttachment { view: &t.id, resolve_target: None, ops: clear, depth_slice: None }),
            ],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &t.depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        if let Some((buf, count)) = &gpu.verts {
            pass.set_pipeline(&gpu.geom);
            pass.set_bind_group(0, &gpu.geom_bind, &[]);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..*count, 0..1);
        }
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        let Some(gpu) = resources.get::<Gpu>() else { return };
        let Some(t) = &gpu.targets else { return };
        pass.set_pipeline(&gpu.show);
        pass.set_bind_group(0, &t.show_bind, &[]);
        pass.draw(0..3, 0..1);
    }
}
