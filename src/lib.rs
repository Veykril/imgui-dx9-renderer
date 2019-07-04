#![cfg(windows)]
#![forbid(rust_2018_idioms)]
#![deny(missing_docs)]
//! This crate offers a DirectX 9 renderer for the imgui-rs rust bindings.

pub use winapi::shared::d3d9::IDirect3DDevice9;

use imgui::{
    internal::RawWrapper, BackendFlags, Context, DrawCmd, DrawCmdParams, DrawData, DrawIdx,
    ImString, TextureId,
};

use winapi::shared::{
    d3d9::{
        IDirect3DIndexBuffer9, IDirect3DVertexBuffer9, LPDIRECT3DDEVICE9, LPDIRECT3DSTATEBLOCK9,
        LPDIRECT3DTEXTURE9,
    },
    d3d9types::*,
    minwindef,
    windef::RECT,
};

use core::{
    fmt,
    mem::{self, ManuallyDrop},
    ptr::{self, NonNull},
    slice,
};

const FONT_TEX_ID: usize = !0;

const D3D_OK: i32 = 0;
const D3DPOLL_DEFAULT: u32 = 0;
const D3DFVF_CUSTOMVERTEX: u32 = D3DFVF_XYZ | D3DFVF_DIFFUSE | D3DFVF_TEX1;

const FALSE: u32 = minwindef::FALSE as u32;
const TRUE: u32 = minwindef::TRUE as u32;

#[repr(C)]
struct CustomVertex {
    pos: [f32; 3],
    col: [u8; 4],
    uv: [f32; 2],
}

type Result<T> = core::result::Result<T, RendererError>;

/// The error type returned by the renderer.
#[derive(Clone, Debug, PartialEq)]
pub enum RendererError {
    /// The renderer failed to create the font texture
    TextureCreation,
    /// The renderer failed to create the index buffer
    IndexCreation,
    /// The renderer failed to create the vertex buffer
    VertexCreation,
    /// The renderer received an invalid texture id
    InvalidTexture(usize),
    /// The renderer failed to backup the dx9 state
    StateBackup,
    /// The renderer failed to write to the buffers
    WriteBuffer,
}

impl fmt::Display for RendererError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            RendererError::VertexCreation => write!(f, "failed to create vertex buffer"),
            RendererError::IndexCreation => write!(f, "failed to create index buffer"),
            RendererError::TextureCreation => write!(f, "failed to create font texture"),
            RendererError::InvalidTexture(id) => write!(f, "failed to find texture with id {}", id),
            RendererError::StateBackup => write!(f, "failed to backup dx9 state"),
            RendererError::WriteBuffer => write!(f, "failed to write to buffer"),
        }
    }
}

impl std::error::Error for RendererError {}

