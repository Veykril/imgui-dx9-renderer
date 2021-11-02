#![cfg(windows)]
#![deny(missing_docs)]
//! This crate offers a DirectX 9 renderer for the [imgui-rs](https://docs.rs/imgui/*/imgui/) rust bindings.

use std::mem;
use std::ptr;
use std::slice;

use imgui::{
    internal::RawWrapper, BackendFlags, Context, DrawCmd, DrawCmdParams, DrawData, DrawIdx,
    TextureId, Textures,
};
use winapi::shared::d3d9::{
    IDirect3DBaseTexture9, IDirect3DDevice9, IDirect3DIndexBuffer9, IDirect3DStateBlock9,
    IDirect3DTexture9, IDirect3DVertexBuffer9,
};
use winapi::shared::d3d9types::*;
use winapi::shared::winerror::{DXGI_ERROR_INVALID_CALL, HRESULT, S_OK};
use winapi::shared::{minwindef, windef::RECT};
use winapi::Interface;
use wio::com::ComPtr;

const FONT_TEX_ID: usize = !0;

const D3DPOLL_DEFAULT: u32 = 0;
const D3DFVF_CUSTOMVERTEX: u32 = D3DFVF_XYZ | D3DFVF_DIFFUSE | D3DFVF_TEX1;

const FALSE: u32 = minwindef::FALSE as u32;
const TRUE: u32 = minwindef::TRUE as u32;

const VERTEX_BUF_ADD_CAPACITY: usize = 5000;
const INDEX_BUF_ADD_CAPACITY: usize = 10000;

static MAT_IDENTITY: D3DMATRIX = D3DMATRIX {
    m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]],
};

#[repr(C)]
struct CustomVertex {
    pos: [f32; 3],
    col: [u8; 4],
    uv: [f32; 2],
}

type Result<T> = core::result::Result<T, HRESULT>;

unsafe fn com_ptr_from_fn<T, F>(fun: F) -> Result<ComPtr<T>>
where
    T: Interface,
    F: FnOnce(&mut *mut T) -> HRESULT,
{
    let mut ptr = ptr::null_mut();
    let res = fun(&mut ptr);
    hresult(res).map(|()| ComPtr::from_raw(ptr))
}

#[inline]
fn hresult(code: HRESULT) -> Result<()> {
    match code {
        S_OK => Ok(()),
        err => Err(err),
    }
}
/// A DirectX 9 renderer for (Imgui-rs)[https://docs.rs/imgui/*/imgui/].
pub struct Renderer {
    device: ComPtr<IDirect3DDevice9>,
    font_tex: ComPtr<IDirect3DBaseTexture9>,
    vertex_buffer: (ComPtr<IDirect3DVertexBuffer9>, usize),
    index_buffer: (ComPtr<IDirect3DIndexBuffer9>, usize),
    textures: Textures<ComPtr<IDirect3DBaseTexture9>>,
}

impl Renderer {
    /// Creates a new renderer for the given [`IDirect3DDevice9`].
    ///
    /// # Safety
    ///
    /// `device` must be a valid [`IDirect3DDevice9`] pointer.
    ///
    /// [`IDirect3DDevice9`]: https://docs.rs/winapi/0.3/x86_64-pc-windows-msvc/winapi/shared/d3d9/struct.IDirect3DDevice9.html
    pub unsafe fn new(ctx: &mut Context, device: ComPtr<IDirect3DDevice9>) -> Result<Self> {
        let font_tex = Self::create_font_texture(ctx.fonts(), &device)?.up();
        ctx.io_mut().backend_flags |= BackendFlags::RENDERER_HAS_VTX_OFFSET;
        ctx.set_renderer_name(String::from(concat!(
            "imgui_dx9_renderer@",
            env!("CARGO_PKG_VERSION")
        )));
        Ok(Renderer {
            vertex_buffer: Self::create_vertex_buffer(&device, 0)?,
            index_buffer: Self::create_index_buffer(&device, 0)?,
            device,
            font_tex,
            textures: Textures::new(),
        })
    }

    /// Creates a new renderer for the given [`IDirect3DDevice9`].
    ///
    /// # Safety
    ///
    /// `device` must be a valid [`IDirect3DDevice9`] pointer.
    ///
    /// [`IDirect3DDevice9`]: https://docs.rs/winapi/0.3/x86_64-pc-windows-msvc/winapi/shared/d3d9/struct.IDirect3DDevice9.html
    pub unsafe fn new_raw(
        im_ctx: &mut imgui::Context,
        device: *mut IDirect3DDevice9,
    ) -> Result<Self> {
        let device = ComPtr::from_raw(device);
        device.AddRef();
        Self::new(im_ctx, device)
    }

