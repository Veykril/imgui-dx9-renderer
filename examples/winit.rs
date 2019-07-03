use imgui::{im_str, FontConfig, FontSource};
use imgui_winit_support::{HiDpiMode, WinitPlatform};
use winapi::shared::{d3d9::*, d3d9caps::*, d3d9types::*, windef::HWND};
use winit::{dpi::LogicalSize, os::windows::WindowExt, Event};

use core::ptr;
use std::{ptr::NonNull, time::Instant};

const WINDOW_WIDTH: f64 = 760.0;
const WINDOW_HEIGHT: f64 = 760.0;

unsafe fn set_up_dx_context(hwnd: HWND) -> (LPDIRECT3D9, LPDIRECT3DDEVICE9) {
    let d9 = Direct3DCreate9(D3D_SDK_VERSION);
    if d9.is_null() {
        panic!("Direct3DCreate9 failed");
    }
    let mut present_params = D3DPRESENT_PARAMETERS {
        BackBufferCount: 1,
        MultiSampleType: D3DMULTISAMPLE_NONE,
        MultiSampleQuality: 0,
        SwapEffect: D3DSWAPEFFECT_DISCARD,
        hDeviceWindow: hwnd,
        Flags: 0,
        FullScreen_RefreshRateInHz: D3DPRESENT_RATE_DEFAULT,
        PresentationInterval: D3DPRESENT_INTERVAL_DEFAULT,
        BackBufferFormat: D3DFMT_R5G6B5,
        EnableAutoDepthStencil: 0,
        Windowed: 1,
        BackBufferWidth: WINDOW_WIDTH as _,
        BackBufferHeight: WINDOW_HEIGHT as _,
        ..core::mem::zeroed()
    };
    let mut device = ptr::null_mut();
    let r = ((*(*d9).lpVtbl).CreateDevice)(
        d9,
        D3DADAPTER_DEFAULT,
        D3DDEVTYPE_HAL,
        hwnd,
        D3DCREATE_SOFTWARE_VERTEXPROCESSING,
        &mut present_params,
        &mut device,
    );
    if r < 0 {
        panic!("CreateDevice failed");
    }
    (d9, device)
}

fn main() {
    let mut events_loop = winit::EventsLoop::new();
    let window = winit::WindowBuilder::new()
        .with_title("imgui_dx9_renderer winit example")
        .with_resizable(false)
        .with_dimensions(LogicalSize {
            width: WINDOW_WIDTH,
            height: WINDOW_HEIGHT,
        })
        .build(&events_loop)
        .unwrap();

    let (d9, device) = unsafe { set_up_dx_context(window.get_hwnd() as _) };
    let mut imgui = imgui::Context::create();
    let mut renderer =
        imgui_dx9_renderer::Renderer::new(&mut imgui, unsafe { NonNull::new_unchecked(device) })
            .unwrap();
    {
        // Fix incorrect colors with sRGB framebuffer
        fn imgui_gamma_to_linear(col: [f32; 4]) -> [f32; 4] {
            [
                col[0].powf(2.2),
                col[1].powf(2.2),
                col[2].powf(2.2),
                1.0 - (1.0 - col[3]).powf(2.2),
            ]
        }

        let style = imgui.style_mut();
        for col in 0..style.colors.len() {
            //style.colors[col] = imgui_gamma_to_linear(style.colors[col]);
        }
    }
    let mut platform = WinitPlatform::init(&mut imgui);
    platform.attach_window(imgui.io_mut(), &window, HiDpiMode::Rounded);

    let hidpi_factor = platform.hidpi_factor();
    let font_size = (13.0 * hidpi_factor) as f32;
    imgui.fonts().add_font(&[FontSource::DefaultFontData {
        config: Some(FontConfig {
            size_pixels: font_size,
            ..FontConfig::default()
        }),
    }]);

    imgui.io_mut().font_global_scale = (1.0 / hidpi_factor) as f32;

    imgui.set_ini_filename(None);

    let mut last_frame = Instant::now();
    let mut quit = false;
    events_loop.poll_events(|_| {});

    loop {
        events_loop.poll_events(|event| {
            platform.handle_event(imgui.io_mut(), &window, &event);
            use winit::WindowEvent;

            if let Event::WindowEvent { event, .. } = event {
                match event {
                    WindowEvent::Resized(_) => unimplemented!(),
                    WindowEvent::CloseRequested => quit = true,
                    _ => (),
                }
            }
        });
        if quit {
            break;
        }
        unsafe {
            ((*(*device).lpVtbl).Clear)(
                device,
                0,
                ptr::null_mut(),
                D3DCLEAR_TARGET,
                0xFF101010,
                1.0,
                0,
            );
            ((*(*device).lpVtbl).BeginScene)(device);
        }

        let now = Instant::now();
        let delta = now - last_frame;
        let delta_s = delta.as_secs() as f32 + delta.subsec_nanos() as f32 / 1_000_000_000.0;
        last_frame = now;

        let io = imgui.io_mut();
        platform
            .prepare_frame(io, &window)
            .expect("Failed to start frame");
        io.update_delta_time(last_frame);

        let mut ui = imgui.frame();
        ui.window(im_str!("Hello world"))
            .size([300.0, 100.0], imgui::Condition::FirstUseEver)
            .build(|| {
                ui.text(im_str!("Hello world!"));
                ui.text(im_str!("こんにちは世界！"));
                ui.text(im_str!("This...is...imgui-rs!"));
                ui.separator();
                let mouse_pos = ui.imgui().mouse_pos();
                ui.text(im_str!(
                    "Mouse Position: ({:.1},{:.1})",
                    mouse_pos.0,
                    mouse_pos.1
                ));
            });
        ui.window(im_str!("Hello wo4rld"))
            .size([300.0, 100.0], imgui::Condition::FirstUseEver)
            .build(|| {
                ui.text(im_str!("Hello world!"));
                ui.text(im_str!("This...is...imgui-rs!"));
                ui.separator();
                let mouse_pos = ui.imgui().mouse_pos();
                ui.text(im_str!(
                    "Mouse Position: ({:.1},{:.1})",
                    mouse_pos.0,
                    mouse_pos.1
                ));
            });
        renderer.render(ui).unwrap();
        unsafe {
            ((*(*device).lpVtbl).EndScene)(device);
            ((*(*device).lpVtbl).Present)(
                device,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            );
        }
    }

    unsafe { ((*(*d9).lpVtbl).parent.Release)(d9 as *mut _ as _) };
}
