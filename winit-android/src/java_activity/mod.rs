//! Experimental Android backend driven by a regular Java `Activity` and its UI Looper.
//!
//! This module deliberately does not start a native polling loop. Java owns the process event loop
//! and invokes short methods on [`Runner`] for lifecycle, surface, frame, input, and text-service
//! callbacks.

mod entry;
mod keycodes;
mod runner;
mod window;

#[doc(hidden)]
pub use entry::{destroy_runner_handle, runner_from_handle_mut, runner_into_handle};
#[doc(hidden)]
pub use dpi as __dpi;
#[doc(hidden)]
pub use winit_core as __winit_core;
pub use jni;
pub use runner::{PointerAction, PointerSample, Runner};
pub use window::{ActiveEventLoopExtJavaActivity, ImeCommand, WindowExtJavaActivity};

/// Exports the JNI entry points consumed by the bundled `WinitActivity` Java class.
///
/// Invoke this exactly once in the Android application library:
///
/// ```ignore
/// winit_android::java_activity_entry!(MyApp, MyApp::default());
/// ```
#[macro_export]
macro_rules! java_activity_entry {
    ($app_ty:ty, $factory:expr) => {
        #[doc(hidden)]
        mod __winit_java_activity_entry {
            use super::*;
            use $crate::java_activity::jni;
            use $crate::java_activity::{PointerAction, PointerSample, Runner};
            use jni::errors::ThrowRuntimeExAndDefault;
            use jni::objects::{JClass, JLongArray, JObject, JString};
            use jni::sys::{jboolean, jdouble, jfloat, jint, jlong, JNI_FALSE, JNI_TRUE};
            use jni::{Env, EnvUnowned};

            type Application = $app_ty;

            fn boolean(value: bool) -> jboolean {
                if value { JNI_TRUE } else { JNI_FALSE }
            }

            fn long_array<'local>(
                env: &mut Env<'local>,
                values: &[jlong],
            ) -> Result<JLongArray<'local>, jni::errors::Error> {
                let array = env.new_long_array(values.len())?;
                array.set_region(env, 0, values)?;
                Ok(array)
            }

            fn state_wire(
                status: jlong,
                state: $crate::java_activity::__winit_core::text_input::TextInputState,
            ) -> [jlong; 6] {
                let (composition_start, composition_end) = state
                    .composition
                    .map_or((-1, -1), |range| (range.start as jlong, range.end as jlong));
                [
                    status,
                    state.text_len as jlong,
                    state.selection.anchor as jlong,
                    state.selection.focus as jlong,
                    composition_start,
                    composition_end,
                ]
            }

            #[unsafe(no_mangle)]
            pub extern "system" fn Java_org_rustwindowing_winit_WinitNative_nativeCreate<'local>(
                mut unowned_env: EnvUnowned<'local>,
                _class: JClass<'local>,
                activity: JObject<'local>,
                files_dir: JString<'local>,
            ) -> jlong {
                unowned_env
                    .with_env(|env| -> Result<jlong, jni::errors::Error> {
                        let files_dir = std::path::PathBuf::from(files_dir.try_to_string(env)?);
                        let runner = Runner::new(env, activity, files_dir, $factory)?;
                        Ok($crate::java_activity::runner_into_handle(runner))
                    })
                    .resolve::<ThrowRuntimeExAndDefault>()
            }

            #[unsafe(no_mangle)]
            pub extern "system" fn Java_org_rustwindowing_winit_WinitNative_nativeDestroy<'local>(
                mut unowned_env: EnvUnowned<'local>,
                _class: JClass<'local>,
                handle: jlong,
            ) {
                unowned_env
                    .with_env(|_env| -> Result<(), jni::errors::Error> {
                        unsafe {
                            $crate::java_activity::destroy_runner_handle::<Application>(handle)
                        };
                        Ok(())
                    })
                    .resolve::<ThrowRuntimeExAndDefault>()
            }

            macro_rules! runner_call {
                ($name:ident, $method:ident) => {
                    #[unsafe(no_mangle)]
                    pub extern "system" fn $name<'local>(
                        mut unowned_env: EnvUnowned<'local>,
                        _class: JClass<'local>,
                        handle: jlong,
                    ) {
                        unowned_env
                            .with_env(|_env| -> Result<(), jni::errors::Error> {
                                unsafe {
                                    $crate::java_activity::runner_from_handle_mut::<Application>(
                                        handle,
                                    )
                                }
                                .$method();
                                Ok(())
                            })
                            .resolve::<ThrowRuntimeExAndDefault>()
                    }
                };
            }

            runner_call!(
                Java_org_rustwindowing_winit_WinitNative_nativeStart,
                start
            );
            runner_call!(Java_org_rustwindowing_winit_WinitNative_nativeStop, stop);
            runner_call!(Java_org_rustwindowing_winit_WinitNative_nativeFrame, frame);
            runner_call!(
                Java_org_rustwindowing_winit_WinitNative_nativeSurfaceDestroyed,
                surface_destroyed
            );

            #[unsafe(no_mangle)]
            pub extern "system" fn Java_org_rustwindowing_winit_WinitNative_nativeSurfaceCreated<
                'local,
            >(
                mut unowned_env: EnvUnowned<'local>,
                _class: JClass<'local>,
                handle: jlong,
                surface: JObject<'local>,
                width: jint,
                height: jint,
                scale_factor: jdouble,
            ) {
                unowned_env
                    .with_env(|env| -> Result<(), jni::errors::Error> {
                        unsafe {
                            $crate::java_activity::runner_from_handle_mut::<Application>(handle)
                                .surface_created(
                                    env,
                                    surface,
                                    width.max(0) as u32,
                                    height.max(0) as u32,
                                    scale_factor,
                                )
                                .expect("failed to install Android Surface")
                        };
                        Ok(())
                    })
                    .resolve::<ThrowRuntimeExAndDefault>()
            }

            #[unsafe(no_mangle)]
            pub extern "system" fn Java_org_rustwindowing_winit_WinitNative_nativeSurfaceChanged<
                'local,
            >(
                mut unowned_env: EnvUnowned<'local>,
                _class: JClass<'local>,
                handle: jlong,
                width: jint,
                height: jint,
                scale_factor: jdouble,
            ) {
                unowned_env
                    .with_env(|_env| -> Result<(), jni::errors::Error> {
                        unsafe {
                            $crate::java_activity::runner_from_handle_mut::<Application>(handle)
                        }
                        .surface_changed(
                            width.max(0) as u32,
                            height.max(0) as u32,
                            scale_factor,
                        );
                        Ok(())
                    })
                    .resolve::<ThrowRuntimeExAndDefault>()
            }

            #[unsafe(no_mangle)]
            pub extern "system" fn Java_org_rustwindowing_winit_WinitNative_nativeFocus<'local>(
                mut unowned_env: EnvUnowned<'local>,
                _class: JClass<'local>,
                handle: jlong,
                focused: jboolean,
            ) {
                unowned_env
                    .with_env(|_env| -> Result<(), jni::errors::Error> {
                        unsafe {
                            $crate::java_activity::runner_from_handle_mut::<Application>(handle)
                        }
                        .set_focus(focused != JNI_FALSE);
                        Ok(())
                    })
                    .resolve::<ThrowRuntimeExAndDefault>()
            }

            #[unsafe(no_mangle)]
            pub extern "system" fn Java_org_rustwindowing_winit_WinitNative_nativeInsets<'local>(
                mut unowned_env: EnvUnowned<'local>,
                _class: JClass<'local>,
                handle: jlong,
                left: jint,
                top: jint,
                right: jint,
                bottom: jint,
                keyboard_visible: jboolean,
            ) {
                unowned_env
                    .with_env(|_env| -> Result<(), jni::errors::Error> {
                        unsafe {
                            $crate::java_activity::runner_from_handle_mut::<Application>(handle)
                        }
                        .set_insets(
                            left.max(0) as u32,
                            top.max(0) as u32,
                            right.max(0) as u32,
                            bottom.max(0) as u32,
                            keyboard_visible != JNI_FALSE,
                        );
                        Ok(())
                    })
                    .resolve::<ThrowRuntimeExAndDefault>()
            }

            #[unsafe(no_mangle)]
            pub extern "system" fn Java_org_rustwindowing_winit_WinitNative_nativePointer<'local>(
                mut unowned_env: EnvUnowned<'local>,
                _class: JClass<'local>,
                handle: jlong,
                action: jint,
                time_nanos: jlong,
                device_id: jint,
                pointer_id: jint,
                x: jfloat,
                y: jfloat,
                pressure: jfloat,
            ) -> jboolean {
                unowned_env
                    .with_env(|_env| -> Result<jboolean, jni::errors::Error> {
                        let action = match action {
                            0 => PointerAction::Down,
                            1 => PointerAction::Move,
                            2 => PointerAction::Up,
                            3 => PointerAction::Cancel,
                            _ => panic!("unknown Android pointer action {action}"),
                        };
                        let sample = PointerSample {
                            time_nanos: time_nanos.max(0) as u64,
                            device_id: device_id as i64,
                            pointer_id: pointer_id.max(0) as usize,
                            position: $crate::java_activity::__dpi::PhysicalPosition::new(
                                x as f64,
                                y as f64,
                            ),
                            pressure: pressure.clamp(0.0, 1.0) as f64,
                        };
                        let handled = unsafe {
                            $crate::java_activity::runner_from_handle_mut::<Application>(handle)
                        }
                        .pointer_batch(action, &[sample]);
                        Ok(boolean(handled))
                    })
                    .resolve::<ThrowRuntimeExAndDefault>()
            }

            #[unsafe(no_mangle)]
            pub extern "system" fn Java_org_rustwindowing_winit_WinitNative_nativeKey<'local>(
                mut unowned_env: EnvUnowned<'local>,
                _class: JClass<'local>,
                handle: jlong,
                pressed: jboolean,
                device_id: jint,
                keycode: jint,
                unicode: jlong,
                unicode_without_modifiers: jlong,
                repeat: jboolean,
            ) -> jboolean {
                unowned_env
                    .with_env(|_env| -> Result<jboolean, jni::errors::Error> {
                        let response = unsafe {
                            $crate::java_activity::runner_from_handle_mut::<Application>(handle)
                        }
                        .keyboard_input(
                            pressed != JNI_FALSE,
                            device_id as i64,
                            keycode.max(0) as u32,
                            unicode.max(0) as u32,
                            unicode_without_modifiers.max(0) as u32,
                            repeat != JNI_FALSE,
                        );
                        Ok(boolean(matches!(
                            response,
                            $crate::java_activity::__winit_core::application::InputResponse::Handled
                        )))
                    })
                    .resolve::<ThrowRuntimeExAndDefault>()
            }

            #[unsafe(no_mangle)]
            pub extern "system" fn Java_org_rustwindowing_winit_WinitNative_nativeBack<'local>(
                mut unowned_env: EnvUnowned<'local>,
                _class: JClass<'local>,
                handle: jlong,
            ) -> jboolean {
                unowned_env
                    .with_env(|_env| -> Result<jboolean, jni::errors::Error> {
                        let response = unsafe {
                            $crate::java_activity::runner_from_handle_mut::<Application>(handle)
                        }
                        .back_requested();
                        Ok(boolean(matches!(
                            response,
                            $crate::java_activity::__winit_core::application::BackResponse::Handled
                        )))
                    })
                    .resolve::<ThrowRuntimeExAndDefault>()
            }

            #[unsafe(no_mangle)]
            pub extern "system" fn Java_org_rustwindowing_winit_WinitNative_nativeTakeWork<'local>(
                mut unowned_env: EnvUnowned<'local>,
                _class: JClass<'local>,
                handle: jlong,
            ) -> jboolean {
                unowned_env
                    .with_env(|_env| -> Result<jboolean, jni::errors::Error> {
                        Ok(boolean(
                            unsafe {
                                $crate::java_activity::runner_from_handle_mut::<Application>(handle)
                            }
                            .take_work_pending(),
                        ))
                    })
                    .resolve::<ThrowRuntimeExAndDefault>()
            }

            #[unsafe(no_mangle)]
            pub extern "system" fn Java_org_rustwindowing_winit_WinitNative_nativeExiting<'local>(
                mut unowned_env: EnvUnowned<'local>,
                _class: JClass<'local>,
                handle: jlong,
            ) -> jboolean {
                unowned_env
                    .with_env(|_env| -> Result<jboolean, jni::errors::Error> {
                        Ok(boolean(
                            unsafe {
                                $crate::java_activity::runner_from_handle_mut::<Application>(handle)
                            }
                            .exiting(),
                        ))
                    })
                    .resolve::<ThrowRuntimeExAndDefault>()
            }

            #[unsafe(no_mangle)]
            pub extern "system" fn Java_org_rustwindowing_winit_WinitNative_nativeTakeImeCommand<
                'local,
            >(
                mut unowned_env: EnvUnowned<'local>,
                _class: JClass<'local>,
                handle: jlong,
            ) -> JLongArray<'local> {
                unowned_env
                    .with_env(|env| -> Result<JLongArray<'local>, jni::errors::Error> {
                        let values = unsafe {
                            $crate::java_activity::runner_from_handle_mut::<Application>(handle)
                        }
                        .take_ime_command()
                        .map(|command| command.into_wire());
                        match values {
                            Some(values) => long_array(env, &values),
                            None => long_array(env, &[]),
                        }
                    })
                    .resolve::<ThrowRuntimeExAndDefault>()
            }

            #[unsafe(no_mangle)]
            pub extern "system" fn Java_org_rustwindowing_winit_WinitNative_nativeTextState<
                'local,
            >(
                mut unowned_env: EnvUnowned<'local>,
                _class: JClass<'local>,
                handle: jlong,
                session_id: jlong,
            ) -> JLongArray<'local> {
                unowned_env
                    .with_env(|env| -> Result<JLongArray<'local>, jni::errors::Error> {
                        use $crate::java_activity::__winit_core::text_input::{
                            TextInputRequest, TextInputResponse, TextInputSessionId,
                        };
                        let response = unsafe {
                            $crate::java_activity::runner_from_handle_mut::<Application>(handle)
                        }
                        .text_input_request(
                            TextInputSessionId::from_raw(session_id.max(0) as u64),
                            TextInputRequest::State,
                        );
                        match response {
                            TextInputResponse::State(state) => {
                                long_array(env, &state_wire(1, state))
                            },
                            _ => long_array(env, &[]),
                        }
                    })
                    .resolve::<ThrowRuntimeExAndDefault>()
            }

            #[unsafe(no_mangle)]
            pub extern "system" fn Java_org_rustwindowing_winit_WinitNative_nativeTextRead<'local>(
                mut unowned_env: EnvUnowned<'local>,
                _class: JClass<'local>,
                handle: jlong,
                session_id: jlong,
                start: jlong,
                end: jlong,
            ) -> JString<'local> {
                unowned_env
                    .with_env(|env| -> Result<JString<'local>, jni::errors::Error> {
                        use $crate::java_activity::__winit_core::text_input::{
                            TextInputRequest, TextInputResponse, TextInputSessionId,
                        };
                        if start < 0 || end < start {
                            return Ok(JString::default());
                        }
                        let response = unsafe {
                            $crate::java_activity::runner_from_handle_mut::<Application>(handle)
                        }
                        .text_input_request(
                            TextInputSessionId::from_raw(session_id.max(0) as u64),
                            TextInputRequest::Read { range: start as usize..end as usize },
                        );
                        match response {
                            TextInputResponse::Text(snapshot) => {
                                JString::from_str(env, snapshot.text)
                            },
                            _ => Ok(JString::default()),
                        }
                    })
                    .resolve::<ThrowRuntimeExAndDefault>()
            }

            #[unsafe(no_mangle)]
            pub extern "system" fn Java_org_rustwindowing_winit_WinitNative_nativeTextApply<
                'local,
            >(
                mut unowned_env: EnvUnowned<'local>,
                _class: JClass<'local>,
                handle: jlong,
                session_id: jlong,
                has_replacement: jboolean,
                replacement_start: jlong,
                replacement_end: jlong,
                replacement_text: JString<'local>,
                selection_anchor: jlong,
                selection_focus: jlong,
                composition_start: jlong,
                composition_end: jlong,
            ) -> JLongArray<'local> {
                unowned_env
                    .with_env(|env| -> Result<JLongArray<'local>, jni::errors::Error> {
                        use $crate::java_activity::__winit_core::text_input::{
                            TextInputApplyResult, TextInputEdit, TextInputRequest,
                            TextInputResponse, TextInputSessionId, TextReplacement, TextSelection,
                        };
                        let replacement = if has_replacement != JNI_FALSE {
                            assert!(
                                replacement_start >= 0 && replacement_end >= replacement_start,
                                "invalid text replacement range"
                            );
                            Some(TextReplacement {
                                range: replacement_start as usize..replacement_end as usize,
                                text: replacement_text.try_to_string(env)?,
                            })
                        } else {
                            None
                        };
                        assert!(selection_anchor >= 0 && selection_focus >= 0);
                        let composition = if composition_start < 0 {
                            None
                        } else {
                            assert!(composition_end >= composition_start);
                            Some(composition_start as usize..composition_end as usize)
                        };
                        let response = unsafe {
                            $crate::java_activity::runner_from_handle_mut::<Application>(handle)
                        }
                        .text_input_request(
                            TextInputSessionId::from_raw(session_id.max(0) as u64),
                            TextInputRequest::Apply(TextInputEdit {
                                replacement,
                                selection: TextSelection::new(
                                    selection_anchor as usize,
                                    selection_focus as usize,
                                ),
                                composition,
                            }),
                        );
                        match response {
                            TextInputResponse::Applied(TextInputApplyResult::Applied(state)) => {
                                long_array(env, &state_wire(1, state))
                            },
                            TextInputResponse::Applied(TextInputApplyResult::Rejected(state)) => {
                                long_array(env, &state_wire(2, state))
                            },
                            TextInputResponse::Applied(TextInputApplyResult::StaleSession) => {
                                long_array(env, &[3])
                            },
                            _ => long_array(env, &[]),
                        }
                    })
                    .resolve::<ThrowRuntimeExAndDefault>()
            }
        }
    };
}
