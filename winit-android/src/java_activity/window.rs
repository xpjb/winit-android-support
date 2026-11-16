use std::ffi::c_void;
use std::fmt;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use dpi::{PhysicalInsets, PhysicalPosition, PhysicalSize, Position, Size};
use jni::objects::{Global, JObject};
use jni::{Env, JavaVM};
use ndk::native_window::NativeWindow;
use tracing::warn;
use winit_core::cursor::{Cursor, CustomCursor, CustomCursorSource};
use winit_core::error::{
    CreateWindowError, CustomCursorError, NotSupportedError, RequestError,
};
use winit_core::event_loop::{EventLoopProxy as CoreEventLoopProxy, OwnedDisplayHandle};
use winit_core::monitor::{Fullscreen, MonitorHandle};
use winit_core::text_input::TextInputSessionId;
use winit_core::window::{
    self, CursorGrabMode, ImeCapabilities, ImeHint, ImePurpose, ImeRequest, ImeRequestError,
    ResizeDirection, Theme, Window as CoreWindow, WindowAttributes, WindowButtons, WindowId,
    WindowLevel,
};

pub(super) const GLOBAL_WINDOW: WindowId = WindowId::from_raw(0);

pub(super) struct JavaBridge {
    vm: JavaVM,
    activity: Global<JObject<'static>>,
}

impl fmt::Debug for JavaBridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JavaBridge").finish_non_exhaustive()
    }
}

impl JavaBridge {
    pub(super) fn new(env: &mut Env<'_>, activity: JObject<'_>) -> jni::errors::Result<Self> {
        Ok(Self { vm: env.get_java_vm()?, activity: env.new_global_ref(activity)? })
    }

