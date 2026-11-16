use jni::sys::jlong;
use winit_core::application::ApplicationHandler;

use super::Runner;

/// Converts an owned runner into the opaque handle stored by `WinitActivity`.
#[doc(hidden)]
pub fn runner_into_handle<A: ApplicationHandler>(runner: Runner<A>) -> jlong {
    Box::into_raw(Box::new(runner)) as jlong
}

/// Borrows the runner behind a Java-owned opaque handle.
///
/// # Safety
/// The handle must have been returned by [`runner_into_handle`] for the same `A`, must not have
/// been destroyed, and access must be serialized on the creating UI thread.
#[doc(hidden)]
pub unsafe fn runner_from_handle_mut<'a, A: ApplicationHandler>(
    handle: jlong,
) -> &'a mut Runner<A> {
    assert_ne!(handle, 0, "null Java Activity runner handle");
    unsafe { &mut *(handle as *mut Runner<A>) }
}

/// Reclaims and destroys a Java-owned runner.
///
/// # Safety
/// The handle must have been returned by [`runner_into_handle`] for the same `A`, and this function
/// must be called exactly once.
#[doc(hidden)]
pub unsafe fn destroy_runner_handle<A: ApplicationHandler>(handle: jlong) {
    assert_ne!(handle, 0, "null Java Activity runner handle");
    let runner = unsafe { Box::from_raw(handle as *mut Runner<A>) };
    runner.destroy();
}
