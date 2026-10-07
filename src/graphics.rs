use crate::{app::UserEvent, model::Terminal, view::TerminalView};
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use winit::{dpi::PhysicalSize, event_loop::EventLoopProxy, window::Window};

pub enum Frame {
    Presented,
    Retry,
    Hidden,
}

pub struct Graphics {
    pub window: Arc<Window>,
    pub context: egui::Context,
    pub input: egui_winit::State,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: egui_wgpu::Renderer,
    size: PhysicalSize<u32>,
    timer: Option<GpuTimer>,
    profile: bool,
    frame_number: u64,
}

impl Graphics {
    pub async fn new(
        window: Arc<Window>,
        proxy: EventLoopProxy<UserEvent>,
        profile: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await?;
        let features = if profile {
            adapter.features() & wgpu::Features::TIMESTAMP_QUERY
        } else {
            wgpu::Features::empty()
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("terminal"),
                required_features: features,
                ..Default::default()
            })
            .await?;
        let error_proxy = proxy.clone();
        device.on_uncaptured_error(Arc::new(move |error| {
            let _ = error_proxy.send_event(UserEvent::GpuError(error.to_string()));
        }));
        let lost_proxy = proxy.clone();
        device.set_device_lost_callback(move |reason, message| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                let _ =
                    lost_proxy.send_event(UserEvent::GpuError(format!("device lost: {message}")));
            }
        });
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface has no supported configuration")?;
        // egui's colors are gamma encoded; prefer an unorm surface when available.
        if let Some(format) = surface
            .get_capabilities(&adapter)
            .formats
            .into_iter()
            .find(|format| !format.is_srgb())
        {
            config.format = format;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
        if size.width != 0 && size.height != 0 {
            surface.configure(&device, &config);
        }
        let context = egui::Context::default();
        crate::view::install_fonts(&context);
        context.options_mut(|options| options.zoom_with_keyboard = false);
        context.set_request_repaint_callback(move |request| {
            if let Some(deadline) = Instant::now().checked_add(request.delay) {
                let _ = proxy.send_event(UserEvent::Repaint(deadline));
            }
        });
        let input = egui_winit::State::new(
            context.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let renderer = egui_wgpu::Renderer::new(
            &device,
            config.format,
            egui_wgpu::RendererOptions::default(),
        );
        let timer = features
            .contains(wgpu::Features::TIMESTAMP_QUERY)
            .then(|| GpuTimer::new(&device));
        if profile {
            eprintln!(
                "adapter={:?}; GPU timestamps={}",
                adapter.get_info(),
                timer.is_some()
            );
        }
        Ok(Self {
            window,
            context,
            input,
            surface,
            device,
            queue,
            config,
            renderer,
            size,
            timer,
            profile,
            frame_number: 0,
        })
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        self.size = size;
        if size.width != 0 && size.height != 0 {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    pub fn is_zero_sized(&self) -> bool {
        self.size.width == 0 || self.size.height == 0
    }

    pub fn draw(
        &mut self,
        terminal: &mut Terminal,
        view: &mut TerminalView,
        preedit: &str,
        cursor_on: bool,
        demo: bool,
        input_started: Option<Instant>,
    ) -> Result<(Frame, Option<(u16, u16)>), String> {
        if self.is_zero_sized() {
            return Ok((Frame::Hidden, None));
        }
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.resize(self.window.inner_size());
                return Ok((Frame::Retry, None));
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok((Frame::Retry, None)),
            Err(error) => return Err(format!("cannot acquire surface: {error}")),
        };
        let started = Instant::now();
        self.frame_number += 1;
        let _ = self.device.poll(wgpu::PollType::Poll);
        if let Some(timer) = &mut self.timer
            && let Some((frame, elapsed)) = timer.collect(self.queue.get_timestamp_period())
        {
            eprintln!("frame={frame} gpu_render_us={}", elapsed.as_micros());
        }
        let mut raw = self.input.take_egui_input(&self.window);
        if view.focused {
            // The terminal handled these from winit already. In particular,
            // Tab must go to the shell rather than move egui keyboard focus.
            raw.events.retain(|event| {
                !matches!(
                    event,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::Ime(_)
                        | egui::Event::Copy
                        | egui::Event::Cut
                        | egui::Event::Paste(_)
                )
            });
        }
        let mut resized = None;
        let output = self.context.run(raw, |ctx| {
            resized = view.show(ctx, terminal, preedit, cursor_on, demo);
        });
        self.input
            .handle_platform_output(&self.window, output.platform_output);
        let jobs = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.size.width, self.size.height],
            pixels_per_point: output.pixels_per_point,
        };
        for (id, delta) in &output.textures_delta.set {
            self.renderer
                .update_texture(&self.device, &self.queue, *id, delta);
        }
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("terminal frame"),
            });
        let commands =
            self.renderer
                .update_buffers(&self.device, &self.queue, &mut encoder, &jobs, &screen);
        let timed = self
            .timer
            .as_ref()
            .is_some_and(|timer| timer.pending.is_none());
        {
            let timestamp_writes = self.timer.as_ref().filter(|_| timed).map(|timer| {
                wgpu::RenderPassTimestampWrites {
                    query_set: &timer.queries,
                    beginning_of_pass_write_index: Some(0),
                    end_of_pass_write_index: Some(1),
                }
            });
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terminal"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                timestamp_writes,
                ..Default::default()
            });
            self.renderer
                .render(&mut pass.forget_lifetime(), &jobs, &screen);
        }
        if let Some(timer) = self.timer.as_ref().filter(|_| timed) {
            encoder.resolve_query_set(&timer.queries, 0..2, &timer.resolve, 0);
            encoder.copy_buffer_to_buffer(&timer.resolve, 0, &timer.readback, 0, 16);
        }
        let cpu = started.elapsed();
        self.queue
            .submit(commands.into_iter().chain([encoder.finish()]));
        if let Some(timer) = self.timer.as_mut().filter(|_| timed) {
            timer.map(self.frame_number);
        }
        self.window.pre_present_notify();
        frame.present();
        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }
        if self.profile {
            let (rows, cols) = terminal.screen().size();
            eprintln!(
                "frame={} grid={cols}x{rows} cpu_prepare_us={} input_to_present_us={:?}",
                self.frame_number,
                cpu.as_micros(),
                input_started.map(|time| time.elapsed().as_micros())
            );
        }
        Ok((Frame::Presented, resized))
    }
}