    /// The textures registry of this renderer.
    ///
    /// The texture slot at !0 is reserved for the font texture, therefore the
    /// renderer will ignore any texture inserted into said slot.
    #[inline]
    pub fn textures_mut(&mut self) -> &mut Textures<ComPtr<IDirect3DBaseTexture9>> {
        &mut self.textures
    }

    /// The textures registry of this renderer.
    #[inline]
    pub fn textures(&self) -> &Textures<ComPtr<IDirect3DBaseTexture9>> {
        &self.textures
    }

    /// Renders the given [`Ui`] with this renderer.
    ///
    /// Should the [`DrawData`] contain an invalid texture index the renderer
    /// will return `DXGI_ERROR_INVALID_CALL` and immediately stop rendering.
    ///
    /// [`Ui`]: https://docs.rs/imgui/*/imgui/struct.Ui.html
    pub fn render(&mut self, draw_data: &DrawData) -> Result<()> {
        if draw_data.display_size[0] < 0.0 || draw_data.display_size[1] < 0.0 {
            return Ok(());
        }
        unsafe {
            if self.vertex_buffer.1 < draw_data.total_vtx_count as usize {
                self.vertex_buffer =
                    Self::create_vertex_buffer(&self.device, draw_data.total_vtx_count as usize)?;
            }
            if self.index_buffer.1 < draw_data.total_idx_count as usize {
                self.index_buffer =
                    Self::create_index_buffer(&self.device, draw_data.total_idx_count as usize)?;
            }

            let _state_guard = StateBackup::backup(&self.device)?;

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
        self.device.SetTexture(0, self.font_tex.as_raw());
        for draw_list in draw_data.draw_lists() {
            for cmd in draw_list.commands() {
                match cmd {
                    DrawCmd::Elements {
                        count,
                        cmd_params: DrawCmdParams { clip_rect, texture_id, .. },
                    } => {
                        if texture_id != last_tex {
                            let texture = if texture_id.id() == FONT_TEX_ID {
                                self.font_tex.as_raw()
                            } else {
                                self.textures
                                    .get(texture_id)
                                    .ok_or(DXGI_ERROR_INVALID_CALL)?
                                    .as_raw()
                            };
                            self.device.SetTexture(0, texture);
                            last_tex = texture_id;
                        }

                        let r: RECT = RECT {
                            left: ((clip_rect[0] - clip_off[0]) * clip_scale[0]) as i32,
                            top: ((clip_rect[1] - clip_off[1]) * clip_scale[1]) as i32,
                            right: ((clip_rect[2] - clip_off[0]) * clip_scale[0]) as i32,
                            bottom: ((clip_rect[3] - clip_off[1]) * clip_scale[1]) as i32,
                        };
                        self.device.SetScissorRect(&r);
                        self.device.DrawIndexedPrimitive(
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

        let vp = D3DVIEWPORT9 {
            X: 0,
            Y: 0,
            Width: fb_width as _,
            Height: fb_height as _,
            MinZ: 0.0,
            MaxZ: 1.0,
        };

        let device = &*self.device;
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
        device.SetTextureStageState(0, D3DTSS_COLOROP, D3DTOP_MODULATE);
        device.SetTextureStageState(0, D3DTSS_COLORARG1, D3DTA_TEXTURE);
        device.SetTextureStageState(0, D3DTSS_COLORARG2, D3DTA_DIFFUSE);
        device.SetTextureStageState(0, D3DTSS_ALPHAOP, D3DTOP_MODULATE);
        device.SetTextureStageState(0, D3DTSS_ALPHAARG1, D3DTA_TEXTURE);
        device.SetTextureStageState(0, D3DTSS_ALPHAARG2, D3DTA_DIFFUSE);
        device.SetSamplerState(0, D3DSAMP_MINFILTER, D3DTEXF_LINEAR);
        device.SetSamplerState(0, D3DSAMP_MAGFILTER, D3DTEXF_LINEAR);

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
        hresult(vb.Lock(
            0,
            (vtx_count * mem::size_of::<CustomVertex>()) as u32,
            &mut vtx_dst as *mut _ as _,
            D3DLOCK_DISCARD,
        ))?;
        match hresult(ib.Lock(
            0,
            (idx_count * mem::size_of::<DrawIdx>()) as u32,
            &mut idx_dst as *mut _ as _,
            D3DLOCK_DISCARD,
        )) {
            Ok(()) => Ok((
                slice::from_raw_parts_mut(vtx_dst, vtx_count),
                slice::from_raw_parts_mut(idx_dst, idx_count),
            )),
            Err(e) => {
                vb.Unlock();
                Err(e)
            },
        }
    }

    unsafe fn write_buffers(&mut self, draw_data: &DrawData) -> Result<()> {
        let (vb, ib) = (&mut *self.vertex_buffer.0.as_raw(), &mut *self.index_buffer.0.as_raw());

        let (mut vtx_dst, mut idx_dst) = Self::lock_buffers(
            vb,
            ib,
            draw_data.total_vtx_count as usize,
            draw_data.total_idx_count as usize,
        )?;

        for (vbuf, ibuf) in
            draw_data.draw_lists().map(|draw_list| (draw_list.vtx_buffer(), draw_list.idx_buffer()))
        {
            for (vertex, vtx_dst) in vbuf.iter().zip(vtx_dst.iter_mut()) {
                *vtx_dst = CustomVertex {
                    pos: [vertex.pos[0], vertex.pos[1], 0.0],
                    col: [vertex.col[2], vertex.col[1], vertex.col[0], vertex.col[3]],
                    uv: [vertex.uv[0], vertex.uv[1]],
                };
            }
            idx_dst[..ibuf.len()].copy_from_slice(ibuf);
            vtx_dst = &mut vtx_dst[vbuf.len()..];
            idx_dst = &mut idx_dst[ibuf.len()..];
        }
        vb.Unlock();
        ib.Unlock();
        self.device.SetStreamSource(0, vb, 0, mem::size_of::<CustomVertex>() as u32);
        self.device.SetIndices(ib);
        self.device.SetFVF(D3DFVF_CUSTOMVERTEX);
        Ok(())
    }

    unsafe fn create_vertex_buffer(
        device: &ComPtr<IDirect3DDevice9>,
        vtx_count: usize,
    ) -> Result<(ComPtr<IDirect3DVertexBuffer9>, usize)> {
        let len = vtx_count + VERTEX_BUF_ADD_CAPACITY;
        com_ptr_from_fn(|vertex_buffer| {
            device.CreateVertexBuffer(
                (len * mem::size_of::<CustomVertex>()) as u32,
                D3DUSAGE_DYNAMIC | D3DUSAGE_WRITEONLY,
                D3DFVF_CUSTOMVERTEX,
                D3DPOOL_DEFAULT,
                vertex_buffer,
                ptr::null_mut(),
            )
        })
        .map(|vb| (vb, len))
    }

    unsafe fn create_index_buffer(
        device: &ComPtr<IDirect3DDevice9>,
        idx_count: usize,
    ) -> Result<(ComPtr<IDirect3DIndexBuffer9>, usize)> {
        let len = idx_count + INDEX_BUF_ADD_CAPACITY;
        com_ptr_from_fn(|index_buffer| {
            device.CreateIndexBuffer(
                (len * mem::size_of::<DrawIdx>()) as u32,
                D3DUSAGE_DYNAMIC | D3DUSAGE_WRITEONLY,
                if mem::size_of::<DrawIdx>() == 2 { D3DFMT_INDEX16 } else { D3DFMT_INDEX32 },
                D3DPOOL_DEFAULT,
                index_buffer,
                ptr::null_mut(),
            )
        })
        .map(|vb| (vb, len))
    }

    // FIXME, imgui hands us an rgba texture while we make dx9 think it receives an
    // argb texture
    unsafe fn create_font_texture(
        mut fonts: imgui::FontAtlasRefMut<'_>,
        device: &ComPtr<IDirect3DDevice9>,
    ) -> Result<ComPtr<IDirect3DTexture9>> {
        let texture = fonts.build_rgba32_texture();

        let texture_handle = com_ptr_from_fn(|texture_handle| {
            device.CreateTexture(
                texture.width,
                texture.height,
                1,
                D3DUSAGE_DYNAMIC,
                D3DFMT_A8R8G8B8,
                D3DPOLL_DEFAULT,
                texture_handle,
                ptr::null_mut(),
            )
        })?;

        let mut locked_rect: D3DLOCKED_RECT = D3DLOCKED_RECT { Pitch: 0, pBits: ptr::null_mut() };
        hresult(texture_handle.LockRect(0, &mut locked_rect, ptr::null_mut(), 0))?;

        let bits = locked_rect.pBits as *mut u8;
        let pitch = locked_rect.Pitch as usize;
        let height = texture.height as usize;
        let width = texture.width as usize;

        for y in 0..height {
            let d3d9_memory = bits.add(pitch * y);
            let pixels = texture.data.as_ptr();
            let pixels = pixels.add((width * 4) * y);
            std::ptr::copy(pixels, d3d9_memory, width * 4);
        }

        texture_handle.UnlockRect(0);
        fonts.tex_id = TextureId::from(FONT_TEX_ID);
        Ok(texture_handle)
    }
}

struct StateBackup(ComPtr<IDirect3DStateBlock9>);

impl StateBackup {
    unsafe fn backup(device: &ComPtr<IDirect3DDevice9>) -> Result<Self> {
        com_ptr_from_fn(|state_block| device.CreateStateBlock(D3DSBT_ALL, state_block))
            .map(StateBackup)
    }
}

impl Drop for StateBackup {
    #[inline]
    fn drop(&mut self) {
        unsafe { self.0.Apply() };
    }
}
