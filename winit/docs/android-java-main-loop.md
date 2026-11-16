# Experimental Android Java-main-loop backend

Status: design and first API slice. This branch starts at upstream Winit commit
`f0abec90a3f9916818b70cde12fea40b797309ae` (2026-11-13).

## Goal

Run the Winit application and its editor model on Android's UI/Looper thread instead of putting the
application loop on the `NativeActivity` worker thread. Android framework callbacks become short,
synchronous entries into Rust. Java owns the infinite event loop; Rust never starts one.

This is intentionally an opinionated experiment. It is not a compatibility wrapper around
`NativeActivity` or `android-activity`.

## Hard invariants

1. `ApplicationHandler` and application state are owned by the Android UI thread.
2. Every Java-to-Rust callback completes promptly. Rendering, text queries, and input dispatch must
   not block on another application thread.
3. Only one application callback is active at a time.
4. Rust never calls a Java operation that can synchronously call Rust while an application callback
   is on the stack.
5. Android text input queries and edits target the application's real document. There is no hidden
   `EditText` text mirror and no bounded surrounding-text viewport.
6. Text positions crossing the public Winit boundary are UTF-8 byte offsets. The Android bridge
   converts UTF-16 code-unit offsets at the JNI edge.
7. A text session ID rejects callbacks from obsolete `InputConnection` objects. There is no
   per-edit generation or revision protocol when all editor access is serialized on one thread.
8. Surface creation/destruction is explicit. A destroyed Android `Surface` invalidates the WGPU
   surface before the Java callback returns.
9. Pointer timestamps come from `MotionEvent.getEventTimeNanos()` and historical samples are not
   discarded.
10. Back is request/response, not a fire-and-forget key event.

## Callback shape

The Java Activity owns a `SurfaceView`, a `Choreographer.FrameCallback`, lifecycle callbacks,
pointer/key dispatch, Back dispatch, window insets, and an `InputConnection` implementation. Each
entry calls a bounded native function:

- `nativeCreate`
- `nativeStart` / `nativeResume` / `nativePause` / `nativeStop` / `nativeDestroy`
- `nativeSurfaceCreated` / `nativeSurfaceChanged` / `nativeSurfaceDestroyed`
- `nativeFrame`
- `nativePointerBatch`
- `nativeKey`
- `nativeBack` -> handled/default
- synchronous text request functions -> direct answers

`nativeFrame` performs one Winit turn and returns. It is not an event loop.

## Preventing same-thread reentrancy

Using one thread removes races, not nested callback reentrancy. For example, Rust handling a frame
could request IME focus, and Android could synchronously query the new `InputConnection` before the
frame callback unwinds.

Window/platform methods therefore append `PlatformCommand` values to a Rust queue. Java applies a
batch only after the JNI method that produced it has returned:

```text
Java nativeFrame() -> Rust application callback -> return
Java nativeTakeCommands() -> Rust moves command batch out -> return
Java applies commands
```

For `InputConnection` methods, Java returns the synchronous answer to Android first and posts command
application to the next Looper turn. No application borrow and no `Runner` mutable reference crosses
a call into Java that may call back into Rust.

A debug-only callback-depth guard fails immediately if an unexpected nested native application
callback still occurs.

## Text protocol

`ApplicationHandler::text_input_request` is synchronous and session-scoped. Its first slice supports:

- reading an arbitrary document range;
- atomically replacing text and setting selection/composition;
- range-to-rectangle geometry;
- point-to-document-index hit testing.

The Android `Editable`/`InputConnection` bridge will translate between Android UTF-16 code units and
application UTF-8 bytes. Java object spans remain bridge-owned metadata; the Rust document remains
the source of truth for text, selection, and composition.

## Delivery order

1. Public synchronous text and Back contracts in `winit-core`.
2. Java Activity + `SurfaceView` runner that clears a WGPU surface.
3. Lifecycle, surface recreation, insets, and on-demand Choreographer scheduling.
4. Pointer batches with native timestamps/history and handled key/Back paths.
5. Real `InputConnection` backed synchronously by the application text client.
6. Chad experimental integration; remove its hidden `EditText`, NativeActivity callbacks, IME
   viewport/generation protocol, Back workaround, and platform velocity side channel.
7. Notes and Calendar experimental integrations and side-by-side latency/correctness tests.

The old Android backend remains present while this path is proved. No production application branch
switches until the experimental runner can survive lifecycle/surface recreation and pass text-input
instrumentation tests on a hardware-rendered emulator or device.
