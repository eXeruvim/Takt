#![deny(clippy::undocumented_unsafe_blocks)]

use std::{
    ffi::{CStr, c_void},
    fmt,
    marker::PhantomData,
    os::raw::{c_char, c_int},
    panic::{AssertUnwindSafe, catch_unwind},
    ptr::NonNull,
    rc::Rc,
    sync::Arc,
};

use anyhow::{Result, anyhow};
use libmpv::Mpv;
use libmpv_sys::{
    MPV_RENDER_API_TYPE_OPENGL, mpv_opengl_fbo, mpv_opengl_init_params, mpv_render_context,
    mpv_render_context_create, mpv_render_context_free, mpv_render_context_render,
    mpv_render_context_report_swap, mpv_render_context_set_update_callback,
    mpv_render_context_update, mpv_render_param, mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
    mpv_render_param_type_MPV_RENDER_PARAM_FLIP_Y, mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
    mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_FBO,
    mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
};

#[derive(Clone)]
pub struct MpvRenderSource {
    mpv: Arc<Mpv>,
}

impl MpvRenderSource {
    pub fn new(mpv: Arc<Mpv>) -> Self {
        Self { mpv }
    }

    fn handle(&self) -> *mut libmpv_sys::mpv_handle {
        self.mpv.ctx.as_ptr()
    }
}

impl fmt::Debug for MpvRenderSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MpvRenderSource")
            .finish_non_exhaustive()
    }
}

struct ProcAddressLoader<'a> {
    get_proc_address: &'a dyn Fn(&CStr) -> *const c_void,
}