/// A DirectX 9 renderer for Imgui-rs.
pub struct Renderer {
    device: NonNull<IDirect3DDevice9>,
    font_tex: Texture,
    vertex_buffer: Option<VertexBuffer>,
    index_buffer: Option<IndexBuffer>,
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
                vertex_buffer: None,
                index_buffer: None,
            })
        }
    }

    /// Renders the given [`Ui`] with this renderer.
    ///
    /// [`Ui`]: https://docs.rs/imgui/*/imgui/struct.Ui.html
    pub fn render(&mut self, draw_data: &DrawData) -> Result<()> {
        if draw_data.display_size[0] < 0.0 || draw_data.display_size[1] < 0.0 {
            Ok(())
        } else {
            unsafe {
                match self.vertex_buffer {
                    Some(ref vb) if vb.len() > draw_data.total_vtx_count as usize => (),
                    _ => self.recreate_vertex_buffer(draw_data)?,
                }
                match self.index_buffer {
                    Some(ref ib) if ib.len() > draw_data.total_idx_count as usize => (),
                    _ => self.recreate_index_buffer(draw_data)?,
                }

                let _state_backup = StateBackup::backup(self.device.as_ptr())?;

                self.set_render_state(draw_data);
                self.write_buffers(draw_data)?;
                self.render_impl(draw_data)
            }
        }
    }

    unsafe fn render_impl(&mut self, draw_data: &DrawData) -> Result<()> {
        let clip_off = draw_data.display_pos;
        let clip_scale = draw_data.framebuffer_scale;
        let mut vertex_offset = 0;
        let mut index_offset = 0;
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
                        let r: RECT = RECT {
                            left: ((clip_rect[0] - clip_off[0]) * clip_scale[0]) as _,
                            top: ((clip_rect[1] - clip_off[1]) * clip_scale[1]) as _,
                            right: ((clip_rect[2] - clip_off[0]) * clip_scale[0]) as _,
                            bottom: ((clip_rect[3] - clip_off[1]) * clip_scale[1]) as _,
                        };
                        let texture = if texture_id.id() == FONT_TEX_ID {
                            self.font_tex.0
                        } else {
                            return Err(RendererError::InvalidTexture(texture_id.id()));
                        };
                        (self.device.as_mut()).SetTexture(0, texture as _);
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
        let mat_identity = D3DMATRIX {
            m: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        };
        let mat_projection = D3DMATRIX {
            m: [
                [2.0 / (r - l), 0.0, 0.0, 0.0],
                [0.0, 2.0 / (t - b), 0.0, 0.0],
                [0.0, 0.0, 0.5, 0.0],
                [(l + r) / (l - r), (t + b) / (b - t), 0.5, 1.0],
            ],
        };
        device.SetTransform(D3DTS_WORLD, &mat_identity);
        device.SetTransform(D3DTS_VIEW, &mat_identity);
        device.SetTransform(D3DTS_PROJECTION, &mat_projection);
    }

    unsafe fn write_buffers(&mut self, draw_data: &DrawData) -> Result<()> {
        let (vb, ib) = (
            self.vertex_buffer
                .as_mut()
                .expect("vertexbuffer should've been initialized"),
            self.index_buffer
                .as_mut()
                .expect("indexbuffer should've been initialized"),
        );
        let mut vtx_dst: *mut CustomVertex = ptr::null_mut();
        let mut idx_dst: *mut DrawIdx = ptr::null_mut();

        if (*vb.as_ptr()).Lock(
            0,
            (draw_data.total_vtx_count as usize * mem::size_of::<CustomVertex>()) as u32,
            &mut vtx_dst as *mut _ as _,
            D3DLOCK_DISCARD,
        ) != D3D_OK
            || (*ib.as_ptr()).Lock(
                0,
                (draw_data.total_idx_count as usize * mem::size_of::<DrawIdx>()) as u32,
                &mut idx_dst as *mut _ as _,
                D3DLOCK_DISCARD,
            ) != D3D_OK
        {
            Err(RendererError::WriteBuffer)
        } else {
            for draw_list in draw_data.draw_lists() {
                for vertex in draw_list.vtx_buffer() {
                    *vtx_dst = CustomVertex {
                        pos: [vertex.pos[0], vertex.pos[1], 0.0],
                        col: [vertex.col[2], vertex.col[1], vertex.col[0], vertex.col[3]],
                        uv: [vertex.uv[0], vertex.uv[1]],
                    };
                    vtx_dst = vtx_dst.add(1);
                }
                ptr::copy(
                    draw_list.idx_buffer().as_ptr(),
                    idx_dst as _,
                    draw_list.idx_buffer().len(),
                );
                idx_dst = idx_dst.add(draw_list.idx_buffer().len());
            }
            (*vb.as_ptr()).Unlock();
            (*ib.as_ptr()).Unlock();
            self.device.as_mut().SetStreamSource(
                0,
                vb.as_ptr(),
                0,
                mem::size_of::<CustomVertex>() as u32,
            );
            self.device.as_mut().SetIndices(ib.as_ptr());
            self.device.as_mut().SetFVF(D3DFVF_CUSTOMVERTEX);
            Ok(())
        }
    }

    unsafe fn recreate_vertex_buffer(&mut self, draw_data: &DrawData) -> Result<()> {
        let mut vertex_buffer = ptr::null_mut();
        let len = draw_data.total_vtx_count as usize + 5000;
        if self.device.as_mut().CreateVertexBuffer(
            (len * mem::size_of::<CustomVertex>()) as u32,
            D3DUSAGE_DYNAMIC | D3DUSAGE_WRITEONLY,
            D3DFVF_CUSTOMVERTEX,
            D3DPOOL_DEFAULT,
            &mut vertex_buffer,
            ptr::null_mut(),
        ) < 0
        {
            Err(RendererError::VertexCreation)
        } else {
            self.vertex_buffer =
                NonNull::new(slice::from_raw_parts_mut(vertex_buffer, len)).map(VertexBuffer);
            Ok(())
        }
    }

    unsafe fn recreate_index_buffer(&mut self, draw_data: &DrawData) -> Result<()> {
        let mut index_buffer = ptr::null_mut();
        let len = draw_data.total_idx_count as usize + 10000;
        if self.device.as_mut().CreateIndexBuffer(
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
            Err(RendererError::IndexCreation)
        } else {
            self.index_buffer =
                NonNull::new(slice::from_raw_parts_mut(index_buffer, len)).map(IndexBuffer);
            Ok(())
        }
    }

    // FIXME, imgui hands us an rgba texture while we make dx9 think it receives an
    // argb texture
    unsafe fn create_font_texture(
        mut fonts: imgui::FontAtlasRefMut<'_>,
        mut device: NonNull<IDirect3DDevice9>,
    ) -> Result<Texture> {
        let texture = fonts.build_rgba32_texture();
        let mut texture_handle: LPDIRECT3DTEXTURE9 = ptr::null_mut();
        let mut tex_locked_rect: D3DLOCKED_RECT = D3DLOCKED_RECT {
            Pitch: 0,
            pBits: ptr::null_mut(),
        };

        if (device.as_mut()).CreateTexture(
            texture.width,
            texture.height,
            1,
            D3DUSAGE_DYNAMIC,
            D3DFMT_A8R8G8B8,
            D3DPOLL_DEFAULT,
            &mut texture_handle,
            ptr::null_mut(),
        ) != D3D_OK
            || (*texture_handle).LockRect(0, &mut tex_locked_rect, ptr::null_mut(), 0) != D3D_OK
        {
            Err(RendererError::TextureCreation)
        } else {
            slice::from_raw_parts_mut(tex_locked_rect.pBits as *mut u8, texture.data.len())
                .copy_from_slice(texture.data);

            (*texture_handle).UnlockRect(0);
            fonts.tex_id = TextureId::from(FONT_TEX_ID);
            Ok(Texture(texture_handle))
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe { self.device.as_mut().Release() };
    }
}

