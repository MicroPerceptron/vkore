//! WASI-P2 component entry point for a bounded, offscreen WebGPU canvas.
//! The host owns the device and target. The component gets a WASI-P2
//! context without preopens, network access, or inherited process state.

use rustix::fs::{Mode, OFlags};
use std::{fs::File, io::Read, path::Path, sync::mpsc, time::Duration};

use wasmtime::{
    Config, Engine, Store, StoreContextMut, StoreLimits, StoreLimitsBuilder,
    component::{Component, Linker, ResourceTable, TypedFunc},
};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};
use wgpu as gpu;

pub const WIDTH: u32 = 512;
pub const HEIGHT: u32 = 288;
const SIZE: gpu::Extent3d = gpu::Extent3d {
    width: WIDTH,
    height: HEIGHT,
    depth_or_array_layers: 1,
};
const BYTES_PER_ROW: u32 = WIDTH * 4;
const IMAGE_BYTES: u64 = (BYTES_PER_ROW * HEIGHT) as u64;
const COMPONENT_LIMIT: u64 = 4 * 1024 * 1024;
const SHADER_LIMIT: usize = 16 * 1024;
const CALL_FUEL: u64 = 1_000_000;
const GPU_TIMEOUT: Duration = Duration::from_secs(2);
const FORMAT: gpu::TextureFormat = gpu::TextureFormat::Rgba8UnormSrgb;

struct CanvasContext {
    wasi: WasiCtx,
    resources: ResourceTable,
    limits: StoreLimits,
    phase: Option<f32>,
}

impl WasiView for CanvasContext {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.resources,
        }
    }
}

pub struct Canvas {
    store: Store<CanvasContext>,
    render: TypedFunc<(f32,), ()>,
    gpu: Gpu,
}

impl Canvas {
    pub fn open(path: &Path) -> wasmtime::Result<Self> {
        let source = File::from(rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NONBLOCK,
            Mode::empty(),
        )?);
        wasmtime::ensure!(source.metadata()?.is_file(), "expected a component file");
        let mut bytes = Vec::new();
        source.take(COMPONENT_LIMIT + 1).read_to_end(&mut bytes)?;
        wasmtime::ensure!(
            bytes.len() as u64 <= COMPONENT_LIMIT,
            "component exceeds 4 MiB"
        );

        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config)?;
        let component = Component::from_binary(&engine, &bytes)?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        linker.root().func_wrap(
            "draw",
            |mut store: StoreContextMut<'_, CanvasContext>, (phase,): (f32,)| {
                wasmtime::ensure!(phase.is_finite(), "invalid animation uniform");
                wasmtime::ensure!(
                    store.data().phase.is_none(),
                    "only one draw is allowed per frame"
                );
                store.data_mut().phase = Some(phase);
                Ok(())
            },
        )?;
        let mut store = Store::new(
            &engine,
            CanvasContext {
                wasi: WasiCtx::builder().allow_tcp(false).allow_udp(false).build(),
                resources: ResourceTable::new(),
                limits: StoreLimitsBuilder::new()
                    .memory_size(4 * 1024 * 1024)
                    .table_elements(4096)
                    .instances(16)
                    .memories(4)
                    .tables(4)
                    .build(),
                phase: None,
            },
        );
        store.limiter(|context| &mut context.limits);
        store.set_fuel(CALL_FUEL)?;
        let instance = linker.instantiate(&mut store, &component)?;
        let source = instance.get_typed_func::<(), (String,)>(&mut store, "shader-source")?;
        let render = instance.get_typed_func::<(f32,), ()>(&mut store, "render")?;
        store.set_fuel(CALL_FUEL)?;
        let (shader,) = source.call(&mut store, ())?;
        wasmtime::ensure!(shader.len() <= SHADER_LIMIT, "WGSL source exceeds 16 KiB");
        let gpu = pollster::block_on(Gpu::new(shader))?;
        Ok(Self { store, render, gpu })
    }

    pub fn adapter(&self) -> &str {
        &self.gpu.adapter
    }

    pub fn frame(&mut self, elapsed: Duration) -> wasmtime::Result<Vec<u8>> {
        self.store.data_mut().phase = None;
        self.store.set_fuel(CALL_FUEL)?;
        self.render
            .call(&mut self.store, (elapsed.as_secs_f32(),))?;
        let phase = self
            .store
            .data()
            .phase
            .ok_or_else(|| wasmtime::format_err!("component returned without drawing"))?;
        self.gpu.draw(phase)
    }
}