unsafe extern "C" fn load_opengl_proc(context: *mut c_void, name: *const c_char) -> *mut c_void {
    if context.is_null() || name.is_null() {
        return std::ptr::null_mut();
    }

    // SAFETY: mpv receives a pointer to the stack-owned loader only for the
    // duration of mpv_render_context_create(). Render parameters are borrowed
    // for that call, and create() does not return until this callback finishes.
    let loader = unsafe { &*(context.cast::<ProcAddressLoader<'_>>()) };
    // SAFETY: mpv documents `name` as a non-null, NUL-terminated GL symbol for
    // the duration of this callback.
    let name = unsafe { CStr::from_ptr(name) };
    catch_unwind(AssertUnwindSafe(|| {
        (loader.get_proc_address)(name).cast_mut()
    }))
    .unwrap_or(std::ptr::null_mut())
}

struct UpdateTarget {
    wake: Box<dyn Fn() + Send + Sync>,
}

type GlGetIntegerv = unsafe extern "system" fn(u32, *mut i32);
type GlGetFloatv = unsafe extern "system" fn(u32, *mut f32);
type GlBindFramebuffer = unsafe extern "system" fn(u32, u32);
type GlViewport = unsafe extern "system" fn(i32, i32, i32, i32);
type GlScissor = unsafe extern "system" fn(i32, i32, i32, i32);
type GlBlendFuncSeparate = unsafe extern "system" fn(u32, u32, u32, u32);
type GlClearColor = unsafe extern "system" fn(f32, f32, f32, f32);
type GlIsEnabled = unsafe extern "system" fn(u32) -> u8;
type GlSetCapability = unsafe extern "system" fn(u32);
type GlActiveTexture = unsafe extern "system" fn(u32);
type GlBindBuffer = unsafe extern "system" fn(u32, u32);
type GlBindTexture = unsafe extern "system" fn(u32, u32);
type GlBindVertexArray = unsafe extern "system" fn(u32);
type GlColorMask = unsafe extern "system" fn(u8, u8, u8, u8);
type GlDepthMask = unsafe extern "system" fn(u8);
type GlStencilMask = unsafe extern "system" fn(u32);
type GlUseProgram = unsafe extern "system" fn(u32);

const GL_ARRAY_BUFFER: u32 = 0x8892;
const GL_BLEND: u32 = 0x0BE2;
const GL_BLEND_DST_ALPHA: u32 = 0x80CA;
const GL_BLEND_DST_RGB: u32 = 0x80C8;
const GL_BLEND_SRC_ALPHA: u32 = 0x80CB;
const GL_BLEND_SRC_RGB: u32 = 0x80C9;
const GL_COLOR_CLEAR_VALUE: u32 = 0x0C22;
const GL_CULL_FACE: u32 = 0x0B44;
const GL_DEPTH_TEST: u32 = 0x0B71;
const GL_DITHER: u32 = 0x0BD0;
const GL_ONE: u32 = 1;
const GL_SCISSOR_TEST: u32 = 0x0C11;
const GL_SCISSOR_BOX: u32 = 0x0C10;
const GL_STENCIL_TEST: u32 = 0x0B90;
const GL_TEXTURE0: u32 = 0x84C0;
const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_VIEWPORT: u32 = 0x0BA2;
const GL_ZERO: u32 = 0;
const GL_DRAW_FRAMEBUFFER: u32 = 0x8CA9;
const GL_READ_FRAMEBUFFER: u32 = 0x8CA8;
const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
const GL_READ_FRAMEBUFFER_BINDING: u32 = 0x8CAA;

struct GlStateApi {
    get_integer: GlGetIntegerv,
    get_float: GlGetFloatv,
    bind_framebuffer: GlBindFramebuffer,
    viewport: GlViewport,
    scissor: GlScissor,
    blend_func_separate: GlBlendFuncSeparate,
    clear_color: GlClearColor,
    is_enabled: GlIsEnabled,
    enable: GlSetCapability,
    disable: GlSetCapability,
    active_texture: GlActiveTexture,
    bind_buffer: GlBindBuffer,
    bind_texture: GlBindTexture,
    bind_vertex_array: GlBindVertexArray,
    color_mask: GlColorMask,
    depth_mask: GlDepthMask,
    stencil_mask: GlStencilMask,
    use_program: GlUseProgram,
}

impl GlStateApi {
    fn load(get_proc_address: &dyn Fn(&CStr) -> *const c_void) -> Result<Self> {
        let load = |name: &CStr| {
            let address = get_proc_address(name);
            (!address.is_null())
                .then_some(address)
                .ok_or_else(|| anyhow!("OpenGL function {} is unavailable", name.to_string_lossy()))
        };

        Ok(Self {
            // SAFETY: Each address was checked for null and comes from the
            // current Slint OpenGL context for the exact named function. On
            // Windows `extern "system"` matches OpenGL's APIENTRY convention.
            get_integer: unsafe {
                std::mem::transmute::<*const c_void, GlGetIntegerv>(load(c"glGetIntegerv")?)
            },
            // SAFETY: Same checked loader and OpenGL ABI contract as above.
            get_float: unsafe {
                std::mem::transmute::<*const c_void, GlGetFloatv>(load(c"glGetFloatv")?)
            },
            // SAFETY: Same checked loader and OpenGL ABI contract as above.
            bind_framebuffer: unsafe {
                std::mem::transmute::<*const c_void, GlBindFramebuffer>(load(c"glBindFramebuffer")?)
            },
            // SAFETY: Same checked loader and OpenGL ABI contract as above.
            viewport: unsafe {
                std::mem::transmute::<*const c_void, GlViewport>(load(c"glViewport")?)
            },
            // SAFETY: Same checked loader and OpenGL ABI contract as above.
            scissor: unsafe {
                std::mem::transmute::<*const c_void, GlScissor>(load(c"glScissor")?)
            },
            // SAFETY: Same checked loader and OpenGL ABI contract as above.
            blend_func_separate: unsafe {
                std::mem::transmute::<*const c_void, GlBlendFuncSeparate>(load(
                    c"glBlendFuncSeparate",
                )?)
            },
            // SAFETY: Same checked loader and OpenGL ABI contract as above.
            clear_color: unsafe {
                std::mem::transmute::<*const c_void, GlClearColor>(load(c"glClearColor")?)
            },
            // SAFETY: Same checked loader and OpenGL ABI contract as above.
            is_enabled: unsafe {
                std::mem::transmute::<*const c_void, GlIsEnabled>(load(c"glIsEnabled")?)
            },
            // SAFETY: Same checked loader and OpenGL ABI contract as above.
            enable: unsafe {
                std::mem::transmute::<*const c_void, GlSetCapability>(load(c"glEnable")?)
            },
            // SAFETY: Same checked loader and OpenGL ABI contract as above.
            disable: unsafe {
                std::mem::transmute::<*const c_void, GlSetCapability>(load(c"glDisable")?)
            },
            // SAFETY: Same checked loader and OpenGL ABI contract as above.
            active_texture: unsafe {
                std::mem::transmute::<*const c_void, GlActiveTexture>(load(c"glActiveTexture")?)
            },
            // SAFETY: Same checked loader and OpenGL ABI contract as above.
            bind_buffer: unsafe {
                std::mem::transmute::<*const c_void, GlBindBuffer>(load(c"glBindBuffer")?)
            },
            // SAFETY: Same checked loader and OpenGL ABI contract as above.
            bind_texture: unsafe {
                std::mem::transmute::<*const c_void, GlBindTexture>(load(c"glBindTexture")?)
            },
            // SAFETY: Same checked loader and OpenGL ABI contract as above.
            bind_vertex_array: unsafe {
                std::mem::transmute::<*const c_void, GlBindVertexArray>(load(c"glBindVertexArray")?)
            },
            // SAFETY: Same checked loader and OpenGL ABI contract as above.
            color_mask: unsafe {
                std::mem::transmute::<*const c_void, GlColorMask>(load(c"glColorMask")?)
            },
            // SAFETY: Same checked loader and OpenGL ABI contract as above.
            depth_mask: unsafe {
                std::mem::transmute::<*const c_void, GlDepthMask>(load(c"glDepthMask")?)
            },
            // SAFETY: Same checked loader and OpenGL ABI contract as above.
            stencil_mask: unsafe {
                std::mem::transmute::<*const c_void, GlStencilMask>(load(c"glStencilMask")?)
            },
            // SAFETY: Same checked loader and OpenGL ABI contract as above.
            use_program: unsafe {
                std::mem::transmute::<*const c_void, GlUseProgram>(load(c"glUseProgram")?)
            },
        })
    }

    fn prepare_for_mpv(&self, size: Option<(i32, i32)>) {
        // FemtoVG and libmpv share one implicit OpenGL context. FemtoVG leaves
        // blending enabled and texture unit 1 active after flushing the Slint
        // background, while libmpv's render API explicitly requires standard
        // OpenGL defaults on entry. Hardware copy-back frames exposed the
        // mismatch as visible corruption even though software frames happened
        // to tolerate it. The surrounding snapshot restores the exceptional
        // state that Slint needs after mpv returns.
        //
        // SAFETY: Every function belongs to the current Slint OpenGL context.
        // The calls use only standard default values and retain no Rust memory.
        unsafe {
            (self.use_program)(0);
            (self.bind_vertex_array)(0);
            (self.bind_buffer)(GL_ARRAY_BUFFER, 0);
            (self.active_texture)(GL_TEXTURE0);
            (self.bind_texture)(GL_TEXTURE_2D, 0);
            (self.disable)(GL_BLEND);
            (self.disable)(GL_CULL_FACE);
            (self.disable)(GL_DEPTH_TEST);
            (self.disable)(GL_SCISSOR_TEST);
            (self.disable)(GL_STENCIL_TEST);
            (self.disable)(GL_DITHER);
            (self.blend_func_separate)(GL_ONE, GL_ZERO, GL_ONE, GL_ZERO);
            (self.color_mask)(1, 1, 1, 1);
            (self.depth_mask)(1);
            (self.stencil_mask)(u32::MAX);
            (self.clear_color)(0.0, 0.0, 0.0, 0.0);
            if let Some((width, height)) = size {
                (self.viewport)(0, 0, width, height);
                (self.scissor)(0, 0, width, height);
            }
        }
    }

    fn capture(&self) -> GlStateSnapshot<'_> {
        let mut draw_framebuffer = 0;
        let mut read_framebuffer = 0;
        let mut viewport = [0; 4];
        let mut scissor = [0; 4];
        let mut blend_src_rgb = 0;
        let mut blend_dst_rgb = 0;
        let mut blend_src_alpha = 0;
        let mut blend_dst_alpha = 0;
        let mut clear_color = [0.0; 4];
        // SAFETY: The renderer can only be created through its unsafe
        // constructor, whose caller guarantees that its original OpenGL
        // context is current. All output pointers refer to initialized local
        // storage of the size required by the queried enum.
        unsafe {
            (self.get_integer)(GL_DRAW_FRAMEBUFFER_BINDING, &mut draw_framebuffer);
            (self.get_integer)(GL_READ_FRAMEBUFFER_BINDING, &mut read_framebuffer);
            (self.get_integer)(GL_VIEWPORT, viewport.as_mut_ptr());
            (self.get_integer)(GL_SCISSOR_BOX, scissor.as_mut_ptr());
            (self.get_integer)(GL_BLEND_SRC_RGB, &mut blend_src_rgb);
            (self.get_integer)(GL_BLEND_DST_RGB, &mut blend_dst_rgb);
            (self.get_integer)(GL_BLEND_SRC_ALPHA, &mut blend_src_alpha);
            (self.get_integer)(GL_BLEND_DST_ALPHA, &mut blend_dst_alpha);
            (self.get_float)(GL_COLOR_CLEAR_VALUE, clear_color.as_mut_ptr());
        }
        // SAFETY: Same current-context invariant as above; glIsEnabled does not
        // retain the enum value or access Rust memory.
        let scissor_enabled = unsafe { (self.is_enabled)(GL_SCISSOR_TEST) != 0 };
        // SAFETY: Same current-context invariant as above.
        let blend_enabled = unsafe { (self.is_enabled)(GL_BLEND) != 0 };
        // SAFETY: Same current-context invariant as above.
        let dither_enabled = unsafe { (self.is_enabled)(GL_DITHER) != 0 };

        GlStateSnapshot {
            api: self,
            draw_framebuffer,
            read_framebuffer,
            viewport,
            scissor,
            scissor_enabled,
            blend_src_rgb,
            blend_dst_rgb,
            blend_src_alpha,
            blend_dst_alpha,
            blend_enabled,
            clear_color,
            dither_enabled,
        }
    }
}

struct GlStateSnapshot<'a> {
    api: &'a GlStateApi,
    draw_framebuffer: i32,
    read_framebuffer: i32,
    viewport: [i32; 4],
    scissor: [i32; 4],
    scissor_enabled: bool,
    blend_src_rgb: i32,
    blend_dst_rgb: i32,
    blend_src_alpha: i32,
    blend_dst_alpha: i32,
    blend_enabled: bool,
    clear_color: [f32; 4],
    dither_enabled: bool,
}

