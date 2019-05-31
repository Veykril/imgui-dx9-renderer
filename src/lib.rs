#![forbid(rust_2018_idioms)]
use imgui::{DrawData, FrameSize, ImDrawIdx, ImGui, Ui};
use winapi::shared::{
    d3d9::{
        IDirect3DDevice9, IDirect3DIndexBuffer9, IDirect3DVertexBuffer9, LPDIRECT3DDEVICE9,
        LPDIRECT3DSTATEBLOCK9, LPDIRECT3DTEXTURE9,
    },
    d3d9types::*,
    windef::RECT,
};

use core::{
    fmt,
    mem::{self, ManuallyDrop},
    ptr::{self, NonNull},
    slice,
};

const D3DPOLL_DEFAULT: u32 = 0;
const D3DFVF_CUSTOMVERTEX: u32 = D3DFVF_XYZ | D3DFVF_DIFFUSE | D3DFVF_TEX1;

#[repr(C)]
struct CustomVertex {
    pos: [f32; 3],
    col: D3DCOLOR,
    uv: [f32; 2],
}

type Textures = imgui::Textures<LPDIRECT3DTEXTURE9>;
type Result<T> = core::result::Result<T, RendererError>;

#[derive(Clone, Debug, PartialEq)]
pub enum RendererError {
    VertexCreation,
    IndexCreation,
    TextureCreation,
    StateBackup,
    WriteBuffer,
}

impl fmt::Display for RendererError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            RendererError::VertexCreation => write!(f, "failed to create vertex buffer"),
            RendererError::IndexCreation => write!(f, "failed to create index buffer"),
            RendererError::TextureCreation => write!(f, "failed to create font texture"),
            RendererError::StateBackup => write!(f, "failed to backup dx9 state"),
            RendererError::WriteBuffer => write!(f, "failed to write to buffer"),
        }
    }
}

impl std::error::Error for RendererError {}

pub struct Renderer {
    device: LPDIRECT3DDEVICE9,
    textures: Textures,
    vertex_buffer: Option<VertexBuffer>,
    index_buffer: Option<IndexBuffer>,
}

impl Renderer {
    pub fn new(imgui: &mut ImGui, device: NonNull<IDirect3DDevice9>) -> Result<Self> {
        unsafe {
            let device = device.as_ptr();
            let textures = Self::create_font_texture(imgui, device)?;
            // device cannot be null at this point
            (((*(*device).lpVtbl).parent).AddRef)(device as _);
            Ok(Renderer {
                device,
                textures,
                vertex_buffer: None,
                index_buffer: None,
            })
        }
    }

    pub fn render(&mut self, ui: Ui<'_>) -> Result<()> {
        ui.render(|ui, draw_data| unsafe {
            match self.vertex_buffer.as_ref() {
                Some(vb) if vb.len() > draw_data.total_vtx_count() => (),
                _ => self.recreate_vertex_buffer(&draw_data)?,
            }
            match self.index_buffer.as_ref() {
                Some(ib) if ib.len() > draw_data.total_idx_count() => (),
                _ => self.recreate_index_buffer(&draw_data)?,
            }

            let _state_backup = StateBackup::backup(self.device)?;

            self.write_buffers(&draw_data)?;

            self.set_render_state(&ui);

            self.render_impl(&draw_data);
            Ok(())
        })
    }

    unsafe fn render_impl(&mut self, draw_data: &DrawData<'_>) {
        let clip_off = (0.0, 0.0);
        let mut vertex_offset = 0u32;
        let mut index_offset = 0;
        for draw_list in draw_data {
            for cmd in draw_list.cmd_buffer {
                let r: RECT = RECT {
                    left: (cmd.clip_rect.x - clip_off.0) as _,
                    top: (cmd.clip_rect.y - clip_off.1) as _,
                    right: (cmd.clip_rect.z - clip_off.0) as _,
                    bottom: (cmd.clip_rect.w - clip_off.1) as _,
                };
                let texture = *self
                    .textures
                    .get(cmd.texture_id.into())
                    .expect("Font hasnt been initialized");
                ((*(*self.device).lpVtbl).SetTexture)(self.device, 0, texture as _);
                ((*(*self.device).lpVtbl).SetScissorRect)(self.device, &r);
                ((*(*self.device).lpVtbl).DrawIndexedPrimitive)(
                    self.device,
                    D3DPT_TRIANGLELIST,
                    vertex_offset as _,
                    0,
                    draw_list.vtx_buffer.len() as u32,
                    index_offset,
                    cmd.elem_count / 3,
                );
                index_offset += cmd.elem_count;
            }
            vertex_offset += draw_list.vtx_buffer.len() as u32;
        }
    }

