//! Opt-in renderer smoke test and bounded offscreen timing study. This exercises
//! the real egui/wgpu pipeline without requiring a window server.
use super::*;
use std::{fs::File, io::Write};

#[test]
#[ignore = "requires a Vulkan adapter; writes captures under target/terminal-validation"]
fn offscreen_renderer() {
    futures_lite::future::block_on(async {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .unwrap();
        let features = adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: features,
                ..Default::default()
            })
            .await
            .unwrap();
        eprintln!("Offscreen adapter: {:?}", adapter.get_info());
        std::fs::create_dir_all("target/terminal-validation").unwrap();
        for (cols, rows) in [(80, 24), (240, 80)] {
            let context = egui::Context::default();
            crate::view::install_fonts(&context, &crate::config::Font::default());
            let mut terminal = Terminal::new(rows, cols, 10_000);
            let mut view = TerminalView::default();
            let mut renderer = egui_wgpu::Renderer::new(
                &device,
                wgpu::TextureFormat::Rgba8Unorm,
                egui_wgpu::RendererOptions::default(),
            );
            let warmup = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(816.0, 592.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    view.show(ctx, &mut terminal, "", true, true);
                },
            );
            for (id, delta) in &warmup.textures_delta.set {
                renderer.update_texture(&device, &queue, *id, delta);
            }
            let cell = view.metrics.unwrap().cell;
            let width = (cell.x * f32::from(cols) + 16.0) as u32;
            let height = (cell.y * f32::from(rows) + 16.0) as u32;
            let size = wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            };
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("offscreen terminal"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let target = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let mut timer = features
                .contains(wgpu::Features::TIMESTAMP_QUERY)
                .then(|| GpuTimer::new(&device));
            for mode in ["idle", "output"] {
                let mut cpu_times = Vec::new();
                let mut gpu_times = Vec::new();
                for frame in 0..35 {
                    let started = Instant::now();
                    if mode == "output" {
                        let text: String = (0..usize::from(rows) * usize::from(cols) - 1)
                            .map(|index| char::from(b'!' + ((index + frame as usize) % 90) as u8))
                            .collect();
                        terminal.process(
                            format!("\x1b[H\x1b[38;5;{}m{text}", 16 + frame % 216).as_bytes(),
                        );
                    }
                    let output = context.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width as f32, height as f32),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            view.show(ctx, &mut terminal, "", true, true);
                        },
                    );
                    assert_eq!(terminal.screen().size(), (rows, cols));
                    let jobs = context.tessellate(output.shapes, output.pixels_per_point);
                    let screen = egui_wgpu::ScreenDescriptor {
                        size_in_pixels: [width, height],
                        pixels_per_point: output.pixels_per_point,
                    };
                    for (id, delta) in &output.textures_delta.set {
                        renderer.update_texture(&device, &queue, *id, delta);
                    }
                    let mut encoder =
                        device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                    let commands =
                        renderer.update_buffers(&device, &queue, &mut encoder, &jobs, &screen);
                    {
                        let timestamps =
                            timer.as_ref().map(|timer| wgpu::RenderPassTimestampWrites {
                                query_set: &timer.queries,
                                beginning_of_pass_write_index: Some(0),
                                end_of_pass_write_index: Some(1),
                            });
                        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &target,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            timestamp_writes: timestamps,
                            ..Default::default()
                        });
                        renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
                    }
                    if let Some(timer) = &timer {
                        encoder.resolve_query_set(&timer.queries, 0..2, &timer.resolve, 0);
                        encoder.copy_buffer_to_buffer(&timer.resolve, 0, &timer.readback, 0, 16);
                    }
                    let cpu = started.elapsed();
                    queue.submit(commands.into_iter().chain([encoder.finish()]));
                    if let Some(timer) = &mut timer {
                        timer.map(frame);
                    }
                    device
                        .poll(wgpu::PollType::Wait {
                            submission_index: None,
                            timeout: Some(Duration::from_secs(10)),
                        })
                        .unwrap();
                    if let Some(timer) = &mut timer
                        && let Some((_, gpu)) = timer.collect(queue.get_timestamp_period())
                        && frame >= 5
                    {
                        gpu_times.push(gpu.as_micros());
                    }
                    for id in &output.textures_delta.free {
                        renderer.free_texture(id);
                    }
                    if frame >= 5 {
                        cpu_times.push(cpu.as_micros());
                    }
                }
                cpu_times.sort_unstable();
                gpu_times.sort_unstable();
                eprintln!(
                    "{cols}x{rows} {mode}: 30 frames after 5 warmup; CPU p50/p95={} / {} us; GPU p50/p95={:?} / {:?} us",
                    cpu_times[15],
                    cpu_times[28],
                    gpu_times.get(15),
                    gpu_times.get(28)
                );
                capture(
                    &device,
                    &queue,
                    &texture,
                    size,
                    &format!("target/terminal-validation/{cols}x{rows}-{mode}.ppm"),
                );
            }
        }
    });
}

fn capture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    size: wgpu::Extent3d,
    path: &str,
) {
    let stride = (size.width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("capture"),
        size: u64::from(stride) * u64::from(size.height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: None,
            },
        },
        size,
    );
    queue.submit([encoder.finish()]);
    let (send, receive) = mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            send.send(result).unwrap()
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(10)),
        })
        .unwrap();
    receive.recv().unwrap().unwrap();
    let bytes = buffer.slice(..).get_mapped_range();
    let mut file = std::io::BufWriter::new(File::create(path).unwrap());
    writeln!(file, "P6\n{} {}\n255", size.width, size.height).unwrap();
    let mut lit_pixels = 0;
    for row in bytes.chunks_exact(stride as usize) {
        for pixel in row[..size.width as usize * 4].as_chunks::<4>().0 {
            file.write_all(&pixel[..3]).unwrap();
            lit_pixels += usize::from(pixel[0] > 100 || pixel[1] > 100 || pixel[2] > 100);
        }
    }
    assert!(
        lit_pixels > 1000,
        "the rendered terminal must contain visible text"
    );
}