struct Texture(LPDIRECT3DTEXTURE9);

impl Drop for Texture {
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
    fn drop(&mut self) {
        unsafe { (*self.as_ptr()).Release() };
    }
}

struct StateBackup {
    device: LPDIRECT3DDEVICE9,
    state_block: LPDIRECT3DSTATEBLOCK9,
    last_world: D3DMATRIX,
    last_view: D3DMATRIX,
    last_projection: D3DMATRIX,
}

impl StateBackup {
    unsafe fn backup(device: LPDIRECT3DDEVICE9) -> Result<Self> {
        // FIXME: Use MaybeUninit once its stable
        let mut this = ManuallyDrop::<Self>::new(mem::zeroed());
        this.device = device;
        if (*device).CreateStateBlock(D3DSBT_ALL, &mut this.state_block) < 0 {
            Err(RendererError::StateBackup)
        } else {
            (*device).GetTransform(D3DTS_WORLD, &mut this.last_world);
            (*device).GetTransform(D3DTS_VIEW, &mut this.last_view);
            (*device).GetTransform(D3DTS_PROJECTION, &mut this.last_projection);
            Ok(ManuallyDrop::into_inner(this))
        }
    }
}

impl Drop for StateBackup {
    fn drop(&mut self) {
        unsafe {
            (*self.device).SetTransform(D3DTS_WORLD, &self.last_world);
            (*self.device).SetTransform(D3DTS_VIEW, &self.last_view);
            (*self.device).SetTransform(D3DTS_PROJECTION, &self.last_projection);
            (*self.state_block).Apply();
            (*self.state_block).Release();
        }
    }
}