    #[rustfmt::skip]
    unsafe fn set_render_state(&mut self, ui: &Ui<'_>) {
        let FrameSize {
            logical_size: (width, height),
            hidpi_factor,
        } = ui.frame_size();
        let fb_size = if !(width > 0.0 && height > 0.0) {
            return;
        } else {
            ((width * hidpi_factor) as f32, (height * hidpi_factor) as f32)
        };
        let vp = D3DVIEWPORT9 {
            X: 0,
            Y: 0,
            Width: fb_size.0 as _,
            Height: fb_size.1 as _,
            MinZ: 0.0,
            MaxZ: 1.0,
        };

        ((*(*self.device).lpVtbl).SetViewport)(self.device, &vp);
        ((*(*self.device).lpVtbl).SetPixelShader)(self.device, ptr::null_mut());
        ((*(*self.device).lpVtbl).SetVertexShader)(self.device, ptr::null_mut());
        ((*(*self.device).lpVtbl).SetRenderState)(self.device, D3DRS_CULLMODE, D3DCULL_NONE);
        ((*(*self.device).lpVtbl).SetRenderState)(self.device, D3DRS_LIGHTING, 0);
        ((*(*self.device).lpVtbl).SetRenderState)(self.device, D3DRS_ZENABLE, 0);
        ((*(*self.device).lpVtbl).SetRenderState)(self.device, D3DRS_ALPHABLENDENABLE, 1);
        ((*(*self.device).lpVtbl).SetRenderState)(self.device, D3DRS_ALPHATESTENABLE, 0);
        ((*(*self.device).lpVtbl).SetRenderState)(self.device, D3DRS_BLENDOP, D3DBLENDOP_ADD);
        ((*(*self.device).lpVtbl).SetRenderState)(self.device, D3DRS_SRCBLEND, D3DBLEND_SRCALPHA);
        ((*(*self.device).lpVtbl).SetRenderState)(self.device, D3DRS_DESTBLEND, D3DBLEND_INVSRCALPHA);
        ((*(*self.device).lpVtbl).SetRenderState)(self.device, D3DRS_SCISSORTESTENABLE, 1);
        ((*(*self.device).lpVtbl).SetRenderState)(self.device, D3DRS_SHADEMODE, D3DSHADE_GOURAUD);
        ((*(*self.device).lpVtbl).SetRenderState)(self.device, D3DRS_FOGENABLE, 0);
        ((*(*self.device).lpVtbl).SetTextureStageState)(self.device, 0, D3DTSS_COLOROP, D3DTOP_MODULATE);
        ((*(*self.device).lpVtbl).SetTextureStageState)(self.device, 0, D3DTSS_COLORARG1, D3DTA_TEXTURE);
        ((*(*self.device).lpVtbl).SetTextureStageState)(self.device, 0, D3DTSS_COLORARG2, D3DTA_DIFFUSE);
        ((*(*self.device).lpVtbl).SetTextureStageState)(self.device, 0, D3DTSS_ALPHAOP, D3DTOP_MODULATE);
        ((*(*self.device).lpVtbl).SetTextureStageState)(self.device, 0, D3DTSS_ALPHAARG1, D3DTA_TEXTURE);
        ((*(*self.device).lpVtbl).SetTextureStageState)(self.device, 0, D3DTSS_ALPHAARG2, D3DTA_DIFFUSE);
        ((*(*self.device).lpVtbl).SetSamplerState)(self.device, 0, D3DSAMP_MINFILTER, D3DTEXF_LINEAR);
        ((*(*self.device).lpVtbl).SetSamplerState)(self.device, 0, D3DSAMP_MAGFILTER, D3DTEXF_LINEAR);

        // FIXME move into a configuration
        let (pos_x, pos_y) = (0.0, 0.0);
        let l = pos_x + 0.5;
        let r = pos_x + fb_size.0 + 0.5;
        let t = pos_y + 0.5;
        let b = pos_y + fb_size.1 + 0.5;
        let mat_identity = D3DMATRIX { m: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0]
        ]};
        let mat_projection = D3DMATRIX { m: [
            [2.0/(r-l),   0.0,         0.0, 0.0],
            [0.0,         2.0/(t-b),   0.0, 0.0],
            [0.0,         0.0,         0.5, 0.0],
            [(l+r)/(l-r), (t+b)/(b-t), 0.5, 1.0]
        ]};
        ((*(*self.device).lpVtbl).SetTransform)(self.device, D3DTS_WORLD, &mat_identity);
        ((*(*self.device).lpVtbl).SetTransform)(self.device, D3DTS_VIEW, &mat_identity);
        ((*(*self.device).lpVtbl).SetTransform)(self.device, D3DTS_PROJECTION, &mat_projection);
    }

    unsafe fn write_buffers(&mut self, draw_data: &DrawData<'_>) -> Result<()> {
        let (vb, ib) = (
            self.vertex_buffer
                .as_ref()
                .expect("vertexbuffer should've been initialized"),
            self.index_buffer
                .as_ref()
                .expect("indexbuffer should've been initialized"),
        );
        let mut vtx_dst: *mut CustomVertex = ptr::null_mut();
        let mut idx_dst: *mut ImDrawIdx = ptr::null_mut();
        if ((*(*vb.as_ptr()).lpVtbl).Lock)(
            vb.as_ptr(),
            0,
            (draw_data.total_vtx_count() * mem::size_of::<CustomVertex>()) as u32,
            &mut vtx_dst as *mut _ as _,
            D3DLOCK_DISCARD,
        ) < 0
        {
            return Err(RendererError::WriteBuffer);
        }
        if ((*(*ib.as_ptr()).lpVtbl).Lock)(
            ib.as_ptr(),
            0,
            (draw_data.total_idx_count() * mem::size_of::<ImDrawIdx>()) as u32,
            &mut idx_dst as *mut _ as _,
            D3DLOCK_DISCARD,
        ) < 0
        {
            return Err(RendererError::WriteBuffer);
        }
        for draw_list in draw_data {
            for vertex in draw_list.vtx_buffer.iter() {
                *vtx_dst = CustomVertex {
                    pos: [vertex.pos.x, vertex.pos.y, 0.0],
                    col: (vertex.col & 0xFF00_FF00)
                        | ((vertex.col & 0x00FF_0000) >> 16)
                        | ((vertex.col & 0xFF) << 16),
                    uv: [vertex.uv.x, vertex.uv.y],
                };
                vtx_dst = vtx_dst.add(1);
            }
            ptr::copy(
                draw_list.idx_buffer.as_ptr(),
                idx_dst as _,
                draw_list.idx_buffer.len(),
            );
            idx_dst = idx_dst.add(draw_list.idx_buffer.len());
        }
        ((*(*vb.as_ptr()).lpVtbl).Unlock)(vb.as_ptr());
        ((*(*ib.as_ptr()).lpVtbl).Unlock)(ib.as_ptr());
        ((*(*self.device).lpVtbl).SetStreamSource)(
            self.device,
            0,
            vb.as_ptr(),
            0,
            mem::size_of::<CustomVertex>() as u32,
        );
        ((*(*self.device).lpVtbl).SetIndices)(self.device, ib.as_ptr());
        ((*(*self.device).lpVtbl).SetFVF)(self.device, D3DFVF_CUSTOMVERTEX);
        Ok(())
    }

    unsafe fn recreate_vertex_buffer(&mut self, draw_data: &DrawData<'_>) -> Result<()> {
        let mut vertex_buffer = ptr::null_mut();
        let len = draw_data.total_vtx_count() + 5000;
        if ((*(*self.device).lpVtbl).CreateVertexBuffer)(
            self.device,
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

    unsafe fn recreate_index_buffer(&mut self, draw_data: &DrawData<'_>) -> Result<()> {
        let mut index_buffer = ptr::null_mut();
        let len = draw_data.total_idx_count() + 10000;
        if ((*(*self.device).lpVtbl).CreateIndexBuffer)(
            self.device,
            (len * mem::size_of::<ImDrawIdx>()) as u32,
            D3DUSAGE_DYNAMIC | D3DUSAGE_WRITEONLY,
            if mem::size_of::<ImDrawIdx>() == 2 {
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

    fn create_font_texture(imgui: &mut ImGui, device: LPDIRECT3DDEVICE9) -> Result<Textures> {
        let texture_handle = imgui.prepare_texture(|handle| unsafe {
            let mut texture_handle: LPDIRECT3DTEXTURE9 = ptr::null_mut();
            if ((*(*device).lpVtbl).CreateTexture)(
                device,
                handle.width,
                handle.height,
                1,
                D3DUSAGE_DYNAMIC,
                D3DFMT_A8R8G8B8,
                D3DPOLL_DEFAULT,
                &mut texture_handle,
                ptr::null_mut(),
            ) < 0
            {
                return Err(RendererError::TextureCreation);
            }
            let mut tex_locked_rect: D3DLOCKED_RECT = D3DLOCKED_RECT {
                Pitch: 0,
                pBits: ptr::null_mut(),
            };
            if ((*(*texture_handle).lpVtbl).LockRect)(
                texture_handle,
                0,
                &mut tex_locked_rect,
                ptr::null_mut(),
                0,
            ) != 0
            {
                return Err(RendererError::TextureCreation);
            }

            slice::from_raw_parts_mut(tex_locked_rect.pBits as *mut u8, handle.pixels.len())
                .copy_from_slice(handle.pixels);

            ((*(*texture_handle).lpVtbl).UnlockRect)(texture_handle, 0);
            Ok(texture_handle)
        })?;
        let mut textures = Textures::new();
        imgui.set_font_texture_id(textures.insert(texture_handle));
        Ok(textures)
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe { ((*(*self.device).lpVtbl).parent.Release)(self.device as _) };
    }
}

struct VertexBuffer(NonNull<[IDirect3DVertexBuffer9]>);

impl VertexBuffer {
    #[inline]
    fn len(&self) -> usize {
        unsafe { self.0.as_ref().len() }
    }

    #[inline]
    fn as_ptr(&self) -> *mut IDirect3DVertexBuffer9 {
        unsafe { (*self.0.as_ptr()).as_mut_ptr() }
    }
}

impl Drop for VertexBuffer {
    fn drop(&mut self) {
        unsafe { ((*(*self.as_ptr()).lpVtbl).parent.parent.Release)(self.0.as_ptr() as _) };
    }
}

struct IndexBuffer(NonNull<[IDirect3DIndexBuffer9]>);

impl IndexBuffer {
    #[inline]
    fn len(&self) -> usize {
        unsafe { self.0.as_ref().len() }
    }

    #[inline]
    fn as_ptr(&self) -> *mut IDirect3DIndexBuffer9 {
        unsafe { (*self.0.as_ptr()).as_mut_ptr() }
    }
}

impl Drop for IndexBuffer {
    fn drop(&mut self) {
        unsafe { ((*(*self.as_ptr()).lpVtbl).parent.parent.Release)(self.0.as_ptr() as _) };
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
        if ((*(*device).lpVtbl).CreateStateBlock)(device, D3DSBT_ALL, &mut this.state_block) < 0 {
            return Err(RendererError::StateBackup);
        }
        ((*(*device).lpVtbl).GetTransform)(device, D3DTS_WORLD, &mut this.last_world);
        ((*(*device).lpVtbl).GetTransform)(device, D3DTS_VIEW, &mut this.last_view);
        ((*(*device).lpVtbl).GetTransform)(device, D3DTS_PROJECTION, &mut this.last_projection);
        Ok(ManuallyDrop::into_inner(this))
    }
}

impl Drop for StateBackup {
    fn drop(&mut self) {
        unsafe {
            ((*(*self.device).lpVtbl).SetTransform)(self.device, D3DTS_WORLD, &self.last_world);
            ((*(*self.device).lpVtbl).SetTransform)(self.device, D3DTS_VIEW, &self.last_view);
            ((*(*self.device).lpVtbl).SetTransform)(
                self.device,
                D3DTS_PROJECTION,
                &self.last_projection,
            );
            ((*(*self.state_block).lpVtbl).Apply)(self.state_block);
            ((*(*self.state_block).lpVtbl).parent.Release)(self.state_block as _);
        }
    }
}