impl Drop for GlStateSnapshot<'_> {
    fn drop(&mut self) {
        // SAFETY: The snapshot is scoped entirely inside one render() call, so
        // the same OpenGL context remains current and all captured values are
        // valid state for that context.
        unsafe {
            (self.api.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, self.draw_framebuffer as u32);
            (self.api.bind_framebuffer)(GL_READ_FRAMEBUFFER, self.read_framebuffer as u32);
            (self.api.viewport)(
                self.viewport[0],
                self.viewport[1],
                self.viewport[2],
                self.viewport[3],
            );
            (self.api.scissor)(
                self.scissor[0],
                self.scissor[1],
                self.scissor[2],
                self.scissor[3],
            );
            (self.api.blend_func_separate)(
                self.blend_src_rgb as u32,
                self.blend_dst_rgb as u32,
                self.blend_src_alpha as u32,
                self.blend_dst_alpha as u32,
            );
            (self.api.clear_color)(
                self.clear_color[0],
                self.clear_color[1],
                self.clear_color[2],
                self.clear_color[3],
            );
            if self.scissor_enabled {
                (self.api.enable)(GL_SCISSOR_TEST);
            } else {
                (self.api.disable)(GL_SCISSOR_TEST);
            }
            if self.blend_enabled {
                (self.api.enable)(GL_BLEND);
            } else {
                (self.api.disable)(GL_BLEND);
            }
            if self.dither_enabled {
                (self.api.enable)(GL_DITHER);
            } else {
                (self.api.disable)(GL_DITHER);
            }
        }
    }
}