    fn request_rust_wake(&self) {
        let result = self.vm.attach_current_thread(|env| {
            env.call_method(
                &self.activity,
                jni::jni_str!("requestRustWake"),
                jni::jni_sig!("()V"),
                &[],
            )
            .map(|_| ())
        });
        if let Err(error) = result {
            tracing::error!(?error, "failed to post a Java-main-loop wake-up");
        }
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ImeCommand {
    Enable {
        session_id: TextInputSessionId,
        hint: ImeHint,
        purpose: ImePurpose,
    },
    Update {
        session_id: TextInputSessionId,
        hint: ImeHint,
        purpose: ImePurpose,
    },
    Disable { session_id: TextInputSessionId },
}

impl ImeCommand {
    pub fn into_wire(self) -> [i64; 4] {
        match self {
            Self::Enable { session_id, hint, purpose } => {
                [1, session_id.into_raw() as i64, hint.bits() as i64, purpose_code(purpose)]
            },
            Self::Update { session_id, hint, purpose } => {
                [2, session_id.into_raw() as i64, hint.bits() as i64, purpose_code(purpose)]
            },
            Self::Disable { session_id } => [3, session_id.into_raw() as i64, 0, 0],
        }
    }
}

fn purpose_code(purpose: ImePurpose) -> i64 {
    match purpose {
        ImePurpose::Normal => 0,
        ImePurpose::Password => 1,
        ImePurpose::Terminal => 2,
        ImePurpose::Number => 3,
        ImePurpose::Phone => 4,
        ImePurpose::Url => 5,
        ImePurpose::Email => 6,
        ImePurpose::Pin => 7,
        ImePurpose::Date => 8,
        ImePurpose::Time => 9,
        ImePurpose::DateTime => 10,
        _ => 0,
    }
}

#[derive(Debug)]
struct ImeState {
    configured_session: Option<TextInputSessionId>,
    active_session: Option<TextInputSessionId>,
    capabilities: Option<ImeCapabilities>,
    hint: ImeHint,
    purpose: ImePurpose,
}

impl Default for ImeState {
    fn default() -> Self {
        Self {
            configured_session: None,
            active_session: None,
            capabilities: None,
            hint: ImeHint::NONE,
            purpose: ImePurpose::Normal,
        }
    }
}

pub(super) struct Shared {
    bridge: JavaBridge,
    callback_active: AtomicBool,
    work_pending: AtomicBool,
    redraw_pending: AtomicBool,
    proxy_wake_pending: AtomicBool,
    native_window: RwLock<Option<NativeWindow>>,
    native_window_ptr: AtomicPtr<c_void>,
    width: AtomicU32,
    height: AtomicU32,
    scale_factor_bits: AtomicU64,
    inset_left: AtomicU32,
    inset_top: AtomicU32,
    inset_right: AtomicU32,
    inset_bottom: AtomicU32,
    focused: AtomicBool,
    title: Mutex<String>,
    ime: Mutex<ImeState>,
    ime_command: Mutex<Option<ImeCommand>>,
    next_session: AtomicU64,
}

impl fmt::Debug for Shared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JavaActivityShared")
            .field("surface_size", &self.surface_size())
            .field("scale_factor", &self.scale_factor())
            .field("focused", &self.focused.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl Shared {
    pub(super) fn new(bridge: JavaBridge) -> Arc<Self> {
        Arc::new(Self {
            bridge,
            callback_active: AtomicBool::new(false),
            work_pending: AtomicBool::new(false),
            redraw_pending: AtomicBool::new(false),
            proxy_wake_pending: AtomicBool::new(false),
            native_window: RwLock::new(None),
            native_window_ptr: AtomicPtr::new(std::ptr::null_mut()),
            width: AtomicU32::new(0),
            height: AtomicU32::new(0),
            scale_factor_bits: AtomicU64::new(1.0f64.to_bits()),
            inset_left: AtomicU32::new(0),
            inset_top: AtomicU32::new(0),
            inset_right: AtomicU32::new(0),
            inset_bottom: AtomicU32::new(0),
            focused: AtomicBool::new(false),
            title: Mutex::new(String::new()),
            ime: Mutex::new(ImeState::default()),
            ime_command: Mutex::new(None),
            next_session: AtomicU64::new(1),
        })
    }

    pub(super) fn set_callback_active(&self, active: bool) {
        self.callback_active.store(active, Ordering::Release);
    }

    pub(super) fn request_work(&self) {
        if !self.work_pending.swap(true, Ordering::AcqRel)
            && !self.callback_active.load(Ordering::Acquire)
        {
            self.bridge.request_rust_wake();
        }
    }

    pub(super) fn take_work_pending(&self) -> bool {
        self.work_pending.swap(false, Ordering::AcqRel)
    }

    pub(super) fn request_redraw(&self) {
        self.redraw_pending.store(true, Ordering::Release);
        self.request_work();
    }

    pub(super) fn take_redraw_pending(&self) -> bool {
        self.redraw_pending.swap(false, Ordering::AcqRel)
    }

    pub(super) fn request_proxy_wake(&self) {
        self.proxy_wake_pending.store(true, Ordering::Release);
        self.request_work();
    }

    pub(super) fn take_proxy_wake(&self) -> bool {
        self.proxy_wake_pending.swap(false, Ordering::AcqRel)
    }

    pub(super) fn set_surface(
        &self,
        native_window: NativeWindow,
        width: u32,
        height: u32,
        scale_factor: f64,
    ) {
        self.width.store(width, Ordering::Release);
        self.height.store(height, Ordering::Release);
        self.scale_factor_bits.store(scale_factor.to_bits(), Ordering::Release);
        self.native_window_ptr
            .store(native_window.ptr().cast::<c_void>().as_ptr(), Ordering::Release);
        *self.native_window.write().unwrap() = Some(native_window);
    }

    pub(super) fn clear_surface(&self) {
        self.native_window_ptr.store(std::ptr::null_mut(), Ordering::Release);
        *self.native_window.write().unwrap() = None;
        self.width.store(0, Ordering::Release);
        self.height.store(0, Ordering::Release);
    }

    pub(super) fn has_surface(&self) -> bool {
        !self.native_window_ptr.load(Ordering::Acquire).is_null()
    }

    pub(super) fn set_surface_size(&self, width: u32, height: u32, scale_factor: f64) {
        self.width.store(width, Ordering::Release);
        self.height.store(height, Ordering::Release);
        self.scale_factor_bits.store(scale_factor.to_bits(), Ordering::Release);
    }

    pub(super) fn surface_size(&self) -> PhysicalSize<u32> {
        PhysicalSize::new(
            self.width.load(Ordering::Acquire),
            self.height.load(Ordering::Acquire),
        )
    }

    pub(super) fn scale_factor(&self) -> f64 {
        f64::from_bits(self.scale_factor_bits.load(Ordering::Acquire))
    }

    pub(super) fn set_insets(&self, left: u32, top: u32, right: u32, bottom: u32) {
        self.inset_left.store(left, Ordering::Release);
        self.inset_top.store(top, Ordering::Release);
        self.inset_right.store(right, Ordering::Release);
        self.inset_bottom.store(bottom, Ordering::Release);
    }

    pub(super) fn set_focused(&self, focused: bool) {
        self.focused.store(focused, Ordering::Release);
    }

    pub(super) fn take_ime_command(&self) -> Option<ImeCommand> {
        self.ime_command.lock().unwrap().take()
    }

    fn queue_ime_command(&self, command: ImeCommand) {
        *self.ime_command.lock().unwrap() = Some(command);
        self.request_work();
    }
}

#[derive(Debug)]
pub(super) struct EventLoopProxy {
    shared: Arc<Shared>,
}

impl winit_core::event_loop::EventLoopProxyProvider for EventLoopProxy {
    fn wake_up(&self) {
        self.shared.request_proxy_wake();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OwnedDisplay;

impl rwh_06::HasDisplayHandle for OwnedDisplay {
    fn display_handle(&self) -> Result<rwh_06::DisplayHandle<'_>, rwh_06::HandleError> {
        let raw = rwh_06::AndroidDisplayHandle::new();
        Ok(unsafe { rwh_06::DisplayHandle::borrow_raw(raw.into()) })
    }
}

#[derive(Debug)]
pub(super) struct ActiveEventLoop {
    pub(super) shared: Arc<Shared>,
    pub(super) control_flow: std::cell::Cell<winit_core::event_loop::ControlFlow>,
    pub(super) exit: std::cell::Cell<bool>,
    proxy: Arc<EventLoopProxy>,
}

impl ActiveEventLoop {
    pub(super) fn new(shared: Arc<Shared>) -> Self {
        let proxy = Arc::new(EventLoopProxy { shared: shared.clone() });
        Self {
            shared,
            control_flow: std::cell::Cell::new(winit_core::event_loop::ControlFlow::Wait),
            exit: std::cell::Cell::new(false),
            proxy,
        }
    }
}

impl winit_core::event_loop::ActiveEventLoop for ActiveEventLoop {
    fn create_proxy(&self) -> CoreEventLoopProxy {
        CoreEventLoopProxy::new(self.proxy.clone())
    }

    fn create_window(
        &self,
        attributes: WindowAttributes,
    ) -> Result<Box<dyn CoreWindow>, CreateWindowError> {
        Ok(Box::new(Window::new(self.shared.clone(), attributes)?))
    }

    fn create_custom_cursor(
        &self,
        _source: CustomCursorSource,
    ) -> Result<CustomCursor, CustomCursorError> {
        Err(CustomCursorError::NotSupported)
    }

    fn available_monitors(&self) -> Box<dyn Iterator<Item = MonitorHandle>> {
        Box::new(std::iter::empty())
    }

    fn primary_monitor(&self) -> Option<MonitorHandle> {
        None
    }

    fn system_theme(&self) -> Option<Theme> {
        None
    }

    fn listen_device_events(&self, _allowed: winit_core::event_loop::DeviceEvents) {}

    fn set_control_flow(&self, control_flow: winit_core::event_loop::ControlFlow) {
        self.control_flow.set(control_flow);
        if control_flow == winit_core::event_loop::ControlFlow::Poll {
            self.shared.request_redraw();
        }
    }

    fn control_flow(&self) -> winit_core::event_loop::ControlFlow {
        self.control_flow.get()
    }

    fn exit(&self) {
        self.exit.set(true);
    }

    fn exiting(&self) -> bool {
        self.exit.get()
    }

    fn owned_display_handle(&self) -> OwnedDisplayHandle {
        OwnedDisplayHandle::new(Arc::new(OwnedDisplay))
    }

    fn rwh_06_handle(&self) -> &dyn rwh_06::HasDisplayHandle {
        self
    }
}

impl rwh_06::HasDisplayHandle for ActiveEventLoop {
    fn display_handle(&self) -> Result<rwh_06::DisplayHandle<'_>, rwh_06::HandleError> {
        let raw = rwh_06::AndroidDisplayHandle::new();
        Ok(unsafe { rwh_06::DisplayHandle::borrow_raw(raw.into()) })
    }
}

#[derive(Debug)]
pub(super) struct Window {
    shared: Arc<Shared>,
}

impl Window {
    fn new(shared: Arc<Shared>, attributes: WindowAttributes) -> Result<Self, CreateWindowError> {
        match attributes.window_type() {
            window::WindowType::Window => {},
            window::WindowType::Popup => return Err(CreateWindowError::PopupNotSupported),
            _ => panic!("unknown Android window type"),
        }
        *shared.title.lock().unwrap() = attributes.title;
        Ok(Self { shared })
    }

    fn set_text_input_session(&self, session_id: TextInputSessionId) {
        self.shared.ime.lock().unwrap().configured_session = Some(session_id);
    }

    fn raw_window_handle(&self) -> Result<rwh_06::RawWindowHandle, rwh_06::HandleError> {
        let ptr = NonNull::new(self.shared.native_window_ptr.load(Ordering::Acquire))
            .ok_or(rwh_06::HandleError::Unavailable)?;
        let handle = rwh_06::AndroidNdkWindowHandle::new(ptr);
        Ok(rwh_06::RawWindowHandle::AndroidNdk(handle))
    }
}

impl rwh_06::HasDisplayHandle for Window {
    fn display_handle(&self) -> Result<rwh_06::DisplayHandle<'_>, rwh_06::HandleError> {
        let raw = rwh_06::RawDisplayHandle::Android(rwh_06::AndroidDisplayHandle::new());
        Ok(unsafe { rwh_06::DisplayHandle::borrow_raw(raw) })
    }
}

impl rwh_06::HasWindowHandle for Window {
    fn window_handle(&self) -> Result<rwh_06::WindowHandle<'_>, rwh_06::HandleError> {
        Ok(unsafe { rwh_06::WindowHandle::borrow_raw(self.raw_window_handle()?) })
    }
}

impl CoreWindow for Window {
    fn window_type(&self) -> window::WindowType {
        window::WindowType::Window
    }

