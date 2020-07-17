#![cfg(windows)]
#![forbid(rust_2018_idioms)]
#![deny(missing_docs)]
//! This crate offers a DirectX 9 renderer for the [imgui-rs](https://docs.rs/imgui/*/imgui/) rust bindings.

pub use winapi::shared::d3d9::{IDirect3DDevice9, LPDIRECT3DBASETEXTURE9};

use imgui::{
    internal::RawWrapper, BackendFlags, Context, DrawCmd, DrawCmdParams, DrawData, DrawIdx,
    ImString, TextureId, Textures,
};

use winapi::shared::d3d9::{
    IDirect3DIndexBuffer9, IDirect3DVertexBuffer9, LPDIRECT3DDEVICE9, LPDIRECT3DSTATEBLOCK9,
    LPDIRECT3DTEXTURE9,
};
use winapi::shared::{d3d9types::*, minwindef, windef::RECT};

use core::fmt;
use core::mem;
use core::ptr::{self, NonNull};
use core::slice;

const FONT_TEX_ID: usize = !0;

const D3D_OK: i32 = 0;
const D3DPOLL_DEFAULT: u32 = 0;
const D3DFVF_CUSTOMVERTEX: u32 = D3DFVF_XYZ | D3DFVF_DIFFUSE | D3DFVF_TEX1;

const FALSE: u32 = minwindef::FALSE as u32;
const TRUE: u32 = minwindef::TRUE as u32;

const VERTEX_BUF_ADD_CAPACITY: usize = 5000;
const INDEX_BUF_ADD_CAPACITY: usize = 10000;

static MAT_IDENTITY: D3DMATRIX = D3DMATRIX {
    m: [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ],
};

#[repr(C)]
struct CustomVertex {
    pos: [f32; 3],
    col: [u8; 4],
    uv: [f32; 2],
}

type Result<T> = core::result::Result<T, RendererError>;

/// The error type returned by the renderer.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum RendererError {
    /// The directx device ran out of memory
    OutOfMemory,
    /// The renderer received an invalid texture id
    InvalidTexture(TextureId),
}

impl fmt::Display for RendererError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            RendererError::OutOfMemory => write!(f, "device ran out of memory"),
            RendererError::InvalidTexture(id) => {
                write!(f, "failed to find texture with id {:?}", id)
            },
        }
    }
}

impl std::error::Error for RendererError {}

/// A DirectX 9 renderer for (Imgui-rs)[https://docs.rs/imgui/*/imgui/].
pub struct Renderer {
    device: NonNull<IDirect3DDevice9>,
    font_tex: FontTexture,
    vertex_buffer: VertexBuffer,
    index_buffer: IndexBuffer,
    textures: Textures<LPDIRECT3DBASETEXTURE9>,
}

impl Renderer {
    /// Creates a new renderer for the given [`IDirect3DDevice9`].
    ///
    /// Internally the renderer will then add a reference through the
    /// COM api via [`IUnknown::AddRef`] and release it via
    /// [`IUnknown::Release`] once dropped.
    ///
    /// [`IDirect3DDevice9`]: https://docs.rs/winapi/0.3/x86_64-pc-windows-msvc/winapi/shared/d3d9/struct.IDirect3DDevice9.html
    /// [`IUnknown::AddRef`]: https://docs.rs/winapi/0.3/x86_64-pc-windows-msvc/winapi/um/unknwnbase/struct.IUnknown.html#method.AddRef
    /// [`IUnknown::Release`]: https://docs.rs/winapi/0.3/x86_64-pc-windows-msvc/winapi/um/unknwnbase/struct.IUnknown.html#method.Release
    pub fn new(ctx: &mut Context, mut device: NonNull<IDirect3DDevice9>) -> Result<Self> {
        unsafe {
            let font_tex = Self::create_font_texture(ctx.fonts(), device)?;
            (device.as_mut()).AddRef();
            ctx.io_mut().backend_flags |= BackendFlags::RENDERER_HAS_VTX_OFFSET;
            ctx.set_renderer_name(ImString::new(concat!(
                "imgui_dx9_renderer@",
                env!("CARGO_PKG_VERSION")
            )));
            Ok(Renderer {
                device,
                font_tex,
                vertex_buffer: Self::create_vertex_buffer(device.as_mut(), 0)?,
                index_buffer: Self::create_index_buffer(device.as_mut(), 0)?,
                textures: Textures::new(),
            })
        }
    }

