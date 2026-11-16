use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use android_activity::input::{KeyMapChar, Keycode};
use dpi::{PhysicalPosition, PhysicalSize};
use jni::objects::JObject;
use jni::Env;
use ndk::native_window::NativeWindow;
use winit_core::application::{ApplicationHandler, BackResponse, InputResponse};
use winit_core::event::{
    ButtonSource, DeviceId, ElementState, FingerId, Force, KeyEvent, PointerKind, PointerSource,
    StartCause, WindowEvent,
};
use winit_core::event_loop::{ActiveEventLoop as _, ControlFlow};
use winit_core::text_input::{TextInputRequest, TextInputResponse, TextInputSessionId};

use super::window::{ActiveEventLoop, ImeCommand, JavaBridge, Shared, GLOBAL_WINDOW};
use crate::keycodes;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PointerAction {
    Down,
    Move,
    Up,
    Cancel,
}

/// One Android pointer sample. `time_nanos` uses Android's monotonic uptime clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerSample {
    pub time_nanos: u64,
    pub device_id: i64,
    pub pointer_id: usize,
    pub position: PhysicalPosition<f64>,
    pub pressure: f64,
}

/// Owns one application on Android's Java UI thread.
///
/// Java must call these methods serially from the same Looper thread that created the runner.
pub struct Runner<A: ApplicationHandler> {
    app: Option<A>,
    event_loop: ActiveEventLoop,
    ui_thread: ThreadId,
    callback_active: bool,
    initialized: bool,
    started: bool,
    primary_pointer: Option<FingerId>,
}

impl<A: ApplicationHandler> Runner<A> {
    pub fn new(
        env: &mut Env<'_>,
        activity: JObject<'_>,
        app: A,
    ) -> jni::errors::Result<Self> {
        let bridge = JavaBridge::new(env, activity)?;
        let shared = Shared::new(bridge);
        let mut runner = Self {
            app: Some(app),
            event_loop: ActiveEventLoop::new(shared),
            ui_thread: thread::current().id(),
            callback_active: false,
            initialized: false,
            started: false,
            primary_pointer: None,
        };
        runner.initialize();
        Ok(runner)
    }

    fn assert_ui_thread(&self) {
        assert_eq!(
            thread::current().id(),
            self.ui_thread,
            "Java Activity runner called from a non-UI thread"
        );
    }

    fn with_application_callback<R>(
        &mut self,
        callback: impl FnOnce(&mut A, &ActiveEventLoop) -> R,
    ) -> R {
        self.assert_ui_thread();
        assert!(!self.callback_active, "nested Java Activity application callback");
        self.callback_active = true;
        self.event_loop.shared.set_callback_active(true);

        let result = catch_unwind(AssertUnwindSafe(|| {
            callback(self.app.as_mut().expect("Android application already destroyed"), &self.event_loop)
        }));

        self.event_loop.shared.set_callback_active(false);
        self.callback_active = false;

        match result {
            Ok(value) => value,
            Err(payload) => resume_unwind(payload),
        }
    }