    fn id(&self) -> WindowId {
        GLOBAL_WINDOW
    }

    fn primary_monitor(&self) -> Option<MonitorHandle> {
        None
    }

    fn available_monitors(&self) -> Box<dyn Iterator<Item = MonitorHandle>> {
        Box::new(std::iter::empty())
    }

    fn current_monitor(&self) -> Option<MonitorHandle> {
        None
    }

    fn scale_factor(&self) -> f64 {
        self.shared.scale_factor()
    }

    fn request_redraw(&self) {
        self.shared.request_redraw();
    }

    fn pre_present_notify(&self) {}

    fn surface_position(&self) -> PhysicalPosition<i32> {
        PhysicalPosition::new(0, 0)
    }

    fn outer_position(&self) -> Result<PhysicalPosition<i32>, RequestError> {
        Err(NotSupportedError::new("outer_position is not supported").into())
    }

    fn set_outer_position(&self, _position: Position) {}

    fn surface_size(&self) -> PhysicalSize<u32> {
        self.shared.surface_size()
    }

    fn request_surface_size(&self, _size: Size) -> Option<PhysicalSize<u32>> {
        Some(self.surface_size())
    }

    fn outer_size(&self) -> PhysicalSize<u32> {
        self.surface_size()
    }

    fn safe_area(&self) -> PhysicalInsets<u32> {
        PhysicalInsets::new(
            self.shared.inset_left.load(Ordering::Acquire),
            self.shared.inset_top.load(Ordering::Acquire),
            self.shared.inset_right.load(Ordering::Acquire),
            self.shared.inset_bottom.load(Ordering::Acquire),
        )
    }