    /// The textures registry of this renderer.
    ///
    /// The texture slot at !0 is reserved for the font texture, therefore the
    /// renderer will ignore any texture inserted into said slot.
    ///
    /// # Safety
    ///
    /// Mutable access is unsafe since the renderer assumes that the texture
    /// handles inside of it are valid until they are removed manually.
    /// Failure to keep this invariant in check will cause UB.
    #[inline]
    pub unsafe fn textures_mut(&mut self) -> &mut Textures<LPDIRECT3DBASETEXTURE9> {
        &mut self.textures
    }

    /// The textures registry of this renderer.
    #[inline]
    pub fn textures(&self) -> &Textures<LPDIRECT3DBASETEXTURE9> {
        &self.textures
    }

    /// Renders the given [`Ui`] with this renderer.
    ///
    /// [`Ui`]: https://docs.rs/imgui/*/imgui/struct.Ui.html
    pub fn render(&mut self, draw_data: &DrawData) -> Result<()> {
        if draw_data.display_size[0] < 0.0 || draw_data.display_size[1] < 0.0 {
            return Ok(());
        }
        unsafe {
            if self.vertex_buffer.len() < draw_data.total_vtx_count as usize {
                self.vertex_buffer = Self::create_vertex_buffer(
                    self.device.as_mut(),
                    draw_data.total_vtx_count as usize,
                )?;
            }
            if self.index_buffer.len() < draw_data.total_idx_count as usize {
                self.index_buffer = Self::create_index_buffer(
                    self.device.as_mut(),
                    draw_data.total_idx_count as usize,
                )?;
            }

            let _state_backup = StateBackup::backup(self.device.as_ptr())?;

            self.set_render_state(draw_data);
            self.write_buffers(draw_data)?;
            self.render_impl(draw_data)
        }
    }

    unsafe fn render_impl(&mut self, draw_data: &DrawData) -> Result<()> {
        let clip_off = draw_data.display_pos;
        let clip_scale = draw_data.framebuffer_scale;
        let mut vertex_offset = 0;
        let mut index_offset = 0;
        let mut last_tex = TextureId::from(FONT_TEX_ID);
        (self.device.as_mut()).SetTexture(0, self.font_tex.0 as LPDIRECT3DBASETEXTURE9);
        for draw_list in draw_data.draw_lists() {
            for cmd in draw_list.commands() {
                match cmd {
                    DrawCmd::Elements {
                        count,
                        cmd_params:
                            DrawCmdParams {
                                clip_rect,
                                texture_id,
                                ..
                            },
                    } => {
                        if texture_id != last_tex {
                            let texture = if texture_id.id() == FONT_TEX_ID {
                                self.font_tex.0 as LPDIRECT3DBASETEXTURE9
                            } else {
                                *self
                                    .textures
                                    .get(texture_id)
                                    .ok_or(RendererError::InvalidTexture(texture_id))?
                            };
                            (self.device.as_mut()).SetTexture(0, texture);
                            last_tex = texture_id;
                        }

                        let r: RECT = RECT {
                            left: ((clip_rect[0] - clip_off[0]) * clip_scale[0]) as i32,
                            top: ((clip_rect[1] - clip_off[1]) * clip_scale[1]) as i32,
                            right: ((clip_rect[2] - clip_off[0]) * clip_scale[0]) as i32,
                            bottom: ((clip_rect[3] - clip_off[1]) * clip_scale[1]) as i32,
                        };
                        (self.device.as_mut()).SetScissorRect(&r);
                        (self.device.as_mut()).DrawIndexedPrimitive(
                            D3DPT_TRIANGLELIST,
                            vertex_offset as i32,
                            0,
                            draw_list.vtx_buffer().len() as u32,
                            index_offset as u32,
                            count as u32 / 3,
                        );
                        index_offset += count;
                    },
                    DrawCmd::ResetRenderState => self.set_render_state(draw_data),
                    DrawCmd::RawCallback { callback, raw_cmd } => {
                        callback(draw_list.raw(), raw_cmd)
                    },
                }
            }
            vertex_offset += draw_list.vtx_buffer().len();
        }
        Ok(())
    }