    fn initialize(&mut self) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.with_application_callback(|app, event_loop| {
            app.new_events(event_loop, StartCause::Init);
            app.about_to_wait(event_loop);
        });
    }

    fn turn(&mut self, dispatch: impl FnOnce(&mut A, &ActiveEventLoop)) {
        let start = Instant::now();
        self.with_application_callback(|app, event_loop| {
            app.new_events(
                event_loop,
                StartCause::WaitCancelled { start, requested_resume: None },
            );
            dispatch(app, event_loop);
            app.about_to_wait(event_loop);
        });

        if self.event_loop.control_flow.get() == ControlFlow::Poll {
            self.event_loop.shared.request_redraw();
        }
    }

    pub fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        self.turn(|app, event_loop| app.resumed(event_loop));
    }

    pub fn stop(&mut self) {
        if !self.started {
            return;
        }
        self.started = false;
        self.turn(|app, event_loop| app.suspended(event_loop));
    }

    pub fn set_focus(&mut self, focused: bool) {
        self.event_loop.shared.set_focused(focused);
        self.turn(|app, event_loop| {
            app.window_event(event_loop, GLOBAL_WINDOW, WindowEvent::Focused(focused));
        });
    }

    /// Installs an Android `Surface` and synchronously permits render-surface creation.
    ///
    /// # Safety
    /// `surface` must be an `android.view.Surface` local reference belonging to `env`.
    pub unsafe fn surface_created(
        &mut self,
        env: &mut Env<'_>,
        surface: JObject<'_>,
        width: u32,
        height: u32,
        scale_factor: f64,
    ) -> Result<(), &'static str> {
        self.assert_ui_thread();
        if self.event_loop.shared.has_surface() {
            return Err("surface_created called while a surface is still active");
        }
        let native_window = unsafe {
            NativeWindow::from_surface(env.get_raw().cast(), surface.as_raw().cast())
        }
        .ok_or("ANativeWindow_fromSurface returned null")?;
        self.event_loop.shared.set_surface(native_window, width, height, scale_factor);
        self.turn(|app, event_loop| {
            app.can_create_surfaces(event_loop);
            app.window_event(
                event_loop,
                GLOBAL_WINDOW,
                WindowEvent::SurfaceResized(PhysicalSize::new(width, height)),
            );
        });
        Ok(())
    }

    pub fn surface_changed(&mut self, width: u32, height: u32, scale_factor: f64) {
        self.event_loop.shared.set_surface_size(width, height, scale_factor);
        self.turn(|app, event_loop| {
            app.window_event(
                event_loop,
                GLOBAL_WINDOW,
                WindowEvent::SurfaceResized(PhysicalSize::new(width, height)),
            );
        });
    }

    pub fn surface_destroyed(&mut self) {
        if !self.event_loop.shared.has_surface() {
            return;
        }
        self.turn(|app, event_loop| app.destroy_surfaces(event_loop));
        self.event_loop.shared.clear_surface();
    }

    pub fn set_insets(&mut self, left: u32, top: u32, right: u32, bottom: u32) {
        self.event_loop.shared.set_insets(left, top, right, bottom);
        self.event_loop.shared.request_redraw();
    }

    pub fn frame(&mut self) {
        let redraw = self.event_loop.shared.take_redraw_pending();
        let proxy_wake = self.event_loop.shared.take_proxy_wake();
        if !redraw && !proxy_wake && self.event_loop.control_flow.get() != ControlFlow::Poll {
            return;
        }
        self.turn(move |app, event_loop| {
            if proxy_wake {
                app.proxy_wake_up(event_loop);
            }
            if redraw && event_loop.shared.has_surface() {
                app.window_event(event_loop, GLOBAL_WINDOW, WindowEvent::RedrawRequested);
            }
        });
    }

    pub fn pointer_batch(&mut self, action: PointerAction, samples: &[PointerSample]) -> bool {
        if samples.is_empty() {
            return false;
        }

        let mut events = Vec::with_capacity(samples.len() * 2);
        for sample in samples {
            let device_id = Some(DeviceId::from_raw(sample.device_id));
            let finger_id = FingerId::from_raw(sample.pointer_id);
            let force = Some(Force::Normalized(sample.pressure));
            match action {
                PointerAction::Down => {
                    let primary = self.primary_pointer.is_none();
                    if primary {
                        self.primary_pointer = Some(finger_id);
                    }
                    events.push(WindowEvent::PointerEntered {
                        device_id,
                        position: sample.position,
                        primary,
                        kind: PointerKind::Touch(finger_id),
                    });
                    events.push(WindowEvent::PointerButton {
                        device_id,
                        state: ElementState::Pressed,
                        timestamp: Some(Duration::from_nanos(sample.time_nanos)),
                        position: sample.position,
                        primary,
                        button: ButtonSource::Touch { finger_id, force },
                        is_macos_activation_click: false,
                    });
                },
                PointerAction::Move => {
                    events.push(WindowEvent::PointerMoved {
                        device_id,
                        timestamp: Some(Duration::from_nanos(sample.time_nanos)),
                        position: sample.position,
                        primary: self.primary_pointer == Some(finger_id),
                        source: PointerSource::Touch { finger_id, force },
                    });
                },
                PointerAction::Up | PointerAction::Cancel => {
                    let primary = self.primary_pointer == Some(finger_id);
                    if action == PointerAction::Up {
                        events.push(WindowEvent::PointerButton {
                            device_id,
                            state: ElementState::Released,
                            timestamp: Some(Duration::from_nanos(sample.time_nanos)),
                            position: sample.position,
                            primary,
                            button: ButtonSource::Touch { finger_id, force },
                            is_macos_activation_click: false,
                        });
                    }
                    events.push(WindowEvent::PointerLeft {
                        device_id,
                        position: Some(sample.position),
                        primary,
                        kind: PointerKind::Touch(finger_id),
                    });
                    if primary {
                        self.primary_pointer = None;
                    }
                },
            }
        }

        self.turn(move |app, event_loop| {
            for event in events {
                app.window_event(event_loop, GLOBAL_WINDOW, event);
            }
        });
        true
    }

    pub fn keyboard_input(
        &mut self,
        pressed: bool,
        device_id: i64,
        keycode: u32,
        unicode: u32,
        unicode_without_modifiers: u32,
        repeat: bool,
    ) -> InputResponse {
        let keycode = Keycode::from(keycode);
        let key_char = key_map_char(unicode);
        let key_without_modifiers = keycodes::to_logical(
            key_map_char(unicode_without_modifiers),
            keycode,
        );
        let state = if pressed { ElementState::Pressed } else { ElementState::Released };
        let logical_key = keycodes::to_logical(key_char, keycode);
        let text = if pressed {
            logical_key.to_text().map(smol_str::SmolStr::new)
        } else {
            None
        };
        let event = KeyEvent {
            state,
            physical_key: keycodes::to_physical_key(keycode),
            logical_key,
            location: keycodes::to_location(keycode),
            repeat,
            text: text.clone(),
            text_with_all_modifiers: text,
            key_without_modifiers,
        };
        self.with_application_callback(|app, event_loop| {
            app.keyboard_input(
                event_loop,
                GLOBAL_WINDOW,
                Some(DeviceId::from_raw(device_id)),
                event,
                false,
            )
        })
    }

    pub fn back_requested(&mut self) -> BackResponse {
        self.with_application_callback(|app, event_loop| {
            app.back_requested(event_loop, GLOBAL_WINDOW)
        })
    }

    pub fn text_input_request(
        &mut self,
        session_id: TextInputSessionId,
        request: TextInputRequest,
    ) -> TextInputResponse {
        self.with_application_callback(|app, event_loop| {
            app.text_input_request(event_loop, GLOBAL_WINDOW, session_id, request)
        })
    }

    pub fn take_work_pending(&self) -> bool {
        self.event_loop.shared.take_work_pending()
    }

    pub fn take_ime_command(&self) -> Option<ImeCommand> {
        self.event_loop.shared.take_ime_command()
    }

    pub fn exiting(&self) -> bool {
        self.event_loop.exiting()
    }

    pub fn destroy(mut self) {
        if self.event_loop.shared.has_surface() {
            self.surface_destroyed();
        }
        if self.started {
            self.stop();
        }
        self.app.take();
    }
}

fn key_map_char(value: u32) -> Option<KeyMapChar> {
    const COMBINING_ACCENT: u32 = 0x8000_0000;
    const COMBINING_ACCENT_MASK: u32 = 0x7fff_ffff;

    if value == 0 {
        None
    } else if value & COMBINING_ACCENT != 0 {
        char::from_u32(value & COMBINING_ACCENT_MASK).map(KeyMapChar::CombiningAccent)
    } else {
        char::from_u32(value).map(KeyMapChar::Unicode)
    }
}