    fn set_min_surface_size(&self, _size: Option<Size>) {}

    fn set_max_surface_size(&self, _size: Option<Size>) {}

    fn surface_resize_increments(&self) -> Option<PhysicalSize<u32>> {
        None
    }

    fn set_surface_resize_increments(&self, _increments: Option<Size>) {}

    fn set_title(&self, title: &str) {
        *self.shared.title.lock().unwrap() = title.to_owned();
    }

    fn set_transparent(&self, _transparent: bool) {}

    fn set_blur(&self, _blur: bool) {}

    fn set_visible(&self, _visibility: bool) {}

    fn is_visible(&self) -> Option<bool> {
        Some(true)
    }

    fn set_resizable(&self, _resizable: bool) {}

    fn is_resizable(&self) -> bool {
        false
    }

    fn set_enabled_buttons(&self, _buttons: WindowButtons) {}

    fn enabled_buttons(&self) -> WindowButtons {
        WindowButtons::all()
    }

    fn set_minimized(&self, _minimized: bool) {}

    fn is_minimized(&self) -> Option<bool> {
        Some(false)
    }

    fn set_maximized(&self, _maximized: bool) {}

    fn is_maximized(&self) -> bool {
        false
    }

    fn set_fullscreen(&self, _monitor: Option<Fullscreen>) {
        warn!("fullscreen requests are controlled by the Android Activity");
    }

    fn fullscreen(&self) -> Option<Fullscreen> {
        None
    }

    fn set_decorations(&self, _decorations: bool) {}

    fn is_decorated(&self) -> bool {
        true
    }

    fn set_window_level(&self, _level: WindowLevel) {}

    fn set_window_icon(&self, _window_icon: Option<winit_core::icon::Icon>) {}