    unsafe fn set_render_state(&mut self, draw_data: &DrawData) {
        let fb_width = draw_data.display_size[0] * draw_data.framebuffer_scale[0];
        let fb_height = draw_data.display_size[1] * draw_data.framebuffer_scale[1];

        let device = self.device.as_mut();
        let vp = D3DVIEWPORT9 {
            X: 0,
            Y: 0,
            Width: fb_width as _,
            Height: fb_height as _,
            MinZ: 0.0,
            MaxZ: 1.0,
        };

        device.SetViewport(&vp);
        device.SetPixelShader(ptr::null_mut());
        device.SetVertexShader(ptr::null_mut());
        device.SetRenderState(D3DRS_CULLMODE, D3DCULL_NONE);
        device.SetRenderState(D3DRS_LIGHTING, FALSE);
        device.SetRenderState(D3DRS_ZENABLE, FALSE);
        device.SetRenderState(D3DRS_ALPHABLENDENABLE, TRUE);
        device.SetRenderState(D3DRS_ALPHATESTENABLE, FALSE);
        device.SetRenderState(D3DRS_BLENDOP, D3DBLENDOP_ADD);
        device.SetRenderState(D3DRS_SRCBLEND, D3DBLEND_SRCALPHA);
        device.SetRenderState(D3DRS_DESTBLEND, D3DBLEND_INVSRCALPHA);
        device.SetRenderState(D3DRS_SCISSORTESTENABLE, TRUE);
        device.SetRenderState(D3DRS_SHADEMODE, D3DSHADE_GOURAUD);
        device.SetRenderState(D3DRS_FOGENABLE, FALSE);
        device.SetTextureStageState(FALSE, D3DTSS_COLOROP, D3DTOP_MODULATE);
        device.SetTextureStageState(FALSE, D3DTSS_COLORARG1, D3DTA_TEXTURE);
        device.SetTextureStageState(FALSE, D3DTSS_COLORARG2, D3DTA_DIFFUSE);
        device.SetTextureStageState(FALSE, D3DTSS_ALPHAOP, D3DTOP_MODULATE);
        device.SetTextureStageState(FALSE, D3DTSS_ALPHAARG1, D3DTA_TEXTURE);
        device.SetTextureStageState(FALSE, D3DTSS_ALPHAARG2, D3DTA_DIFFUSE);
        device.SetSamplerState(FALSE, D3DSAMP_MINFILTER, D3DTEXF_LINEAR);
        device.SetSamplerState(FALSE, D3DSAMP_MAGFILTER, D3DTEXF_LINEAR);

        let l = draw_data.display_pos[0] + 0.5;
        let r = draw_data.display_pos[0] + draw_data.display_size[0] + 0.5;
        let t = draw_data.display_pos[1] + 0.5;
        let b = draw_data.display_pos[1] + draw_data.display_size[1] + 0.5;
        let mat_projection = D3DMATRIX {
            m: [
                [2.0 / (r - l), 0.0, 0.0, 0.0],
                [0.0, 2.0 / (t - b), 0.0, 0.0],
                [0.0, 0.0, 0.5, 0.0],
                [(l + r) / (l - r), (t + b) / (b - t), 0.5, 1.0],
            ],
        };
        device.SetTransform(D3DTS_WORLD, &MAT_IDENTITY);
        device.SetTransform(D3DTS_VIEW, &MAT_IDENTITY);
        device.SetTransform(D3DTS_PROJECTION, &mat_projection);
    }