struct Gpu {
    adapter: String,
    device: gpu::Device,
    queue: gpu::Queue,
    pipeline: gpu::RenderPipeline,
    uniforms: gpu::Buffer,
    bind_group: gpu::BindGroup,
    image: gpu::Texture,
    view: gpu::TextureView,
    readback: gpu::Buffer,
}

impl Gpu {
    async fn new(wgsl: String) -> wasmtime::Result<Self> {
        let instance = gpu::Instance::new(gpu::InstanceDescriptor {
            backends: if cfg!(target_os = "macos") {
                gpu::Backends::METAL
            } else {
                gpu::Backends::VULKAN
            },
            ..gpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&gpu::RequestAdapterOptions::default())
            .await?;
        let info = adapter.get_info();
        let adapter_name = format!(
            "{} · {:?} · {:?}",
            info.name, info.backend, info.device_type
        );
        let (device, queue) = adapter
            .request_device(&gpu::DeviceDescriptor {
                required_limits: gpu::Limits::downlevel_defaults(),
                ..Default::default()
            })
            .await?;
        let validation = device.push_error_scope(gpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(gpu::ShaderModuleDescriptor {
            label: Some("Canvas plugin"),
            source: gpu::ShaderSource::Wgsl(wgsl.into()),
        });
        let pipeline = device.create_render_pipeline(&gpu::RenderPipelineDescriptor {
            label: Some("Canvas plugin"),
            layout: None,
            vertex: gpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(gpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        if let Some(error) = validation.pop().await {
            wasmtime::bail!("canvas shader: {error}");
        }
        let uniforms = device.create_buffer(&gpu::BufferDescriptor {
            label: Some("Canvas phase"),
            size: 16,
            usage: gpu::BufferUsages::UNIFORM | gpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let validation = device.push_error_scope(gpu::ErrorFilter::Validation);
        let bind_group = device.create_bind_group(&gpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[gpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        if let Some(error) = validation.pop().await {
            wasmtime::bail!("canvas shader bindings: {error}");
        }
        let image = device.create_texture(&gpu::TextureDescriptor {
            label: Some("Canvas render target"),
            size: SIZE,
            mip_level_count: 1,
            sample_count: 1,
            dimension: gpu::TextureDimension::D2,
            format: FORMAT,
            usage: gpu::TextureUsages::RENDER_ATTACHMENT | gpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = image.create_view(&gpu::TextureViewDescriptor::default());
        let readback = device.create_buffer(&gpu::BufferDescriptor {
            label: Some("Canvas readback"),
            size: IMAGE_BYTES,
            usage: gpu::BufferUsages::COPY_DST | gpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Ok(Self {
            adapter: adapter_name,
            device,
            queue,
            pipeline,
            uniforms,
            bind_group,
            image,
            view,
            readback,
        })
    }

    fn draw(&self, phase: f32) -> wasmtime::Result<Vec<u8>> {
        self.queue
            .write_buffer(&self.uniforms, 0, &phase.to_le_bytes());
        let mut encoder = self
            .device
            .create_command_encoder(&gpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&gpu::RenderPassDescriptor {
                color_attachments: &[Some(gpu::RenderPassColorAttachment {
                    view: &self.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: gpu::Operations {
                        load: gpu::LoadOp::Clear(gpu::Color::BLACK),
                        store: gpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            self.image.as_image_copy(),
            gpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: gpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(BYTES_PER_ROW),
                    rows_per_image: Some(HEIGHT),
                },
            },
            SIZE,
        );
        let submission = self.queue.submit([encoder.finish()]);
        let slice = self.readback.slice(..);
        let (sender, receiver) = mpsc::channel();
        slice.map_async(gpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device.poll(gpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(GPU_TIMEOUT),
        })?;
        receiver.recv_timeout(GPU_TIMEOUT)??;
        let pixels = slice.get_mapped_range()?.to_vec();
        self.readback.unmap();
        Ok(pixels)
    }
}