unsafe extern "C" fn request_redraw(context: *mut c_void) {
    if context.is_null() {
        return;
    }

    // SAFETY: The pointer refers to MpvOpenGlRenderer::update_target. Drop
    // unregisters the callback and frees the mpv render context before the Box
    // is released, so the allocation outlives every registered callback.
    let target = unsafe { &*(context.cast::<UpdateTarget>()) };
    // A panic must never unwind through libmpv's C callback boundary.
    let _ = catch_unwind(AssertUnwindSafe(|| (target.wake)()));
}

/// A renderer tied to the OpenGL context and thread supplied by Slint.
///
/// The `Rc` marker deliberately makes this type `!Send` and `!Sync`: mpv requires
/// every render operation and destruction to happen with the same current GL
/// context. Oxide owns it only inside Slint's rendering notifier.
pub struct MpvOpenGlRenderer {
    context: NonNull<mpv_render_context>,
    _update_target: Box<UpdateTarget>,
    gl_state: GlStateApi,
    _source: MpvRenderSource,
    _render_thread: PhantomData<Rc<()>>,
}

impl MpvOpenGlRenderer {
    /// Creates the libmpv render context for the current OpenGL context.
    ///
    /// # Safety
    ///
    /// The caller must invoke this from Slint's rendering notifier while its
    /// OpenGL context is current. The same context and thread must be current
    /// for every call to [`Self::render`], [`Self::report_swap`], and when this
    /// value is dropped. `get_proc_address` must return addresses with the ABI
    /// and lifetime required by that context.
    pub unsafe fn create(
        source: MpvRenderSource,
        get_proc_address: &dyn Fn(&CStr) -> *const c_void,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self> {
        let gl_state = GlStateApi::load(get_proc_address)?;
        let loader = ProcAddressLoader { get_proc_address };
        let mut init = mpv_opengl_init_params {
            get_proc_address: Some(load_opengl_proc),
            get_proc_address_ctx: std::ptr::from_ref(&loader).cast_mut().cast(),
        };
        let mut context = std::ptr::null_mut();
        let mut parameters = [
            mpv_render_param {
                type_: mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
                data: MPV_RENDER_API_TYPE_OPENGL.as_ptr().cast_mut().cast(),
            },
            mpv_render_param {
                type_: mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                data: std::ptr::from_mut(&mut init).cast(),
            },
            mpv_render_param {
                type_: mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];

        let result = {
            // mpv 0.41 explicitly excludes viewport, scissor, blend factors,
            // clear color, and dithering from its state-reset guarantee. This
            // guard also protects Slint from initialization-time changes.
            let _state = gl_state.capture();
            gl_state.prepare_for_mpv(None);
            // SAFETY: The caller upholds the current-context requirement. Every
            // parameter and the loader context remain valid until the synchronous
            // create call returns, and the array is terminated by INVALID.
            unsafe {
                mpv_render_context_create(&mut context, source.handle(), parameters.as_mut_ptr())
            }
        };
        if result < 0 {
            return Err(anyhow!(
                "mpv render context initialization failed ({result})"
            ));
        }
        let context =
            NonNull::new(context).ok_or_else(|| anyhow!("mpv returned an empty render context"))?;

        let mut update_target = Box::new(UpdateTarget {
            wake: Box::new(wake),
        });
        let update_target_ptr = NonNull::from(update_target.as_mut());
        // SAFETY: The Box keeps a stable allocation until Drop. Setting the
        // callback may invoke it immediately, after the allocation is ready.
        unsafe {
            mpv_render_context_set_update_callback(
                context.as_ptr(),
                Some(request_redraw),
                update_target_ptr.as_ptr().cast(),
            );
        }

        Ok(Self {
            context,
            _update_target: update_target,
            gl_state,
            _source: source,
            _render_thread: PhantomData,
        })
    }

    /// Renders into Slint's current default framebuffer.
    ///
    /// # Safety
    ///
    /// The OpenGL context used by [`Self::create`] must be current on this
    /// thread for the entire call.
    pub unsafe fn render(&mut self, size: (u32, u32)) -> Result<()> {
        let _state = self.gl_state.capture();
        let width = c_int::try_from(size.0).unwrap_or(c_int::MAX).max(1);
        let height = c_int::try_from(size.1).unwrap_or(c_int::MAX).max(1);
        self.gl_state.prepare_for_mpv(Some((width, height)));
        let mut framebuffer = mpv_opengl_fbo {
            fbo: 0,
            w: width,
            h: height,
            internal_format: 0,
        };
        let mut flip_y: c_int = 1;
        let mut parameters = [
            mpv_render_param {
                type_: mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_FBO,
                data: std::ptr::from_mut(&mut framebuffer).cast(),
            },
            mpv_render_param {
                type_: mpv_render_param_type_MPV_RENDER_PARAM_FLIP_Y,
                data: std::ptr::from_mut(&mut flip_y).cast(),
            },
            mpv_render_param {
                type_: mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];

        // SAFETY: The caller guarantees the original context is current; the
        // render context is owned by self and still alive.
        unsafe {
            mpv_render_context_update(self.context.as_ptr());
        }
        // SAFETY: The parameter array and pointed-to FBO/flip values live for
        // the synchronous call and are terminated by INVALID.
        let result =
            unsafe { mpv_render_context_render(self.context.as_ptr(), parameters.as_mut_ptr()) };
        if result < 0 {
            return Err(anyhow!("mpv OpenGL render failed ({result})"));
        }
        Ok(())
    }

    /// Reports the presentation boundary to libmpv.
    ///
    /// # Safety
    ///
    /// The same OpenGL context used by [`Self::create`] must be current.
    pub unsafe fn report_swap(&self) {
        // SAFETY: The caller upholds the current-context invariant and self
        // owns a live render context.
        unsafe {
            mpv_render_context_report_swap(self.context.as_ptr());
        }
    }
}

impl Drop for MpvOpenGlRenderer {
    fn drop(&mut self) {
        // mpv can touch the same excluded OpenGL state while releasing GPU
        // resources. Preserve it for the Slint teardown path as well.
        let _state = self.gl_state.capture();
        self.gl_state.prepare_for_mpv(None);
        // SAFETY: The type is !Send and Oxide drops it only from Slint's
        // RenderingTeardown (or a render failure), where Slint keeps the same
        // OpenGL context current. The callback is removed first. The Box stays
        // alive through context_free, covering a synchronous final callback.
        unsafe {
            mpv_render_context_set_update_callback(
                self.context.as_ptr(),
                None,
                std::ptr::null_mut(),
            );
            mpv_render_context_free(self.context.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::{ProcAddressLoader, UpdateTarget, load_opengl_proc, request_redraw};

    #[test]
    fn ffi_callbacks_accept_null_contexts() {
        // SAFETY: The callbacks explicitly accept and ignore null contexts.
        unsafe {
            request_redraw(std::ptr::null_mut());
            assert!(load_opengl_proc(std::ptr::null_mut(), c"glGetIntegerv".as_ptr()).is_null());
        }
    }

    #[test]
    fn redraw_callback_invokes_target_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let mut target = UpdateTarget {
            wake: Box::new(move || {
                observed.fetch_add(1, Ordering::Relaxed);
            }),
        };

        // SAFETY: `target` remains alive and immovable for the synchronous call.
        unsafe {
            request_redraw(std::ptr::from_mut(&mut target).cast());
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn panics_do_not_cross_ffi_callback_boundaries() {
        let mut target = UpdateTarget {
            wake: Box::new(|| panic!("redraw panic fixture")),
        };
        let loader_callback =
            |_name: &std::ffi::CStr| -> *const std::ffi::c_void { panic!("loader panic fixture") };
        let mut loader = ProcAddressLoader {
            get_proc_address: &loader_callback,
        };

        // SAFETY: Both stack values remain alive and immovable for these
        // synchronous calls; the symbol name is a static C string.
        unsafe {
            request_redraw(std::ptr::from_mut(&mut target).cast());
            assert!(
                load_opengl_proc(
                    std::ptr::from_mut(&mut loader).cast(),
                    c"glGetIntegerv".as_ptr(),
                )
                .is_null()
            );
        }
    }
}