    unsafe fn lock_buffers<'v, 'i>(
        vb: &'v mut IDirect3DVertexBuffer9,
        ib: &'i mut IDirect3DIndexBuffer9,
        vtx_count: usize,
        idx_count: usize,
    ) -> Result<(&'v mut [CustomVertex], &'i mut [DrawIdx])> {
        let mut vtx_dst: *mut CustomVertex = ptr::null_mut();
        let mut idx_dst: *mut DrawIdx = ptr::null_mut();
        let vb_locked = vb.Lock(
            0,
            (vtx_count * mem::size_of::<CustomVertex>()) as u32,
            &mut vtx_dst as *mut _ as _,
            D3DLOCK_DISCARD,
        ) == D3D_OK;
        let ib_locked = ib.Lock(
            0,
            (idx_count * mem::size_of::<DrawIdx>()) as u32,
            &mut idx_dst as *mut _ as _,
            D3DLOCK_DISCARD,
        ) == D3D_OK;
        if vb_locked && ib_locked {
            Ok((
                slice::from_raw_parts_mut(vtx_dst, vtx_count),
                slice::from_raw_parts_mut(idx_dst, idx_count),
            ))
        } else {
            if vb_locked {
                vb.Unlock();
            } else if ib_locked {
                ib.Unlock();
            }
            Err(RendererError::OutOfMemory)
        }
    }

    unsafe fn write_buffers(&mut self, draw_data: &DrawData) -> Result<()> {
        let (vb, ib) = (
            &mut *self.vertex_buffer.as_ptr(),
            &mut *self.index_buffer.as_ptr(),
        );

        let (mut vtx_dst, mut idx_dst) = Self::lock_buffers(
            vb,
            ib,
            draw_data.total_vtx_count as usize,
            draw_data.total_idx_count as usize,
        )?;

        for draw_list in draw_data.draw_lists() {
            for (vertex, vtx_dst) in draw_list.vtx_buffer().iter().zip(vtx_dst.iter_mut()) {
                *vtx_dst = CustomVertex {
                    pos: [vertex.pos[0], vertex.pos[1], 0.0],
                    col: [vertex.col[2], vertex.col[1], vertex.col[0], vertex.col[3]],
                    uv: [vertex.uv[0], vertex.uv[1]],
                };
            }
            idx_dst[..draw_list.idx_buffer().len()].copy_from_slice(draw_list.idx_buffer());
            vtx_dst = &mut vtx_dst[draw_list.vtx_buffer().len()..];
            idx_dst = &mut idx_dst[draw_list.idx_buffer().len()..];
        }
        vb.Unlock();
        ib.Unlock();
        self.device
            .as_mut()
            .SetStreamSource(0, vb, 0, mem::size_of::<CustomVertex>() as u32);
        self.device.as_mut().SetIndices(ib);
        self.device.as_mut().SetFVF(D3DFVF_CUSTOMVERTEX);
        Ok(())
    }

    unsafe fn create_vertex_buffer(
        device: &mut IDirect3DDevice9,
        vtx_count: usize,
    ) -> Result<VertexBuffer> {
        let mut vertex_buffer = ptr::null_mut();
        let len = vtx_count + VERTEX_BUF_ADD_CAPACITY;
        if device.CreateVertexBuffer(
            (len * mem::size_of::<CustomVertex>()) as u32,
            D3DUSAGE_DYNAMIC | D3DUSAGE_WRITEONLY,
            D3DFVF_CUSTOMVERTEX,
            D3DPOOL_DEFAULT,
            &mut vertex_buffer,
            ptr::null_mut(),
        ) < 0
        {
            Err(RendererError::OutOfMemory)
        } else {
            NonNull::new(slice::from_raw_parts_mut(vertex_buffer, len))
                .map(VertexBuffer)
                .ok_or(RendererError::OutOfMemory)
        }
    }

    unsafe fn create_index_buffer(
        device: &mut IDirect3DDevice9,
        idx_count: usize,
    ) -> Result<IndexBuffer> {
        let mut index_buffer = ptr::null_mut();
        let len = idx_count + INDEX_BUF_ADD_CAPACITY;
        if device.CreateIndexBuffer(
            (len * mem::size_of::<DrawIdx>()) as u32,
            D3DUSAGE_DYNAMIC | D3DUSAGE_WRITEONLY,
            if mem::size_of::<DrawIdx>() == 2 {
                D3DFMT_INDEX16
            } else {
                D3DFMT_INDEX32
            },
            D3DPOOL_DEFAULT,
            &mut index_buffer,
            ptr::null_mut(),
        ) < 0
        {
            Err(RendererError::OutOfMemory)
        } else {
            NonNull::new(slice::from_raw_parts_mut(index_buffer, len))
                .map(IndexBuffer)
                .ok_or(RendererError::OutOfMemory)
        }
    }

