#![cfg(target_os = "android")]

use winit_core::application::ApplicationHandler;
use winit_core::event::WindowEvent;
use winit_core::event_loop::ActiveEventLoop;
use winit_core::window::{WindowAttributes, WindowId};

#[derive(Default)]
struct App;

impl ApplicationHandler for App {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        let _window = event_loop.create_window(WindowAttributes::default()).unwrap();
    }

    fn window_event(
        &mut self,
        _event_loop: &dyn ActiveEventLoop,
        _window_id: WindowId,
        _event: WindowEvent,
    ) {
    }
}

winit_android::java_activity_entry!(App, App);

fn main() {}