struct GpuTimer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    pending: Option<(u64, mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>)>,
}

impl GpuTimer {
    fn new(device: &wgpu::Device) -> Self {
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("frame timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("timestamp resolve"),
            size: 16,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("timestamp readback"),
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            queries,
            resolve,
            readback,
            pending: None,
        }
    }

    fn map(&mut self, frame: u64) {
        let (send, receive) = mpsc::channel();
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = send.send(result);
            });
        self.pending = Some((frame, receive));
    }

    fn collect(&mut self, period: f32) -> Option<(u64, Duration)> {
        let mut sample = None;
        if let Some((frame, receive)) = &self.pending
            && let Ok(result) = receive.try_recv()
        {
            match result {
                Ok(()) => {
                    let bytes = self.readback.slice(..).get_mapped_range();
                    let start = u64::from_ne_bytes(bytes[0..8].try_into().unwrap());
                    let end = u64::from_ne_bytes(bytes[8..16].try_into().unwrap());
                    let elapsed = Duration::from_secs_f64(
                        end.saturating_sub(start) as f64 * f64::from(period) / 1e9,
                    );
                    sample = Some((*frame, elapsed));
                    drop(bytes);
                    self.readback.unmap();
                }
                Err(error) => eprintln!("GPU timing unavailable: {error}"),
            }
            self.pending = None;
        }
        sample
    }
}

#[cfg(test)]
#[path = "graphics_test.rs"]
mod tests;