    // FIXME, imgui hands us an rgba texture while we make dx9 think it receives an
    // argb texture
    unsafe fn create_font_texture(
        mut fonts: imgui::FontAtlasRefMut<'_>,
        mut device: NonNull<IDirect3DDevice9>,
    ) -> Result<FontTexture> {
        let texture = fonts.build_rgba32_texture();
        let mut texture_handle: LPDIRECT3DTEXTURE9 = ptr::null_mut();
        let mut tex_locked_rect: D3DLOCKED_RECT = D3DLOCKED_RECT {
            Pitch: 0,
            pBits: ptr::null_mut(),
        };

        let tex_created = (device.as_mut()).CreateTexture(
            texture.width,
            texture.height,
            1,
            D3DUSAGE_DYNAMIC,
            D3DFMT_A8R8G8B8,
            D3DPOLL_DEFAULT,
            &mut texture_handle,
            ptr::null_mut(),
        ) == D3D_OK;
        if tex_created
            && (*texture_handle).LockRect(0, &mut tex_locked_rect, ptr::null_mut(), 0) == D3D_OK
        {
            let bits = tex_locked_rect.pBits as *mut u8;
            let pitch = tex_locked_rect.Pitch as usize;
            let height = texture.height as usize;
            let width = texture.width as usize;

            for y in 0..height {
                let d3d9_memory = bits.add(pitch * y);
                let pixels = texture.data.as_ptr();
                let pixels = pixels.add((width * 4) * y);
                std::ptr::copy(pixels, d3d9_memory, width * 4);
            }

            (*texture_handle).UnlockRect(0);
            fonts.tex_id = TextureId::from(FONT_TEX_ID);
            Ok(FontTexture(texture_handle))
        } else {
            Err(RendererError::OutOfMemory)
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe { self.device.as_mut().Release() };
    }
}

struct FontTexture(LPDIRECT3DTEXTURE9);

impl Drop for FontTexture {
    #[inline]
    fn drop(&mut self) {
        unsafe { (*self.0).Release() };
    }
}

struct VertexBuffer(NonNull<[IDirect3DVertexBuffer9]>);

impl VertexBuffer {
    #[inline]
    fn len(&self) -> usize {
        unsafe { self.0.as_ref().len() }
    }

    #[inline]
    fn as_ptr(&mut self) -> *mut IDirect3DVertexBuffer9 {
        unsafe { (*self.0.as_ptr()).as_mut_ptr() }
    }
}

impl Drop for VertexBuffer {
    #[inline]
    fn drop(&mut self) {
        unsafe { (*self.as_ptr()).Release() };
    }
}

struct IndexBuffer(NonNull<[IDirect3DIndexBuffer9]>);

impl IndexBuffer {
    #[inline]
    fn len(&self) -> usize {
        unsafe { self.0.as_ref().len() }
    }

    #[inline]
    fn as_ptr(&mut self) -> *mut IDirect3DIndexBuffer9 {
        unsafe { (*self.0.as_ptr()).as_mut_ptr() }
    }
}

impl Drop for IndexBuffer {
    #[inline]
    fn drop(&mut self) {
        unsafe { (*self.as_ptr()).Release() };
    }
}

struct StateBackup {
    state_block: LPDIRECT3DSTATEBLOCK9,
}

impl StateBackup {
    unsafe fn backup(device: LPDIRECT3DDEVICE9) -> Result<Self> {
        let mut state_block = ptr::null_mut();
        if (*device).CreateStateBlock(D3DSBT_ALL, &mut state_block) < 0 {
            Err(RendererError::OutOfMemory)
        } else {
            Ok(StateBackup { state_block })
        }
    }
}

impl Drop for StateBackup {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            (*self.state_block).Apply();
            (*self.state_block).Release();
        }
    }
}
