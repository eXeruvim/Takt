use std::{
    ffi::{CStr, c_void},
    fmt,
    os::raw::{c_char, c_int},
    ptr::NonNull,
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

    let loader = unsafe { &*(context.cast::<ProcAddressLoader<'_>>()) };
    let name = unsafe { CStr::from_ptr(name) };
    (loader.get_proc_address)(name).cast_mut()
}

struct UpdateTarget {
    wake: Box<dyn Fn() + Send + Sync>,
}

type GlGetIntegerv = unsafe extern "system" fn(u32, *mut i32);
type GlBindFramebuffer = unsafe extern "system" fn(u32, u32);
type GlViewport = unsafe extern "system" fn(i32, i32, i32, i32);
type GlScissor = unsafe extern "system" fn(i32, i32, i32, i32);
type GlIsEnabled = unsafe extern "system" fn(u32) -> u8;
type GlSetCapability = unsafe extern "system" fn(u32);

const GL_SCISSOR_TEST: u32 = 0x0C11;
const GL_SCISSOR_BOX: u32 = 0x0C10;
const GL_VIEWPORT: u32 = 0x0BA2;
const GL_DRAW_FRAMEBUFFER: u32 = 0x8CA9;
const GL_READ_FRAMEBUFFER: u32 = 0x8CA8;
const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
const GL_READ_FRAMEBUFFER_BINDING: u32 = 0x8CAA;

struct GlStateApi {
    get_integer: GlGetIntegerv,
    bind_framebuffer: GlBindFramebuffer,
    viewport: GlViewport,
    scissor: GlScissor,
    is_enabled: GlIsEnabled,
    enable: GlSetCapability,
    disable: GlSetCapability,
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
            get_integer: unsafe {
                std::mem::transmute::<*const c_void, GlGetIntegerv>(load(c"glGetIntegerv")?)
            },
            bind_framebuffer: unsafe {
                std::mem::transmute::<*const c_void, GlBindFramebuffer>(load(c"glBindFramebuffer")?)
            },
            viewport: unsafe {
                std::mem::transmute::<*const c_void, GlViewport>(load(c"glViewport")?)
            },
            scissor: unsafe {
                std::mem::transmute::<*const c_void, GlScissor>(load(c"glScissor")?)
            },
            is_enabled: unsafe {
                std::mem::transmute::<*const c_void, GlIsEnabled>(load(c"glIsEnabled")?)
            },
            enable: unsafe {
                std::mem::transmute::<*const c_void, GlSetCapability>(load(c"glEnable")?)
            },
            disable: unsafe {
                std::mem::transmute::<*const c_void, GlSetCapability>(load(c"glDisable")?)
            },
        })
    }

    fn capture(&self) -> GlStateSnapshot<'_> {
        let mut draw_framebuffer = 0;
        let mut read_framebuffer = 0;
        let mut viewport = [0; 4];
        let mut scissor = [0; 4];
        unsafe {
            (self.get_integer)(GL_DRAW_FRAMEBUFFER_BINDING, &mut draw_framebuffer);
            (self.get_integer)(GL_READ_FRAMEBUFFER_BINDING, &mut read_framebuffer);
            (self.get_integer)(GL_VIEWPORT, viewport.as_mut_ptr());
            (self.get_integer)(GL_SCISSOR_BOX, scissor.as_mut_ptr());
        }
        let scissor_enabled = unsafe { (self.is_enabled)(GL_SCISSOR_TEST) != 0 };

        GlStateSnapshot {
            api: self,
            draw_framebuffer,
            read_framebuffer,
            viewport,
            scissor,
            scissor_enabled,
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
}

impl Drop for GlStateSnapshot<'_> {
    fn drop(&mut self) {
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
            if self.scissor_enabled {
                (self.api.enable)(GL_SCISSOR_TEST);
            } else {
                (self.api.disable)(GL_SCISSOR_TEST);
            }
        }
    }
}

unsafe extern "C" fn request_redraw(context: *mut c_void) {
    if context.is_null() {
        return;
    }

    let target = unsafe { &*(context.cast::<UpdateTarget>()) };
    (target.wake)();
}

pub struct MpvOpenGlRenderer {
    context: NonNull<mpv_render_context>,
    update_target: NonNull<UpdateTarget>,
    gl_state: GlStateApi,
    _source: MpvRenderSource,
}

impl MpvOpenGlRenderer {
    pub fn create(
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

        let result = unsafe {
            mpv_render_context_create(&mut context, source.handle(), parameters.as_mut_ptr())
        };
        if result < 0 {
            return Err(anyhow!(
                "mpv render context initialization failed ({result})"
            ));
        }
        let context =
            NonNull::new(context).ok_or_else(|| anyhow!("mpv returned an empty render context"))?;

        let update_target = NonNull::from(Box::leak(Box::new(UpdateTarget {
            wake: Box::new(wake),
        })));
        unsafe {
            mpv_render_context_set_update_callback(
                context.as_ptr(),
                Some(request_redraw),
                update_target.as_ptr().cast(),
            );
        }

        Ok(Self {
            context,
            update_target,
            gl_state,
            _source: source,
        })
    }

    pub fn render(&mut self, size: (u32, u32)) -> Result<()> {
        let _state = self.gl_state.capture();
        let width = c_int::try_from(size.0).unwrap_or(c_int::MAX).max(1);
        let height = c_int::try_from(size.1).unwrap_or(c_int::MAX).max(1);
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

        unsafe {
            mpv_render_context_update(self.context.as_ptr());
        }
        let result =
            unsafe { mpv_render_context_render(self.context.as_ptr(), parameters.as_mut_ptr()) };
        if result < 0 {
            return Err(anyhow!("mpv OpenGL render failed ({result})"));
        }
        Ok(())
    }

    pub fn report_swap(&self) {
        unsafe {
            mpv_render_context_report_swap(self.context.as_ptr());
        }
    }
}

impl Drop for MpvOpenGlRenderer {
    fn drop(&mut self) {
        unsafe {
            mpv_render_context_set_update_callback(
                self.context.as_ptr(),
                None,
                std::ptr::null_mut(),
            );
            mpv_render_context_free(self.context.as_ptr());
            drop(Box::from_raw(self.update_target.as_ptr()));
        }
    }
}