    fn set_ime_cursor_area(&self, _position: Position, _size: Size) {}

    fn request_ime_update(&self, request: ImeRequest) -> Result<(), ImeRequestError> {
        let mut state = self.shared.ime.lock().unwrap();
        match request {
            ImeRequest::Enable(enable) => {
                if state.active_session.is_some() {
                    return Err(ImeRequestError::AlreadyEnabled);
                }
                let (capabilities, request_data) = enable.into_raw();
                if let Some((hint, purpose)) = request_data.hint_and_purpose {
                    state.hint = hint;
                    state.purpose = purpose;
                }
                let session_id = state.configured_session.unwrap_or_else(|| {
                    TextInputSessionId::from_raw(
                        self.shared.next_session.fetch_add(1, Ordering::Relaxed),
                    )
                });
                state.active_session = Some(session_id);
                state.capabilities = Some(capabilities);
                self.shared.queue_ime_command(ImeCommand::Enable {
                    session_id,
                    hint: state.hint,
                    purpose: state.purpose,
                });
            },
            ImeRequest::Update(request_data) => {
                let Some(session_id) = state.active_session else {
                    return Err(ImeRequestError::NotEnabled);
                };
                if let Some((hint, purpose)) = request_data.hint_and_purpose {
                    state.hint = hint;
                    state.purpose = purpose;
                }
                self.shared.queue_ime_command(ImeCommand::Update {
                    session_id,
                    hint: state.hint,
                    purpose: state.purpose,
                });
            },
            ImeRequest::Disable => {
                if let Some(session_id) = state.active_session.take() {
                    state.capabilities = None;
                    self.shared.queue_ime_command(ImeCommand::Disable { session_id });
                }
            },
            _ => return Err(ImeRequestError::NotSupported),
        }
        Ok(())
    }

    fn ime_capabilities(&self) -> Option<ImeCapabilities> {
        self.shared.ime.lock().unwrap().capabilities
    }

    fn set_ime_purpose(&self, purpose: ImePurpose) {
        let mut state = self.shared.ime.lock().unwrap();
        state.purpose = purpose;
        if let Some(session_id) = state.active_session {
            self.shared.queue_ime_command(ImeCommand::Update {
                session_id,
                hint: state.hint,
                purpose,
            });
        }
    }

    fn focus_window(&self) {}

    fn request_user_attention(&self, _request_type: Option<window::UserAttentionType>) {}

    fn set_cursor(&self, _cursor: Cursor) {}

    fn set_cursor_position(&self, _position: Position) -> Result<(), RequestError> {
        Err(NotSupportedError::new("set_cursor_position is not supported").into())
    }

    fn set_cursor_grab(&self, _mode: CursorGrabMode) -> Result<(), RequestError> {
        Err(NotSupportedError::new("set_cursor_grab is not supported").into())
    }

    fn set_cursor_visible(&self, _visible: bool) {}

    fn drag_window(&self) -> Result<(), RequestError> {
        Err(NotSupportedError::new("drag_window is not supported").into())
    }

    fn drag_resize_window(&self, _direction: ResizeDirection) -> Result<(), RequestError> {
        Err(NotSupportedError::new("drag_resize_window is not supported").into())
    }

    fn show_window_menu(&self, _position: Position) {}

    fn set_cursor_hittest(&self, _hittest: bool) -> Result<(), RequestError> {
        Err(NotSupportedError::new("set_cursor_hittest is not supported").into())
    }

    fn set_theme(&self, _theme: Option<Theme>) {}

    fn theme(&self) -> Option<Theme> {
        None
    }

    fn set_content_protected(&self, _protected: bool) {}

    fn has_focus(&self) -> bool {
        self.shared.focused.load(Ordering::Acquire)
    }

    fn title(&self) -> String {
        self.shared.title.lock().unwrap().clone()
    }

    fn reset_dead_keys(&self) {}

    fn rwh_06_display_handle(&self) -> &dyn rwh_06::HasDisplayHandle {
        self
    }

    fn rwh_06_window_handle(&self) -> &dyn rwh_06::HasWindowHandle {
        self
    }
}

pub trait WindowExtJavaActivity {
    /// Sets the application-owned ID to attach to the next enabled text-input connection.
    fn set_text_input_session(&self, session_id: TextInputSessionId);
}

impl WindowExtJavaActivity for dyn CoreWindow + '_ {
    fn set_text_input_session(&self, session_id: TextInputSessionId) {
        let window = self.cast_ref::<Window>().expect("window is not a Java-Activity Android window");
        window.set_text_input_session(session_id);
    }
}
